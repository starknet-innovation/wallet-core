//! One call in an Argent multisig transaction, as the platform's `ArgentMultisigCall` and its
//! trust-boundary validators `isArgentMultisigCall(s)`
//! (`packages/platform/starknet/src/rpc/multisig-invoke-{types,prepared}.ts`): canonical felts
//! only, a plain entrypoint name, and the request budgets. Every untrusted batch passes these
//! before it is reviewed, parsed or prepared.

use serde::{Deserialize, Serialize};

use crate::felt;
use crate::js::felt::is_non_zero_felt;

/// `ARGENT_MULTISIG_INVOKE_LIMITS`: calls in one transaction, felts in one call, felts in all.
/// Bounds on untrusted input, so they stay here rather than with the wallet.
pub const MAX_CALLS: usize = 16;
pub const MAX_CALL_CALLDATA: usize = 256;
pub const MAX_TOTAL_CALLDATA: usize = 512;

/// `ArgentMultisigCall`: one Cairo-1 call in canonical felts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArgentMultisigCall {
    pub calldata: Vec<String>,
    pub contract_address: String,
    pub entrypoint: String,
}

fn is_canonical(value: &str) -> bool {
    felt::parse(value).is_some_and(|f| felt::canonical(&f) == value)
}

/// `^[a-z_][A-Za-z0-9_]{0,63}$`.
fn is_entrypoint(value: &str) -> bool {
    let bytes = value.as_bytes();
    matches!(bytes.first(), Some(b'a'..=b'z' | b'_'))
        && bytes.len() <= 64
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'_')
}

/// `isArgentMultisigCall`: canonical felts throughout, a non-zero contract, a plain entrypoint.
pub fn is_argent_multisig_call(call: &ArgentMultisigCall) -> bool {
    call.calldata.len() <= MAX_CALL_CALLDATA
        && call.calldata.iter().all(|value| is_canonical(value))
        && is_canonical(&call.contract_address)
        && is_non_zero_felt(&call.contract_address)
        && is_entrypoint(&call.entrypoint)
}

/// `isArgentMultisigCalls`: 1..=16 valid calls carrying at most 512 felts between them.
pub fn is_argent_multisig_calls(calls: &[ArgentMultisigCall]) -> bool {
    !calls.is_empty()
        && calls.len() <= MAX_CALLS
        && calls.iter().all(is_argent_multisig_call)
        && calls.iter().map(|call| call.calldata.len()).sum::<usize>() <= MAX_TOTAL_CALLDATA
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(entrypoint: &str, calldata: &[&str]) -> ArgentMultisigCall {
        ArgentMultisigCall {
            calldata: calldata.iter().map(|v| v.to_string()).collect(),
            contract_address: "0xabc".into(),
            entrypoint: entrypoint.into(),
        }
    }

    #[test]
    fn calls_are_canonical_and_bounded() {
        let ok = call("transfer", &["0x1"]);
        assert!(is_argent_multisig_call(&ok));
        for bad in [
            call("transfer", &["0x01"]),
            call("transfer", &["0xA"]),
            call("Transfer", &[]),
            call("1transfer", &[]),
            call("trans-fer", &[]),
            call(&"a".repeat(65), &[]),
            ArgentMultisigCall {
                contract_address: "0x0".into(),
                ..ok.clone()
            },
            ArgentMultisigCall {
                contract_address: "0x0abc".into(),
                ..ok.clone()
            },
        ] {
            assert!(!is_argent_multisig_call(&bad), "{bad:?}");
        }
        assert!(is_argent_multisig_call(&call(&"_".repeat(64), &[])));
        let wide = ArgentMultisigCall {
            calldata: vec!["0x1".into(); MAX_CALL_CALLDATA + 1],
            ..ok.clone()
        };
        assert!(!is_argent_multisig_call(&wide));
        assert!(!is_argent_multisig_calls(&[]));
        assert!(!is_argent_multisig_calls(&vec![ok.clone(); MAX_CALLS + 1]));
        assert!(is_argent_multisig_calls(&vec![ok.clone(); MAX_CALLS]));
        let half = ArgentMultisigCall {
            calldata: vec!["0x1".into(); MAX_CALL_CALLDATA],
            ..ok
        };
        assert!(is_argent_multisig_calls(&[half.clone(), half.clone()]));
        let mut over = half.clone();
        over.calldata.push("0x1".into());
        assert!(!is_argent_multisig_calls(&[half, over]));
    }
}
