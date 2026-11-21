//! P7: the reader accepts every collection of the extension's schema v13, operates only the
//! records mobile holds keys for, and writes back unchanged everything it keeps unused. The vault
//! is built in test code over G4's payload (`vault_v13_support`); G4's own fixture test
//! (`vault_v13.rs`) is untouched. A kept record is one mobile holds no usable key for: a Ledger
//! account, an account derived from an imported phrase, one on the extension's devnet.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

mod common;
#[path = "vault_v13_support/mod.rs"]
mod support;

use std::sync::Arc;

use bramble_core::catalogs::networks;
use bramble_core::testing::{open_vault, seal_container_vault};
use bramble_core::{CoreError, NetworkId, PrivacyError, SendError, TransferRequest, WalletCore};
use common::{core, PASSWORD};
use serde_json::{json, Value};
use support::{full, g4, kept_ids, opens, round_trip, text};

/// A core over a fresh directory holding `payload`, sealed and unlocked.
fn unlocked(payload: &Value) -> (tempfile::TempDir, Arc<WalletCore>) {
    let (dir, _clock, core) = core();
    let path = dir.path().to_string_lossy().to_string();
    seal_container_vault(&path, PASSWORD, 13, &payload.to_string()).unwrap();
    core.unlock(PASSWORD.into()).unwrap();
    (dir, core)
}

fn on_disk(dir: &tempfile::TempDir) -> Value {
    serde_json::from_str(&open_vault(&dir.path().to_string_lossy(), PASSWORD).unwrap()).unwrap()
}

fn ids(values: &Value) -> Vec<String> {
    values
        .as_array()
        .unwrap()
        .iter()
        .map(|account| text(&account["id"]))
        .collect()
}

/// `payload` without the record `id` and its key, and with `active` as its active account.
fn without(payload: &Value, id: &str, active: &Value) -> Value {
    let mut payload = payload.clone();
    for (collection, field) in [("accounts", "id"), ("signingKeys", "accountId")] {
        payload[collection]
            .as_array_mut()
            .unwrap()
            .retain(|entry| entry[field] != id);
    }
    payload["activeAccountId"] = active.clone();
    payload
}

#[test]
fn every_collection_opens_and_writes_back_unchanged() {
    let full = full();
    assert_eq!(round_trip(&full).unwrap(), full);
    // The extension's own key order survives, whatever it is (no writer of the extension
    // produces another than account order, but its validator does not require one).
    let mut reordered = full.clone();
    reordered["signingKeys"].as_array_mut().unwrap().reverse();
    assert_eq!(round_trip(&reordered).unwrap(), reordered);
    // A kept record may be the active one.
    let mut ledger_active = full.clone();
    ledger_active["activeAccountId"] = kept_ids(&full).ledger.into();
    assert_eq!(round_trip(&ledger_active).unwrap(), ledger_active);
    // G4 with no kept record still round-trips as before.
    assert_eq!(round_trip(&g4()).unwrap(), g4());
}

/// No list, selection, signature, send or privacy read reaches a record mobile holds no key for.
#[test]
fn kept_records_are_never_listed_selected_or_signed() {
    let full = full();
    let kept = kept_ids(&full);
    let (_dir, core) = unlocked(&full);
    let listed: Vec<String> = core
        .all_account_records()
        .unwrap()
        .into_iter()
        .map(|record| record.id)
        .collect();
    assert_eq!(listed, ids(&g4()["accounts"]));
    let token = networks::info(NetworkId::Sepolia).tokens[0].address.clone();
    let recipient = text(&g4()["accounts"][0]["address"]);
    for id in [&kept.ledger, &kept.seeded, &kept.devnet] {
        assert!(core.select_account(id.clone()).is_err(), "{id}");
        assert!(matches!(
            core.sign_hash_for_account(id.clone(), "0x1".into()),
            Err(CoreError::InvalidInput { .. })
        ));
        let transfer = core.clone().prepare_transfer(TransferRequest {
            network: NetworkId::Sepolia,
            account_id: id.clone(),
            token_address: token.clone(),
            recipient: recipient.clone(),
            amount: "1".into(),
            fee_balance: None,
            tracked_tokens: vec![],
        });
        assert!(matches!(transfer, Err(SendError::UnknownAccount)), "{id}");
        assert!(matches!(
            core.clone().prepare_deployment(id.clone()),
            Err(SendError::UnknownAccount)
        ));
        assert!(matches!(
            core.clone().prepare_privacy_read(id.clone()),
            Err(PrivacyError::UnknownAccount)
        ));
    }
}

/// A write (here, adding an account) puts every kept record and collection back, and a new
/// record never takes a kept record's id: Sepolia's next OpenZeppelin index (1) is the address
/// of the kept imported-phrase record, so it is skipped as an imported key's would be.
#[test]
fn a_write_keeps_everything_kept_and_takes_no_kept_id() {
    let full = full();
    let kept = kept_ids(&full);
    let (dir, core) = unlocked(&full);
    let added = core.add_regular_account(NetworkId::Sepolia).unwrap();
    assert_eq!(added.account_index, 2);
    assert_ne!(added.id, kept.seeded);
    let written = on_disk(&dir);
    assert_eq!(written["activeAccountId"], json!(added.id));
    assert_eq!(without(&written, &added.id, &full["activeAccountId"]), full);
}

/// The vault's active account may be a kept one: then no account is active here until the
/// person picks one, and the network switch picks the first visible one there.
#[test]
fn an_active_kept_record_leaves_no_account_active() {
    let mut full = full();
    let kept = kept_ids(&full);
    full["activeAccountId"] = kept.ledger.clone().into();
    let (dir, core) = unlocked(&full);
    assert_eq!(core.active_account().unwrap(), None);
    let picked = core.ensure_network_account(NetworkId::Sepolia).unwrap();
    assert_eq!(picked.unwrap().id, text(&g4()["accounts"][5]["id"]));
    let written = on_disk(&dir);
    let mut expected = full.clone();
    expected["activeAccountId"] = g4()["accounts"][5]["id"].clone();
    assert_eq!(written, expected);
}

/// A vault holding a phrase and only kept records waits for discovery, and restoring keeps them.
#[test]
fn a_vault_of_kept_records_only_waits_for_discovery_and_keeps_them() {
    let full = full();
    let kept = kept_ids(&full);
    let mut only = full.clone();
    for (collection, field) in [("accounts", "id"), ("signingKeys", "accountId")] {
        only[collection]
            .as_array_mut()
            .unwrap()
            .retain(|entry| entry[field] == kept.ledger);
    }
    only["activeAccountId"] = kept.ledger.clone().into();
    only["originGrants"] = json!([{ "accountIds": [kept.ledger], "origin": support::ORIGIN }]);
    only["shadowAccountScopes"]
        .as_array_mut()
        .unwrap()
        .remove(0);
    assert_eq!(round_trip(&only).unwrap(), only);
    let (dir, core) = unlocked(&only);
    assert!(core.recovery_pending().unwrap());
    let restored = core
        .restore_recovered_accounts(NetworkId::Sepolia, vec![common::oz(0)])
        .unwrap();
    let written = on_disk(&dir);
    assert_eq!(
        without(&written, &restored[0].id, &only["activeAccountId"]),
        only
    );
}

/// `isJson`'s budget (`vault-capacity.test.ts`: "is the shared JSON budget"): the extension
/// refuses a vault past it on read and on write, so mobile does both. Imported phrases fill G4
/// up to the last one the budget admits; one more is refused, and so is a write that would pass
/// it, which leaves the vault on disk as it was.
#[test]
fn the_budget_bounds_every_open_and_every_write() {
    let with_seeds = |count: usize| {
        let mut payload = g4();
        let mnemonic = support::fixture()["mnemonic"].clone();
        payload["importedSeeds"] = (1..=count)
            .map(|n| json!({ "id": format!("phrase-{n}"), "mnemonic": mnemonic }))
            .collect();
        payload
    };
    let (mut fits, mut over) = (1, 4_000);
    assert!(opens(&with_seeds(fits)) && !opens(&with_seeds(over)));
    while over - fits > 1 {
        let middle = (fits + over) / 2;
        if opens(&with_seeds(middle)) {
            fits = middle;
        } else {
            over = middle;
        }
    }
    let at_capacity = with_seeds(fits);
    assert_eq!(round_trip(&at_capacity).unwrap(), at_capacity);
    let (dir, core) = unlocked(&at_capacity);
    assert!(matches!(
        core.add_regular_account(NetworkId::Mainnet),
        Err(CoreError::Storage { reason }) if reason == "vault storage limit reached"
    ));
    assert_eq!(on_disk(&dir), at_capacity);
    assert_eq!(core.all_account_records().unwrap().len(), 6);
}
