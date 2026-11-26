use serde_json::{json, Value};

use super::*;
use crate::send::calls::{execute_calldata, Call};

pub(crate) const POOL: &str = "0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a";

fn f(hex: &str) -> Felt {
    Felt::from_hex(hex).unwrap()
}

fn secrets(values: &[Felt]) -> Vec<SecretFelt> {
    values.iter().copied().map(SecretFelt::new).collect()
}

/// The invocation the pinned SDK built in `sdk_invocation_json` below.
pub(crate) fn sdk_invocation() -> ProofInvocation {
    ProofInvocation::new(
        f(POOL),
        &secrets(&[f("0xabc"), f("0xf00"), Felt::ZERO]),
        f("0x2a"),
        vec![f("0x1234ab"), Felt::from(5678u64)],
    )
}

/// Observed from the pinned SDK (`PRIVACY-0.14.3-RC.8`, starknet.js 10.5.0) under bun:
/// `new ProofInvocationFactory().create({address: "0xabc", viewingKey: "0x0f00", signer}, POOL,
/// [], {...getDefaultProofDetails(SN_SEPOLIA), nonce: 42n})`, the stub signer returning
/// `["0x1234ab", "5678"]`, then `JSON.stringify` of the result.
pub(crate) const SDK_INVOCATION: &str = concat!(
    r#"{"type":"INVOKE","sender_address":"0x40337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a","#,
    r#""calldata":["0x1","0x40337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a","#,
    r#""0x360f8727b971d0bc6b93fc840d637c077f8ae59eb6ca8ce27fdb5422b688192","0x3","0xabc","0xf00","0x0"],"#,
    r#""signature":["0x1234ab","0x162e"],"nonce":"0x2a","#,
    r#""resource_bounds":{"l1_gas":{"max_amount":"0x1","max_price_per_unit":"0x0"},"#,
    r#""l2_gas":{"max_amount":"0x5f5e100","max_price_per_unit":"0x0"},"#,
    r#""l1_data_gas":{"max_amount":"0x1","max_price_per_unit":"0x0"}},"#,
    r#""tip":"0x0","paymaster_data":[],"account_deployment_data":[],"#,
    r#""nonce_data_availability_mode":"L1","fee_data_availability_mode":"L1","version":"0x3"}"#,
);

#[test]
fn sdk_invocation_json() {
    let json = sdk_invocation().to_json();
    assert_eq!(std::str::from_utf8(&json).unwrap(), SDK_INVOCATION);
}

/// `compileExecuteCalldata("0x7", ["0x1", "2", "0x0"])` under the pinned SDK returned
/// `["1","7","1528…194","3","1","2","0"]`: the same felts as `calls::execute_calldata` of the
/// one `compile_actions` call.
#[test]
fn execute_calldata_is_the_sdks_layout() {
    let inner = [Felt::ONE, Felt::TWO, Felt::ZERO];
    let invocation = ProofInvocation::new(Felt::from(7u64), &secrets(&inner), Felt::ZERO, vec![]);
    let ours: Vec<Felt> = invocation
        .calldata()
        .iter()
        .map(|value| *value.expose_secret())
        .collect();
    let sdk: Vec<Felt> = [
        "1",
        "7",
        "1528270583512024523147690760965477538713896997546369339541288693599081890194",
        "3",
        "1",
        "2",
        "0",
    ]
    .iter()
    .map(|value| Felt::from_dec_str(value).unwrap())
    .collect();
    assert_eq!(ours, sdk);
    let call = Call {
        contract_address: Felt::from(7u64),
        entrypoint: COMPILE_ACTIONS.into(),
        calldata: inner.to_vec(),
    };
    assert_eq!(ours, execute_calldata(&[call]));
    // An empty `compile_actions` is still one call with a zero length.
    let empty = ProofInvocation::new(Felt::from(7u64), &[], Felt::ZERO, vec![]);
    assert_eq!(empty.calldata().len(), 4);
    assert!(std::str::from_utf8(&empty.to_json())
        .unwrap()
        .contains(r#""calldata":["0x1","0x7","0x360f8727b971d0bc6b93fc840d637c077f8ae59eb6ca8ce27fdb5422b688192","0x0"],"signature":[],"#));
}

#[test]
fn hex_is_the_canonical_spelling() {
    let values = [
        Felt::ZERO,
        Felt::ONE,
        Felt::from(0xfu64),
        Felt::from(0x10u64),
        Felt::from(0xabcdefu64),
        Felt::from(u64::MAX),
        f("0x800000000000011000000000000000000000000000000000000000000000000"),
        Felt::MAX,
    ];
    for value in values {
        let mut out = Vec::new();
        push_hex(&mut out, &value);
        assert_eq!(String::from_utf8(out).unwrap(), felt::canonical(&value));
        assert_eq!(hex_digits(&value) + 2, felt::canonical(&value).len());
    }
}

#[test]
fn json_is_sized_exactly_so_the_key_is_never_reallocated() {
    let calldata = vec![Felt::MAX; 300];
    let invocation = ProofInvocation::new(
        Felt::MAX,
        &secrets(&calldata),
        Felt::MAX,
        vec![Felt::MAX; 3],
    );
    let json = invocation.to_json();
    assert_eq!(json.len(), json.capacity());
    let parsed: Value = serde_json::from_slice(&json).unwrap();
    assert_eq!(parsed["calldata"].as_array().unwrap().len(), 304);
    let wrapped = invocation.json_within("[", "]");
    assert_eq!(wrapped.len(), wrapped.capacity());
    assert_eq!(&wrapped[1..wrapped.len() - 1], &json[..]);
}

#[test]
fn debug_never_shows_the_calldata() {
    let invocation = ProofInvocation::new(
        f(POOL),
        &secrets(&[f("0xabc"), f("0x5ec7e7"), Felt::ZERO]),
        Felt::ZERO,
        vec![],
    );
    let debug = format!("{invocation:?}");
    assert!(debug.contains("[redacted 7 felts]"), "{debug}");
    assert!(!debug.contains("5ec7e7"), "{debug}");
    assert!(!debug.to_lowercase().contains("5ec7e7"), "{debug}");
    // `Felt`'s `Debug` is decimal.
    assert!(
        !debug.contains(&Felt::from(0x5ec7e7u64).to_string()),
        "{debug}"
    );
}

/// starknet.js 10.5.0 `RpcProvider.getNonceForAddress(POOL, "latest")`, observed under bun with
/// a recording `fetch`, sent `{"id":1,"jsonrpc":"2.0","method":"starknet_getNonce","params":
/// {"contract_address":"0x40337…12a","block_id":"latest"}}`. `rpc_bodies` writes the same
/// members in sorted key order; the node reads JSON, and the body is public.
#[test]
fn pool_nonce_request_is_starknet_js_body() {
    let body: Value = serde_json::from_str(&pool_nonce_request(1, &f(POOL))).unwrap();
    assert_eq!(
        body,
        json!({
            "id": 1,
            "jsonrpc": "2.0",
            "method": "starknet_getNonce",
            "params": {
                "contract_address": "0x40337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a",
                "block_id": "latest",
            },
        })
    );
}

#[test]
fn pool_nonce_needs_a_felt_result() {
    let reply = |result: &str| format!(r#"{{"jsonrpc":"2.0","id":3,"result":{result}}}"#);
    assert_eq!(pool_nonce(&reply(r#""0x2a""#), 3), Some(f("0x2a")));
    assert_eq!(pool_nonce(&reply(r#""0x0""#), 3), Some(Felt::ZERO));
    assert_eq!(pool_nonce(&reply(r#""0x2a""#), 4), None, "another request");
    for refused in [r#""42""#, "42", "null", r#"["0x2a"]"#, r#""0xZZ""#] {
        assert_eq!(pool_nonce(&reply(refused), 3), None, "{refused}");
    }
    // `getNonceSafe` would read this as 0; the pool's nonce is never guessed.
    let error = r#"{"jsonrpc":"2.0","id":3,"error":{"code":20,"message":"Contract not found"}}"#;
    assert_eq!(pool_nonce(error, 3), None);
    assert_eq!(pool_nonce("not json", 3), None);
}
