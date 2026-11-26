//! STRK20 proofs through an engine the wallet supplies (P15). The engine is StarkWare's privacy
//! SDK, run by the app in a sandboxed runtime; this core drives it through `PrivacyEngine` and
//! keeps what the extension keeps outside it: the account key, the `CallSet` signature, and the
//! judging of everything the engine returns (`call_set`, `engine_judge`).
//!
//! One job is one proof: `invocation` hands the engine the job (with the viewing key, its one
//! crossing) and gets back the calls the SDK asks to sign; the core decodes them as the pool's one
//! `compile_actions` for this account and this viewing key and binds the value they move to the
//! job's reviewed actions (`pool_actions`), computes the `CallSet` digest itself, has the wallet
//! sign it, and passes only the signature to `prove`. No signing callback reaches the engine.
//! The proof that comes back is refused unless it is the configured pool's `apply_actions` within
//! the wallet's bounds, with no SDK warning. Any refusal cancels the job.

use super::call_set::{call_set_hash, sign_call_set};
use super::config::is_service_url;
use super::engine_judge::{judge_proof, judge_warnings};
use super::pool_actions::bound_sign_request;
use crate::send::calls::Call;
use crate::send::types::SignRefusal;
use crate::{Felt, SecretFelt, StarkSignature};

/// The core's name for one proof, echoed back to the engine in `prove` and `cancel`.
pub type JobId = u64;

/// One reviewed action, as the SDK's builder takes it. Activation, shielding, unshielding,
/// private sends and open notes (the SDK's `Open`); dapp invokes and shadow accounts join with
/// their flows, and the binding in `pool_actions` with them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineAction {
    /// `register`: set the account's viewing key in the pool, alone or ahead of deposits.
    Register,
    /// `with(token).deposit`: public balance into the pool, to `recipient` or the account.
    Deposit {
        token: Felt,
        amount: Felt,
        recipient: Option<Felt>,
    },
    /// `with(token).transfer`: a private note for `recipient`.
    Transfer {
        token: Felt,
        recipient: Felt,
        amount: Felt,
    },
    /// `with(token).transfer({amount: Open})`: an open note for `recipient`, whose amount a later
    /// call fills (the SDK's `CreateOpenNote`).
    OpenTransfer { token: Felt, recipient: Felt },
    /// `with(token).withdraw`: out of the pool to `recipient`'s public balance.
    Withdraw {
        token: Felt,
        recipient: Felt,
        amount: Felt,
    },
}

/// Everything the engine needs for one proof. The viewing key goes no further than the engine,
/// which sends it to the discovery service and the prover as the extension's SDK does (PLAN.md,
/// "P15 spike findings"); it is a `SecretFelt`, so it is redacted from `Debug` and zeroed when
/// the job is dropped.
#[derive(Debug)]
pub struct EngineJob {
    pub id: JobId,
    /// The chain the `CallSet` is bound to (`NetworkId::chain_id`).
    pub chain_id: Felt,
    pub pool: Felt,
    pub account: Felt,
    pub actions: Vec<EngineAction>,
    /// The block the proof is made against (the extension's maturity read).
    pub proving_block: u64,
    /// Where the engine is told to post the viewing key, checked with `config::is_service_url`
    /// before the job is handed over. That guards against a misconfigured route only: a hostile
    /// engine can send the key anywhere it can reach, which only the app's native fetch bridge
    /// (its origin allowlist) can stop.
    pub discovery_url: String,
    pub prover_url: String,
    /// The network's RPC, for the pool's nonce. Not checked: the extension's SDK never posts the
    /// key there (it never calls `simulate`), and nothing here can make a hostile engine agree.
    pub node_url: String,
    pub viewing_key: SecretFelt,
}

/// A warning the SDK attached to a result, unread until judged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineWarning {
    pub code: String,
    pub message: String,
}

/// What `createProofInvocation` asked the account to sign, and its warnings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignRequest {
    pub calls: Vec<Call>,
    pub warnings: Vec<EngineWarning>,
}

/// A proof as `STRK20_PROOF`: opaque data plus felt outputs and facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proof {
    pub data: String,
    pub output: Vec<Felt>,
    pub proof_facts: Vec<Felt>,
}

/// What `executeWithInvocation` returned: the call to submit, its proof, and warnings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineProof {
    pub call: Call,
    pub proof: Proof,
    pub warnings: Vec<EngineWarning>,
}

/// A judged proof: the configured pool's `apply_actions` and its proof, the only proof-bearing
/// call a wallet may submit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallAndProof {
    pub call: Call,
    pub proof: Proof,
}

/// Why the engine could not finish. Carries no engine text, so nothing the runtime says reaches
/// a log through the core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineError {
    /// The SDK or a service it calls failed.
    Unavailable,
    /// The wait budget for this step ran out.
    TimedOut,
}

/// The engine, implemented by each app over its runtime. Every answer is untrusted.
pub trait PrivacyEngine {
    /// Discovers notes and builds the proof invocation for `job`; returns the calls the SDK's
    /// signer was asked to sign. The engine keeps the job (and the SDK's pending signer) until
    /// `prove` or `cancel`.
    fn invocation(&self, job: EngineJob) -> Result<SignRequest, EngineError>;
    /// Completes the SDK's signing with `signature` and proves job `id`.
    fn prove(&self, id: JobId, signature: Vec<Felt>) -> Result<EngineProof, EngineError>;
    /// Drops job `id` and whatever the engine holds for it. Called after any refusal.
    fn cancel(&self, id: JobId);
}

/// The wallet's bounds on what the engine returns (the extension's `STRK20_LIMITS`). `prove`
/// clamps each to `ProofLimits::CEILING`, so a wallet can tighten them but never widen them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProofLimits {
    pub sdk_warnings: usize,
    /// Calldata of the call to sign and of the call to submit.
    pub invoke_calldata_felts: usize,
    pub proof_output_felts: usize,
    pub proof_facts: usize,
    /// In UTF-16 code units, as JavaScript counts `data.length`.
    pub proof_data_characters: usize,
}

impl ProofLimits {
    /// The extension's `STRK20_LIMITS` for the protocol's call-and-proof envelope
    /// (`@bramble/wallet-protocol/strk20`), the most any wallet on this core may admit.
    pub const CEILING: Self = Self {
        sdk_warnings: 64,
        invoke_calldata_felts: 4096,
        proof_output_felts: 4096,
        proof_facts: 4096,
        proof_data_characters: 512 * 1024,
    };

    fn clamped(&self) -> Self {
        let ceiling = Self::CEILING;
        Self {
            sdk_warnings: self.sdk_warnings.min(ceiling.sdk_warnings),
            invoke_calldata_felts: self
                .invoke_calldata_felts
                .min(ceiling.invoke_calldata_felts),
            proof_output_felts: self.proof_output_felts.min(ceiling.proof_output_felts),
            proof_facts: self.proof_facts.min(ceiling.proof_facts),
            proof_data_characters: self
                .proof_data_characters
                .min(ceiling.proof_data_characters),
        }
    }
}

/// Why a proof was refused. No variant carries input or engine text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofRefusal {
    /// A discovery or prover URL is not a service URL; the key was not handed over.
    InvalidRoute,
    Engine(EngineError),
    /// The calls to sign are not the pool's one `compile_actions` for this account and viewing
    /// key, or they move value the job did not ask for (or omit some it did).
    UnboundSignRequest,
    /// The session ended before the `CallSet` was signed.
    Locked,
    /// The wallet could not sign, or its signature is not the account's over the digest.
    SigningFailed,
    /// The SDK reported `USER_LINKAGE`.
    PrivacyLeak,
    /// Too many warnings, or one out of shape.
    InvalidWarnings,
    /// A warning code this core has not reviewed.
    UnsupportedWarning,
    /// The call or proof is over a bound.
    InvalidCallOrProof,
    /// The call is not the configured pool's `apply_actions`, or the proof is empty or partial.
    UnboundCallOrProof,
}

impl From<EngineError> for ProofRefusal {
    fn from(error: EngineError) -> Self {
        Self::Engine(error)
    }
}

impl From<SignRefusal> for ProofRefusal {
    fn from(refusal: SignRefusal) -> Self {
        match refusal {
            SignRefusal::Locked => Self::Locked,
            SignRefusal::Failed => Self::SigningFailed,
        }
    }
}

/// Runs one job to a judged proof. `public_key` is the account's signer key, which the
/// signature must verify under; `sign` signs a hash with the account's key under the session
/// the job was approved in (the plan's `PlanHost::sign`), and is called at most once, with the
/// `CallSet` digest the core computed, and only once the calls are bound to `job.actions`. Any
/// refusal after the job was handed over cancels it, and so does a panic in a build that unwinds
/// (release builds abort on panic, which ends the engine's host with the process).
pub fn prove<E: PrivacyEngine + ?Sized>(
    engine: &E,
    job: EngineJob,
    public_key: &Felt,
    limits: &ProofLimits,
    sign: impl FnOnce(&Felt) -> Result<StarkSignature, SignRefusal>,
) -> Result<CallAndProof, ProofRefusal> {
    if !is_service_url(&job.discovery_url) || !is_service_url(&job.prover_url) {
        return Err(ProofRefusal::InvalidRoute);
    }
    let limits = &limits.clamped();
    let (id, chain_id, pool, account) = (job.id, job.chain_id, job.pool, job.account);
    // Kept to bind the calls to sign: the key slot must hold this key, and the actions are what
    // was reviewed. Both stay in Rust and are dropped (the key zeroed) when `prove` returns.
    let (viewing_key, reviewed) = (job.viewing_key.clone(), job.actions.clone());
    let mut guard = CancelOnDrop {
        engine,
        id,
        armed: true,
    };
    let request = engine.invocation(job)?;
    judge_warnings(&request.warnings, limits)?;
    bound_sign_request(
        &request.calls,
        &pool,
        &account,
        &viewing_key,
        &reviewed,
        limits,
    )?;
    let hash = call_set_hash(&account, &request.calls, &chain_id);
    let signature = sign_call_set(&hash, public_key, sign)?;
    let proof = judge_proof(engine.prove(id, signature)?, &pool, limits)?;
    guard.armed = false;
    Ok(proof)
}

/// Cancels the job unless disarmed: on every early return, and on a panic only where panics unwind
/// (release builds set `panic = "abort"`).
struct CancelOnDrop<'a, E: PrivacyEngine + ?Sized> {
    engine: &'a E,
    id: JobId,
    armed: bool,
}

impl<E: PrivacyEngine + ?Sized> Drop for CancelOnDrop<'_, E> {
    fn drop(&mut self) {
        if self.armed {
            self.engine.cancel(self.id);
        }
    }
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
