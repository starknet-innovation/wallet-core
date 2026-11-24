//! The journal row's life: filed under the message hash, never observed, guarding until its
//! deadline, settled into one transaction row.

use super::*;
use crate::activity::store::Entry;
use crate::host::testing::Memory;
use crate::send::judge::{Observation, TxStatus};
use serde_json::{json, Value};

const SCOPE: &str = "SN_MAIN:0x5dccf9cfdcabb9e4ef45814bda1b056bb23063d0e9b64e5a845e9dfb3d23f10";
const MESSAGE: &str = "0x5f4b514c262716a56a770a9ca110b68f17eb045f4b01d1d2cec8ebaf90a6f47";
const BEFORE: u64 = 1_790_000_600;

fn journal(relay: Value) -> Entry {
    serde_json::from_value(json!({
        "accountId": SCOPE,
        "hash": MESSAGE,
        "kind": "transfer",
        "networkId": "SN_MAIN",
        "origin": "wallet",
        "relay": relay,
        "status": "pending",
        "submittedAt": 1,
        "summary": "Send 1 ETH (network fee 0.001 USDC)",
    }))
    .unwrap()
}

fn live() -> Entry {
    journal(json!({"executeBefore": BEFORE, "nonce": "0x5"}))
}

fn felt_of(text: &str) -> Felt {
    felt::parse(text).unwrap()
}

#[test]
fn a_journal_row_guards_until_its_deadline_and_is_never_observed() {
    let storage = Memory::default();
    let mut log = ActivityLog::new(&storage);
    assert!(log.record(live()));
    assert!(log.relay_pending(SCOPE, BEFORE - 1));
    assert!(!log.relay_pending(SCOPE, BEFORE));
    let missing = Observation::NotFound;
    for _ in 0..5 {
        assert_eq!(log.observe(SCOPE, &felt_of(MESSAGE), missing), None);
    }
    let row = &log.list(SCOPE)[0];
    assert_eq!(
        (row.status.as_str(), row.not_found_count),
        ("pending", None)
    );
}

#[test]
fn the_relays_answer_settles_the_journal_into_one_transaction_row() {
    let storage = Memory::default();
    let mut log = ActivityLog::new(&storage);
    log.record(live());
    assert!(!log.settle_relay(SCOPE, &felt_of("0x1"), &felt_of("0x7a1ce5")));
    assert!(log.settle_relay(SCOPE, &felt_of(MESSAGE), &felt_of("0x7a1ce5")));
    let rows = log.list(SCOPE);
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0].hash.as_str(), rows[0].relay.is_none()),
        ("0x7a1ce5", true)
    );
    assert!(!log.relay_pending(SCOPE, 0));
    assert!(!log.settle_relay(SCOPE, &felt_of(MESSAGE), &felt_of("0x7a1ce5")));
    // Now an ordinary row: the status read reaches it.
    let found = Observation::Found {
        status: TxStatus::Accepted,
        accepted_finality: false,
        finalized_on_l1: false,
    };
    assert_eq!(
        log.observe(SCOPE, &felt_of("0x7a1ce5"), found),
        Some(TxStatus::Accepted)
    );
}

#[test]
fn only_a_well_formed_relay_is_stored() {
    for relay in [
        json!({"executeBefore": BEFORE, "nonce": "0x05"}),
        json!({"executeBefore": BEFORE, "nonce": "5"}),
        json!({"executeBefore": -1, "nonce": "0x5"}),
        json!({"executeBefore": 9_007_199_254_740_992u64, "nonce": "0x5"}),
        json!({"executeBefore": BEFORE, "nonce": "0x5", "messageHash": MESSAGE}),
        json!({"nonce": "0x5"}),
    ] {
        let storage = Memory::default();
        let mut log = ActivityLog::new(&storage);
        assert!(!log.record(journal(relay.clone())), "{relay}");
    }
}
