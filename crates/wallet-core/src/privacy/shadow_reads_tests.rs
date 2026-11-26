use super::*;

const LIMITS: ShadowScanLimits = ShadowScanLimits {
    nonce_limit: u64::MAX,
    max_scan_range: 1024,
};

fn felts(values: &[u64]) -> Vec<Felt> {
    values.iter().copied().map(Felt::from).collect()
}

fn reply(id: u64, result: &[&str]) -> String {
    serde_json::json!({"id": id, "jsonrpc": "2.0", "result": result}).to_string()
}

#[test]
fn windows_are_contiguous_sorted_distinct_and_bounded() {
    assert_eq!(
        nonce_windows(&felts(&[5, 0, 2, 1, 2]), &LIMITS),
        Ok(vec![(0, 3), (5, 6)])
    );
    assert_eq!(nonce_windows(&[], &LIMITS), Ok(vec![]));
    let small = ShadowScanLimits {
        nonce_limit: 100,
        max_scan_range: 2,
    };
    assert_eq!(
        nonce_windows(&felts(&[0, 1, 2, 3, 4]), &small),
        Ok(vec![(0, 2), (2, 4), (4, 5)])
    );
    // At or past the limit (or past u64), nothing is asked.
    for refused in [felts(&[100]), vec![Felt::from(u128::from(u64::MAX) + 1)]] {
        assert_eq!(
            nonce_windows(&refused, &small),
            Err(ShadowReadRefusal::NonceOutOfRange)
        );
    }
    assert_eq!(
        nonce_windows(&felts(&[u64::MAX - 1]), &LIMITS),
        Ok(vec![(u64::MAX - 1, u64::MAX)])
    );
}

#[test]
fn a_well_formed_reply_is_read_in_order() {
    let body = reply(3, &["0x2", "0x7", "0xa", "0x1", "0x8", "0xb", "0x0"]);
    assert_eq!(
        judge_shadow_accounts(&body, 3, (7, 9)),
        Ok(vec![
            ShadowAccountInfo {
                nonce: 7,
                address: Felt::from(10u8),
                deployed: true
            },
            ShadowAccountInfo {
                nonce: 8,
                address: Felt::from(11u8),
                deployed: false
            },
        ])
    );
}

#[test]
fn every_malformed_reply_is_refused() {
    let malformed = Err(ShadowReadRefusal::Malformed);
    for result in [
        // A length that is not the window's, or entries that do not match it.
        &["0x2", "0x7", "0xa", "0x1"][..],
        &["0x1", "0x7", "0xa", "0x1", "0x8", "0xb", "0x0"],
        &["0x2", "0x7", "0xa", "0x1", "0x8", "0xb"],
        &["0x2", "0x7", "0xa", "0x1", "0x8", "0xb", "0x0", "0x0"],
        &[],
        // Nonces out of sequence.
        &["0x2", "0x8", "0xa", "0x1", "0x7", "0xb", "0x0"],
        &["0x2", "0x7", "0xa", "0x1", "0x7", "0xb", "0x0"],
        // A zero address.
        &["0x2", "0x7", "0x0", "0x1", "0x8", "0xb", "0x0"],
        // A flag that is not 0 or 1.
        &["0x2", "0x7", "0xa", "0x2", "0x8", "0xb", "0x0"],
        // Not a felt.
        &["0x2", "0x7", "zz", "0x1", "0x8", "0xb", "0x0"],
    ] {
        assert_eq!(
            judge_shadow_accounts(&reply(3, result), 3, (7, 9)),
            malformed,
            "{result:?}"
        );
    }
    // Another request's reply, or not JSON-RPC.
    let good = reply(3, &["0x1", "0x7", "0xa", "0x1"]);
    assert_eq!(judge_shadow_accounts(&good, 4, (7, 8)), malformed);
    assert_eq!(judge_shadow_accounts("[]", 3, (7, 8)), malformed);
    // A node error.
    let error = r#"{"id":3,"jsonrpc":"2.0","error":{"code":40,"message":"x"}}"#;
    assert_eq!(
        judge_shadow_accounts(error, 3, (7, 8)),
        Err(ShadowReadRefusal::Unavailable)
    );
}

#[test]
fn the_body_calls_the_view_at_latest() {
    let body: serde_json::Value = serde_json::from_str(&get_shadow_accounts_body(
        9,
        &Felt::from(0xa11u64),
        &Felt::from(0xc0u64),
        (5, 6),
    ))
    .unwrap();
    assert_eq!(
        body,
        serde_json::json!({
            "id": 9, "jsonrpc": "2.0", "method": "starknet_call",
            "params": {"block_id": "latest", "request": {
                "contract_address": "0xa11",
                "entry_point_selector": crate::felt::canonical(&selector(GET_SHADOW_ACCOUNTS)),
                "calldata": ["0xc0", "0x5", "0x6", "0x0"],
            }},
        })
    );
}
