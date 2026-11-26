//! What one STRK20 proof is made from and what it ends in (P15): the reviewed actions the wallet
//! asks for, the wallet's bounds, the judged `apply_actions` call and proof, and every refusal on
//! the way. `job::ProofJob` produces them; the judge here is the last check before a proof may
//! reach submission (the extension's `official-sdk-results.ts` `readCallAndProof`).

use super::compiler::CompileRefusal;
use super::types::PrivacyFailure;
use crate::js::num::utf16_len;
use crate::send::calls::Call;
use crate::send::types::SignRefusal;
use crate::Felt;

/// The pool's proof-bearing entrypoint.
pub const APPLY_ACTIONS: &str = "apply_actions";

/// One item of a reviewed invoke's calldata: a felt, or a value the job resolves while it
/// compiles (the extension's `${poolAddress}` and `${openNoteIds[N]}` placeholders, whose grammar
/// is Bramble's).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvokeArg {
    Felt(Felt),
    /// The pool's address.
    PoolAddress,
    /// The id of the batch's `N`th open note (from 0), in the order the batch creates them.
    OpenNoteId(usize),
}

/// The anonymizer's `CollectPolicy` for the open notes a shadow-account invoke settles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectPolicy {
    All,
    Diff,
    Exact(u128),
}

/// One reviewed action, as the SDK's builder takes it: activation, shielding, unshielding,
/// private sends, open notes (the SDK's `Open`), an external invoke and a shadow-account invoke.
/// At most one invoke of either kind per job (the pool's one invoke phase).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewedAction {
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
    /// `invoke`: the pool calls `contract` with `calldata` (the SDK's `InvokeExternal`).
    Invoke {
        contract: Felt,
        calldata: Vec<InvokeArg>,
    },
    /// `shadowAccounts(dapp_name).invoke(nonce, {calls, collectPolicy})`: `calls` run from the
    /// shadow account of (`dapp_name`, `nonce`) through the job's anonymizer, which settles the
    /// batch's open notes under `collect` (the SDK's `ComputeAndInvoke`).
    ShadowInvoke {
        dapp_name: Felt,
        nonce: Felt,
        calls: Vec<Call>,
        collect: CollectPolicy,
    },
}

/// A proof as `STRK20_PROOF`: opaque data plus felt outputs and facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proof {
    pub data: String,
    pub output: Vec<Felt>,
    pub proof_facts: Vec<Felt>,
}

/// A judged proof: the configured pool's `apply_actions` and its proof, the only proof-bearing
/// call a wallet may submit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallAndProof {
    pub call: Call,
    pub proof: Proof,
}

/// The wallet's bounds on a proof job (the extension's `STRK20_LIMITS`). Each is clamped to
/// `ProofLimits::CEILING`, so a wallet can tighten them but never widen them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProofLimits {
    /// Calldata of the `compile_actions` call to sign and of the call to submit.
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
        invoke_calldata_felts: 4096,
        proof_output_felts: 4096,
        proof_facts: 4096,
        proof_data_characters: 512 * 1024,
    };

    pub(super) fn clamped(&self) -> Self {
        let ceiling = Self::CEILING;
        Self {
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

/// Why a proof job ended without a proof. No variant carries input, service text, a note or key
/// material; the wrapped failures keep which step and which rule it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofRefusal {
    /// The route has OHTTP on or is another network's, or a discovery or prover URL is not a
    /// service URL. Nothing was requested.
    InvalidRoute,
    /// A discovery walk failed (`Reorg` only on the second reorg; the first restarts the job).
    Discovery(PrivacyFailure),
    /// The compiler refused the reviewed actions against what discovery found.
    Compile(CompileRefusal),
    /// The pool's nonce could not be read (transport failure, an error, or no nonce).
    NonceUnavailable,
    /// The compiled `compile_actions` is not the pool's one call for this account and viewing
    /// key, or it moves value the job did not ask for (or omits some it did): a compiler bug. Also
    /// a batch the reviewed actions alone put past the pool's `MAX_ACTIONS` (too many notes is
    /// `Compile(TooManyNotes)`).
    UnboundSignRequest,
    /// The session ended: no viewing key, or the wallet refused to sign as locked.
    Locked,
    /// The wallet could not sign, or its signature is not the account's over the digest.
    SigningFailed,
    /// The compile would open more than one channel (the SDK's `USER_LINKAGE`).
    PrivacyLeak,
    /// The call or proof is over a bound.
    InvalidCallOrProof,
    /// The call is not the configured pool's `apply_actions`, or the proof is empty or partial
    /// (the prover's reply has no message from the pool, or more than one).
    UnboundCallOrProof,
    /// The prover answered with an error it does not ask to be retried (any HTTP status but 503,
    /// any JSON-RPC code but `-32005`), or kept asking past the retry policy. Its text is dropped.
    ProverFailed,
    /// The prover could not be reached or timed out. Not retried, as the SDK does not.
    ProverUnavailable,
    /// The prover's reply is not the `starknet_proveTransaction` result schema.
    InvalidProverReply,
}

impl From<SignRefusal> for ProofRefusal {
    fn from(refusal: SignRefusal) -> Self {
        match refusal {
            SignRefusal::Locked => Self::Locked,
            SignRefusal::Failed => Self::SigningFailed,
        }
    }
}

impl From<CompileRefusal> for ProofRefusal {
    fn from(refusal: CompileRefusal) -> Self {
        Self::Compile(refusal)
    }
}

/// `readCallAndProof`: the bounds, then the call pinned to `pool`'s `apply_actions`. Stricter
/// than the extension in one place: with no simulation here, an empty proof is refused rather
/// than passed on as unsubmittable. `limits` are already clamped.
pub(super) fn judge_proof(
    call: Call,
    proof: Proof,
    pool: &Felt,
    limits: &ProofLimits,
) -> Result<CallAndProof, ProofRefusal> {
    if call.calldata.len() > limits.invoke_calldata_felts
        || proof.output.len() > limits.proof_output_felts
        || proof.proof_facts.len() > limits.proof_facts
        || utf16_len(&proof.data) > limits.proof_data_characters
    {
        return Err(ProofRefusal::InvalidCallOrProof);
    }
    let proved =
        !proof.data.is_empty() && !proof.output.is_empty() && !proof.proof_facts.is_empty();
    // The call's calldata is not compared with the signed `compile_actions`, as in the extension:
    // the pool verifies the proof, and the CallSet signature over it, on chain.
    if call.contract_address != *pool || call.entrypoint != APPLY_ACTIONS || !proved {
        return Err(ProofRefusal::UnboundCallOrProof);
    }
    Ok(CallAndProof { call, proof })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::privacy::config::testing::proof_limits;

    const POOL: Felt = Felt::from_hex_unchecked("0x7");

    fn proof() -> CallAndProof {
        CallAndProof {
            call: Call {
                contract_address: POOL,
                entrypoint: APPLY_ACTIONS.into(),
                calldata: vec![Felt::ONE, Felt::TWO],
            },
            proof: Proof {
                data: "cHJvb2Y=".into(),
                output: vec![Felt::THREE],
                proof_facts: vec![Felt::from(4u64)],
            },
        }
    }

    fn judged(result: CallAndProof) -> Result<CallAndProof, ProofRefusal> {
        judge_proof(result.call, result.proof, &POOL, &proof_limits())
    }

    #[test]
    fn accepts_the_pools_apply_actions_unchanged() {
        assert_eq!(judged(proof()), Ok(proof()));
    }

    #[test]
    fn refuses_any_other_call() {
        let mut other_pool = proof();
        other_pool.call.contract_address = Felt::from(8u64);
        let mut other_entry = proof();
        other_entry.call.entrypoint = "compile_actions".into();
        let mut cased = proof();
        cased.call.entrypoint = "Apply_actions".into();
        for refused in [other_pool, other_entry, cased] {
            assert_eq!(judged(refused), Err(ProofRefusal::UnboundCallOrProof));
        }
    }

    #[test]
    fn refuses_an_empty_or_partial_proof() {
        let mut empty = proof();
        empty.proof = Proof {
            data: String::new(),
            output: vec![],
            proof_facts: vec![],
        };
        let mut no_data = proof();
        no_data.proof.data.clear();
        let mut no_output = proof();
        no_output.proof.output.clear();
        let mut no_facts = proof();
        no_facts.proof.proof_facts.clear();
        for refused in [empty, no_data, no_output, no_facts] {
            assert_eq!(judged(refused), Err(ProofRefusal::UnboundCallOrProof));
        }
    }

    #[test]
    fn enforces_the_calldata_and_proof_bounds() {
        let limits = proof_limits();
        let mut calldata = proof();
        calldata
            .call
            .calldata
            .resize(limits.invoke_calldata_felts + 1, Felt::ZERO);
        let mut output = proof();
        output
            .proof
            .output
            .resize(limits.proof_output_felts + 1, Felt::ZERO);
        let mut facts = proof();
        facts
            .proof
            .proof_facts
            .resize(limits.proof_facts + 1, Felt::ZERO);
        let mut data = proof();
        data.proof.data = "A".repeat(limits.proof_data_characters + 1);
        for refused in [calldata, output, facts, data] {
            assert_eq!(judged(refused), Err(ProofRefusal::InvalidCallOrProof));
        }
        let mut at_bounds = proof();
        at_bounds
            .call
            .calldata
            .resize(limits.invoke_calldata_felts, Felt::ZERO);
        at_bounds.proof.data = "A".repeat(limits.proof_data_characters);
        assert!(judged(at_bounds).is_ok());
    }

    #[test]
    fn wider_limits_are_clamped_to_the_ceiling() {
        let wide = ProofLimits {
            invoke_calldata_felts: usize::MAX,
            proof_output_felts: usize::MAX,
            proof_facts: usize::MAX,
            proof_data_characters: usize::MAX,
        };
        assert_eq!(wide.clamped(), ProofLimits::CEILING);
        let tight = ProofLimits {
            proof_facts: 1,
            ..ProofLimits::CEILING
        };
        assert_eq!(tight.clamped(), tight);
    }
}
