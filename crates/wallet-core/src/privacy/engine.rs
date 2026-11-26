//! STRK20 proofs through an engine the wallet supplies (P15). The engine is StarkWare's privacy
//! SDK, run by the app in a sandboxed runtime; this core drives it through `PrivacyEngine` and
//! keeps what the extension keeps outside it: the account key, the `CallSet` signature, and the
//! judging of everything the engine returns (`call_set`, `engine_judge`).
//!
//! One job is one proof: `invocation` hands the engine the job (with the viewing key, its one
//! crossing) and gets back the calls the SDK asks to sign; the core checks they are the pool's
//! one `compile_actions` for this account, computes the `CallSet` digest itself, has the wallet
//! sign it, and passes only the signature to `prove`. No signing callback reaches the engine.
//! The proof that comes back is refused unless it is the configured pool's `apply_actions` within
//! the wallet's bounds, with no SDK warning. Any refusal cancels the job.

use super::call_set::{bound_sign_request, call_set_hash, sign_call_set};
use super::config::is_service_url;
use super::engine_judge::{judge_proof, judge_warnings};
use crate::send::calls::Call;
use crate::send::types::SignRefusal;
use crate::{Felt, SecretFelt, StarkSignature};

/// The core's name for one proof, echoed back to the engine in `prove` and `cancel`.
pub type JobId = u64;

/// A transfer's amount: a fixed amount, or an open note (the SDK's `Open`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferAmount {
    Fixed(Felt),
    Open,
}

/// One reviewed action, as the SDK's builder takes it. Activation, shielding, unshielding and
/// private sends; dapp invokes, shadow accounts and surplus routing join with their flows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EngineAction {
    /// `register`: set the account's viewing key in the pool. Alone in its job.
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
        amount: TransferAmount,
    },
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
    /// Where the engine may post the viewing key: `config::is_service_url`, checked before the
    /// job is handed over.
    pub discovery_url: String,
    pub prover_url: String,
    /// The network's RPC, for the pool's nonce. The SDK never posts the key there (the
    /// extension never calls `simulate`), so it is not held to the service-URL rule.
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

/// The wallet's bounds on what the engine returns (the extension's `STRK20_LIMITS`).
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

/// Why a proof was refused. No variant carries input or engine text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofRefusal {
    /// A discovery or prover URL is not a service URL; the key was not handed over.
    InvalidRoute,
    Engine(EngineError),
    /// The calls to sign are not the pool's one `compile_actions` for this account.
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
/// `CallSet` digest the core computed. Any refusal after the job was handed over cancels it.
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
    let (id, chain_id, pool, account) = (job.id, job.chain_id, job.pool, job.account);
    let run = || {
        let request = engine.invocation(job)?;
        judge_warnings(&request.warnings, limits)?;
        bound_sign_request(&request.calls, &pool, &account, limits)?;
        let hash = call_set_hash(&account, &request.calls, &chain_id);
        let signature = sign_call_set(&hash, public_key, sign)?;
        judge_proof(engine.prove(id, signature)?, &pool, limits)
    };
    let result = run();
    if result.is_err() {
        engine.cancel(id);
    }
    result
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
