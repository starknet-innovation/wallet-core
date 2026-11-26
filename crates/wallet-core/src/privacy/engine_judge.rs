//! Judges what the engine returns (the extension's `official-sdk-results.ts`). Everything is
//! untrusted: warnings are shape-checked before any code is read, and a proof is accepted only as
//! the configured pool's `apply_actions` within the wallet's bounds, the one call that may carry
//! a prepared proof into submission.

use super::engine::{CallAndProof, EngineProof, EngineWarning, ProofLimits, ProofRefusal};
use crate::js::num::utf16_len;
use crate::Felt;

/// The pool's proof-bearing entrypoint.
pub const APPLY_ACTIONS: &str = "apply_actions";
/// The one warning code the pinned SDK declares: the proof would link the user.
const USER_LINKAGE: &str = "USER_LINKAGE";
const MAX_WARNING_CODE: usize = 64;
const MAX_WARNING_MESSAGE: usize = 512;

fn is_warning_shape(warning: &EngineWarning) -> bool {
    utf16_len(&warning.code) <= MAX_WARNING_CODE
        && utf16_len(&warning.message) <= MAX_WARNING_MESSAGE
}

/// `assertNoSdkWarnings`: the count and every warning's shape first, so a malformed envelope is
/// never read as a recognised signal; then `USER_LINKAGE` is a privacy leak, and any other code
/// is refused because this core has not reviewed it.
pub(super) fn judge_warnings(
    warnings: &[EngineWarning],
    limits: &ProofLimits,
) -> Result<(), ProofRefusal> {
    if warnings.len() > limits.sdk_warnings || !warnings.iter().all(is_warning_shape) {
        return Err(ProofRefusal::InvalidWarnings);
    }
    if warnings.iter().any(|warning| warning.code == USER_LINKAGE) {
        return Err(ProofRefusal::PrivacyLeak);
    }
    if !warnings.is_empty() {
        return Err(ProofRefusal::UnsupportedWarning);
    }
    Ok(())
}

/// `readCallAndProof`, after the warnings: the bounds, then the call pinned to `pool`'s
/// `apply_actions`. Stricter than the extension in one place: with no simulation here, an empty
/// proof is refused rather than passed on as unsubmittable.
pub(super) fn judge_proof(
    result: EngineProof,
    pool: &Felt,
    limits: &ProofLimits,
) -> Result<CallAndProof, ProofRefusal> {
    judge_warnings(&result.warnings, limits)?;
    let EngineProof { call, proof, .. } = result;
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
    use crate::privacy::engine::Proof;
    use crate::send::calls::Call;

    fn warning(code: &str, message: &str) -> EngineWarning {
        EngineWarning {
            code: code.into(),
            message: message.into(),
        }
    }

    fn judge(warnings: &[EngineWarning]) -> Result<(), ProofRefusal> {
        judge_warnings(warnings, &proof_limits())
    }

    // `official-sdk-results.test.ts`, "official SDK warning envelope".
    #[test]
    fn accepts_no_warnings_and_maps_linkage_to_a_privacy_leak() {
        assert_eq!(judge(&[]), Ok(()));
        assert_eq!(
            judge(&[warning(USER_LINKAGE, "linked")]),
            Err(ProofRefusal::PrivacyLeak)
        );
    }

    #[test]
    fn checks_warning_shape_before_reading_any_code() {
        let long_code = "x".repeat(65);
        let long_message = "m".repeat(513);
        assert_eq!(
            judge(&[warning(&long_code, "long code")]),
            Err(ProofRefusal::InvalidWarnings)
        );
        // A linkage warning out of shape is unusable, not a recognised signal.
        assert_eq!(
            judge(&[warning(USER_LINKAGE, &long_message)]),
            Err(ProofRefusal::InvalidWarnings)
        );
        // Counted as JavaScript counts: 64 two-unit characters are 128 units.
        assert_eq!(
            judge(&[warning(&"😀".repeat(33), "")]),
            Err(ProofRefusal::InvalidWarnings)
        );
        assert_eq!(
            judge(&[warning(&"x".repeat(64), &"m".repeat(512))]),
            Err(ProofRefusal::UnsupportedWarning)
        );
    }

    #[test]
    fn bounds_the_warning_count_ahead_of_the_linkage_branch() {
        let linked = warning(USER_LINKAGE, "linked");
        assert_eq!(
            judge(&vec![linked.clone(); 64]),
            Err(ProofRefusal::PrivacyLeak)
        );
        assert_eq!(judge(&vec![linked; 65]), Err(ProofRefusal::InvalidWarnings));
    }

    #[test]
    fn refuses_an_unreviewed_warning_code() {
        assert_eq!(
            judge(&[warning("UNKNOWN", "review")]),
            Err(ProofRefusal::UnsupportedWarning)
        );
    }

    const POOL: Felt = Felt::from_hex_unchecked("0x7");

    fn proof() -> EngineProof {
        EngineProof {
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
            warnings: vec![],
        }
    }

    fn judged(result: EngineProof) -> Result<CallAndProof, ProofRefusal> {
        judge_proof(result, &POOL, &proof_limits())
    }

    #[test]
    fn accepts_the_pools_apply_actions_unchanged() {
        let expected = proof();
        assert_eq!(
            judged(proof()),
            Ok(CallAndProof {
                call: expected.call,
                proof: expected.proof,
            })
        );
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
    fn a_proofs_warnings_are_judged_too() {
        let mut linked = proof();
        linked.warnings = vec![warning(USER_LINKAGE, "linked")];
        assert_eq!(judged(linked), Err(ProofRefusal::PrivacyLeak));
    }
}
