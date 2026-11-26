//! Ported from the extension's `tests/provider/contract/provider-spec-shapes.test.ts` and
//! `tests/provider/bridge/provider-bridge-results-guards.test.ts` ("providerInvokeCalls calldata
//! arms"), through `read_request` as a bridge reaches it.

use starknet_types_core::felt::Felt;

use crate::dapp::request::tests::POLICY;
use crate::dapp::{read_request, Request, WalletRpcError};
use crate::send::calls::Call;

fn invoke(params: &str) -> Result<Request, WalletRpcError> {
    read_request("wallet_addInvokeTransaction", Some(params), &POLICY)
}

fn invalid(params: &str) -> bool {
    invoke(params) == Err(WalletRpcError::invalid_request_payload())
}

fn call(address: u64, entrypoint: &str, calldata: &[u64]) -> Call {
    Call {
        contract_address: Felt::from(address),
        entrypoint: entrypoint.into(),
        calldata: calldata.iter().copied().map(Felt::from).collect(),
    }
}

fn calls(params: &str) -> Vec<Call> {
    match invoke(params) {
        Ok(Request::AddInvoke { calls }) => calls,
        other => panic!("{params}: {other:?}"),
    }
}

const SPEC_CALL: &str =
    r#"{"calldata":["0x1"],"contract_address":"0x123","entry_point":"transfer"}"#;
const LEGACY_CALL: &str =
    r#"{"calldata":["0x1"],"contractAddress":"0x123","entrypoint":"transfer"}"#;

#[test]
fn spec_and_legacy_shapes() {
    let transfer = vec![call(0x123, "transfer", &[1])];
    assert_eq!(
        calls(&format!(r#"{{"invoke_transaction":[{SPEC_CALL}]}}"#)),
        transfer
    );
    assert_eq!(calls(&format!(r#"{{"calls":[{LEGACY_CALL}]}}"#)), transfer);
    // Either spelling of each key, mixed within one call.
    assert_eq!(
        calls(
            r#"{"calls":[{"calldata":["0x1"],"contract_address":"0x123","entrypoint":"transfer"}]}"#
        ),
        transfer
    );
    // No calldata, or a null one, is an empty list.
    assert_eq!(
        calls(r#"{"invoke_transaction":[{"contract_address":"0x9","entry_point":"ping"}]}"#),
        vec![call(9, "ping", &[])]
    );
    assert_eq!(
        calls(r#"{"calls":[{"calldata":null,"contractAddress":"0x9","entrypoint":"ping"}]}"#),
        vec![call(9, "ping", &[])]
    );
}

#[test]
fn malformed_and_both_shape_params_are_refused() {
    assert!(invalid(
        r#"{"invoke_transaction":[{"entry_point":"transfer"}]}"#
    ));
    assert!(invalid(&format!(
        r#"{{"calls":[{LEGACY_CALL}],"invoke_transaction":[{SPEC_CALL}]}}"#
    )));
    assert!(invalid(r#"{"calls":[]}"#));
    assert!(invalid(r#"{"calls":{}}"#));
    assert!(invalid(r#"{"calls":[42]}"#));
    assert!(invalid(r#"{"invoke_transaction":null}"#));
    assert!(invalid(&format!(r#"[{SPEC_CALL}]"#)));
    assert!(invalid(&format!(
        r#"{{"calls":[{LEGACY_CALL}],"extra":1}}"#
    )));
    assert_eq!(
        read_request("wallet_addInvokeTransaction", None, &POLICY),
        Err(WalletRpcError::invalid_request_payload())
    );
}

#[test]
fn call_fields_are_checked() {
    let one = |call: &str| invalid(&format!(r#"{{"calls":[{call}]}}"#));
    // Address: a non-zero felt in lowercase hex.
    assert!(one(r#"{"contractAddress":"0x0","entrypoint":"t"}"#));
    assert!(one(r#"{"contractAddress":"0xABC","entrypoint":"t"}"#));
    assert!(one(r#"{"contractAddress":"291","entrypoint":"t"}"#));
    assert!(one(r#"{"contractAddress":291,"entrypoint":"t"}"#));
    assert!(one(
        r#"{"contractAddress":"0x800000000000011000000000000000000000000000000000000000000000001","entrypoint":"t"}"#
    ));
    // Entrypoint: an identifier of at most 64 characters.
    assert!(one(r#"{"contractAddress":"0x1","entrypoint":"1abc"}"#));
    assert!(one(r#"{"contractAddress":"0x1","entrypoint":"a-b"}"#));
    assert!(one(r#"{"contractAddress":"0x1","entrypoint":""}"#));
    assert!(one(&format!(
        r#"{{"contractAddress":"0x1","entrypoint":"{}"}}"#,
        "a".repeat(65)
    )));
    assert!(!one(&format!(
        r#"{{"contractAddress":"0x1","entrypoint":"_{}"}}"#,
        "a".repeat(63)
    )));
    // A nested api_version or any other key.
    assert!(one(
        r#"{"contractAddress":"0x1","entrypoint":"t","api_version":"0.10.4"}"#
    ));
    // The snake key wins only when it is a string.
    assert_eq!(
        calls(r#"{"calls":[{"contract_address":7,"contractAddress":"0x1","entrypoint":"t"}]}"#),
        vec![call(1, "t", &[])]
    );
}

// "canonicalizes decimal calldata accepted by Starknet providers"
#[test]
fn calldata_reads_as_bigint_does() {
    let felts = |calldata: &str| {
        calls(&format!(
            r#"{{"calls":[{{"calldata":{calldata},"contractAddress":"0x123","entrypoint":"transfer"}}]}}"#
        ))
        .remove(0)
        .calldata
    };
    let rejects = |calldata: &str| {
        invalid(&format!(
            r#"{{"calls":[{{"calldata":{calldata},"contractAddress":"0x1","entrypoint":"t"}}]}}"#
        ))
    };
    assert_eq!(
        felts(r#"["1000","0"]"#),
        vec![Felt::from(1000u64), Felt::ZERO]
    );
    // `BigInt` reads upper-case hex, octal and binary, trims whitespace and reads "" as zero.
    assert_eq!(
        felts(r#"["0xABC"," 7 ","0o10","0b11",""]"#),
        [0xabc_u64, 7, 8, 3, 0].map(Felt::from).to_vec()
    );
    // Integer numbers, exactly as the double holds them.
    assert_eq!(
        felts("[5, 0, 1e3]"),
        [5u64, 0, 1000].map(Felt::from).to_vec()
    );
    assert_eq!(
        felts("[18446744073709551616]"),
        vec![Felt::from(u128::from(u64::MAX) + 1)]
    );
    assert_eq!(
        felts("[3.6185027886661311e75]"),
        vec![Felt::TWO.pow(251u32)]
    );
    for bad in [
        "[{}]",
        "[true]",
        "[null]",
        "[[1]]",
        "[1.5]",
        "[-1]",
        r#"["-1"]"#,
        r#"["0xg"]"#,
        r#"["1.5"]"#,
        // 2^251 is the last double below the prime.
        "[3.6185027886661317e75]",
        // The prime itself.
        r#"["0x800000000000011000000000000000000000000000000000000000000000001"]"#,
    ] {
        assert!(rejects(bad), "{bad}");
    }
    // A spelling longer than 78 characters, even of a small value.
    assert!(rejects(&format!(r#"["{}1"]"#, "0".repeat(78))));
    assert!(!rejects(&format!(r#"["{}1"]"#, "0".repeat(77))));
}

// "accepts the call cap, rejects one more, and rejects calldata past the array budget"
#[test]
fn call_and_calldata_caps() {
    let batch = |count: usize| {
        let call = r#"{"calldata":[],"contractAddress":"0x1","entrypoint":"t"}"#;
        format!(r#"{{"calls":[{}]}}"#, vec![call; count].join(","))
    };
    let limits = POLICY.invoke_limits;
    assert_eq!(calls(&batch(limits.calls)).len(), limits.calls);
    assert!(invalid(&batch(limits.calls + 1)));
    let calldata = |count: usize| {
        format!(
            r#"{{"calls":[{{"calldata":[{}],"contractAddress":"0x1","entrypoint":"t"}}]}}"#,
            vec![r#""0x1""#; count].join(",")
        )
    };
    assert_eq!(
        calls(&calldata(limits.calldata_felts))[0].calldata.len(),
        limits.calldata_felts
    );
    assert!(invalid(&calldata(limits.calldata_felts + 1)));
}

// "drops a well-formed api_version from both invoke shapes, with or without a proof", and
// "rejects a malformed, call-nested, or both-shape invoke api_version".
#[test]
fn api_version_beside_the_calls() {
    let transfer = vec![call(0x123, "transfer", &[1])];
    for version in ["0.10", "0.10.4", "0.10.5"] {
        assert_eq!(
            calls(&format!(
                r#"{{"api_version":"{version}","invoke_transaction":[{SPEC_CALL}]}}"#
            )),
            transfer
        );
        assert_eq!(
            calls(&format!(
                r#"{{"api_version":"{version}","calls":[{LEGACY_CALL}]}}"#
            )),
            transfer
        );
    }
    for bad in [
        r#""not.a.version.x""#,
        r#""""#,
        r#""v0.10.4""#,
        r#""0.10.4-rc.0""#,
        "10",
        "null",
    ] {
        assert!(invalid(&format!(
            r#"{{"api_version":{bad},"invoke_transaction":[{SPEC_CALL}]}}"#
        )));
    }
}

// "preserves a spec-conforming invoke proof": STRK20 is P15's, so a proof is refused for now,
// but only once the calls parsed; malformed calls beside a proof are still 114.
#[test]
fn a_proof_waits_for_strk20() {
    let proof = r#"{"data":"proof","output":["0x1"],"proof_facts":["0x2"]}"#;
    assert_eq!(
        invoke(&format!(
            r#"{{"invoke_transaction":[{SPEC_CALL}],"proof":{proof}}}"#
        )),
        Err(WalletRpcError::unknown_error())
    );
    assert!(invalid(&format!(
        r#"{{"invoke_transaction":[],"proof":{proof}}}"#
    )));
    // `proof: null` is not a proof: 114, as the extension answers it.
    assert!(invalid(&format!(
        r#"{{"invoke_transaction":[{SPEC_CALL}],"proof":null}}"#
    )));
}

// `invokeTxPayload`: the hash padded to PADDED_TXN_HASH.
#[test]
fn the_result_is_the_padded_hash() {
    let hash = Felt::from_hex_unchecked("0xabc");
    assert_eq!(
        crate::dapp::invoke::invoke_result(&hash).to_json(),
        format!(r#"{{"transaction_hash":"0x{:0>64}"}}"#, "abc")
    );
}
