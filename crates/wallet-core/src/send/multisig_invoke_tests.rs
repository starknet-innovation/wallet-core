//! `isArgentMultisigCalls`: canonical felts, a non-zero canonical contract, a plain entrypoint
//! name, and the request budgets.

use super::*;

fn is_call(call: &MultisigCall) -> bool {
    is_calls(std::slice::from_ref(call))
}

fn call(entrypoint: &str, calldata: &[&str]) -> MultisigCall {
    MultisigCall {
        calldata: calldata.iter().map(|v| v.to_string()).collect(),
        contract_address: "0xabc".into(),
        entrypoint: entrypoint.into(),
    }
}

#[test]
fn calls_are_canonical_and_bounded() {
    let ok = call("transfer", &["0x1"]);
    assert!(is_call(&ok));
    for bad in [
        call("transfer", &["0x01"]),
        call("transfer", &["0xA"]),
        call("Transfer", &[]),
        call("1transfer", &[]),
        call("trans-fer", &[]),
        call(&"a".repeat(65), &[]),
        MultisigCall {
            contract_address: "0x0".into(),
            ..ok.clone()
        },
        MultisigCall {
            contract_address: "0x0abc".into(),
            ..ok.clone()
        },
    ] {
        assert!(!is_call(&bad), "{bad:?}");
    }
    assert!(is_call(&call(&"_".repeat(64), &[])));
    let wide = MultisigCall {
        calldata: vec!["0x1".into(); MAX_CALL_CALLDATA_FELTS + 1],
        ..ok.clone()
    };
    assert!(!is_call(&wide));
    assert!(!is_calls(&[]));
    assert!(!is_calls(&vec![ok.clone(); MAX_CALLS + 1]));
    assert!(is_calls(&vec![ok.clone(); MAX_CALLS]));
    let half = MultisigCall {
        calldata: vec!["0x1".into(); MAX_CALL_CALLDATA_FELTS],
        ..ok
    };
    assert!(is_calls(&[half.clone(), half.clone()]));
    let mut over = half.clone();
    over.calldata.push("0x1".into());
    assert!(!is_calls(&[half, over]));
}
