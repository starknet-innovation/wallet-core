//! Ported from the extension's `tests/background/ledger/ledger-braavos-activity.test.ts`: the
//! verdict `ledgerStoreRefusal` draws from `readLedgerAccountState`.

use super::*;

const NOT_FOUND: &str =
    r#"{"jsonrpc":"2.0","id":0,"error":{"code":20,"message":"Contract not found"}}"#;
const DEPLOYED: &str = r#"{"jsonrpc":"2.0","id":0,"result":"0x1"}"#;
const DOWN: &str = "";

fn balance(id: u64, low: &str) -> Response {
    Response {
        id,
        body: format!(r#"{{"jsonrpc":"2.0","id":{id},"result":["{low}","0x0"]}}"#),
    }
}

fn class(body: &str) -> Response {
    Response {
        id: 0,
        body: body.into(),
    }
}

#[test]
fn the_reads_are_the_class_then_each_balance_of_the_address() {
    let address = Felt::from(0xb0a_u64);
    let tokens = [Felt::from(1_u64), Felt::from(2_u64)];
    let requests = requests(&address, &tokens).unwrap();
    let ids: Vec<u64> = requests.iter().map(|r| r.id).collect();
    assert_eq!(ids, [0, 1, 2]);
    assert!(requests[0].body.contains("starknet_getClassHashAt"));
    assert!(requests[0].body.contains(&felt::canonical(&address)));
    assert!(requests[2].body.contains("starknet_call"));
    assert!(requests[2].body.contains(&felt::canonical(&tokens[1])));
}

#[test]
fn a_never_used_address_is_unused() {
    assert_eq!(
        judge(2, &[class(NOT_FOUND), balance(1, "0x0"), balance(2, "0x0")]),
        Activity::Unused
    );
}

#[test]
fn an_unreadable_chain_is_unknown() {
    assert_eq!(judge(1, &[class(DOWN)]), Activity::Unknown);
    assert_eq!(judge(1, &[]), Activity::Unknown);
}

#[test]
fn a_half_read_chain_is_unknown_not_unused() {
    assert_eq!(judge(1, &[class(NOT_FOUND)]), Activity::Unknown);
    assert_eq!(
        judge(1, &[class(DOWN), balance(1, "0x0")]),
        Activity::Unknown
    );
}

#[test]
fn a_deployed_address_is_active() {
    assert_eq!(
        judge(1, &[class(DEPLOYED), balance(1, "0x0")]),
        Activity::Active
    );
}

#[test]
fn a_held_balance_alone_proves_use() {
    // Deployment unreadable, but a held balance proves the address is used.
    assert_eq!(
        judge(2, &[class(DOWN), balance(2, "0x1")]),
        Activity::Active
    );
}

#[test]
fn with_no_tokens_an_undeployed_address_is_unused() {
    assert_eq!(judge(0, &[class(NOT_FOUND)]), Activity::Unused);
}
