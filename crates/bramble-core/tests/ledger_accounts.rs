//! P8: a Ledger record the vault holds is operated. It is listed and flagged, signs nothing with
//! a key, and a send from it hands its hash to the device. The vault is G4 with its Sepolia
//! OpenZeppelin account 0 held as a Ledger account (Bramble's scheme, index 0, no key, no
//! y-coordinate), the shape the extension's `addLedgerAccount` writes.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

mod send_support;
#[path = "vault_v13_support/mod.rs"]
mod support;

use std::sync::Arc;

use bramble_core::testing::{open_vault, seal_container_vault};
use bramble_core::{CoreError, SendStep, StepReply, WalletCore};
use send_support::{empty_core, reviewed, run_until, sepolia, ACCOUNT_1, PASSWORD};
use serde_json::{json, Value};
use support::g4;

const LEDGERW_0: &str = "m/2645'/1195502025'/734980059'/0'/0'/0";

/// G4 with its Sepolia OpenZeppelin account 0 as a Ledger record; its id.
fn with_ledger() -> (Value, String) {
    let mut payload = g4();
    let id = format!("SN_SEPOLIA:{}", send_support::ACCOUNT_0);
    let account = payload["accounts"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|account| account["id"] == id.as_str())
        .unwrap();
    account["derivation"] = json!({
        "accountIndex": 0, "eip2645application": "LedgerW", "kind": "ledger", "path": LEDGERW_0,
    });
    account.as_object_mut().unwrap().remove("publicKeyY");
    payload["signingKeys"]
        .as_array_mut()
        .unwrap()
        .retain(|key| key["accountId"] != id.as_str());
    (payload, id)
}

fn unlocked(payload: &Value) -> (tempfile::TempDir, Arc<WalletCore>) {
    let (dir, _clock, core) = empty_core();
    let path = dir.path().to_string_lossy().to_string();
    seal_container_vault(&path, PASSWORD, 13, &payload.to_string()).unwrap();
    core.unlock(PASSWORD.into()).unwrap();
    (dir, core)
}

#[test]
fn a_ledger_record_is_listed_flagged_and_written_back_keyless() {
    let (payload, id) = with_ledger();
    assert_eq!(support::round_trip(&payload).unwrap(), payload);
    let (dir, core) = unlocked(&payload);
    let records = core.all_account_records().unwrap();
    let ledger = records.iter().find(|record| record.id == id).unwrap();
    assert!(ledger.ledger && !ledger.imported);
    assert!(records.iter().filter(|record| record.ledger).count() == 1);
    assert!(core.unusable_accounts().unwrap().is_empty());
    // A write (selecting it) puts it back as it was: no key, no y-coordinate.
    core.select_account(id.clone()).unwrap();
    let written: Value =
        serde_json::from_str(&open_vault(&dir.path().to_string_lossy(), PASSWORD).unwrap())
            .unwrap();
    let record = written["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|account| account["id"] == id.as_str())
        .unwrap();
    assert_eq!(record["derivation"]["path"], LEDGERW_0);
    assert!(record.get("publicKeyY").is_none());
    assert!(written["signingKeys"]
        .as_array()
        .unwrap()
        .iter()
        .all(|key| key["accountId"] != id.as_str()));
}

#[test]
fn a_ledger_record_signs_nothing_with_a_key() {
    let (payload, id) = with_ledger();
    let (_dir, core) = unlocked(&payload);
    assert!(matches!(
        core.sign_hash_for_account(id, "0x1".into()),
        Err(CoreError::InvalidInput { .. })
    ));
}

#[test]
fn a_send_from_a_ledger_record_asks_its_device() {
    let (payload, _id) = with_ledger();
    let (_dir, core) = unlocked(&payload);
    let mut node = sepolia(true);
    let plan = reviewed(&core, &mut node, ACCOUNT_1);
    let step = plan.advance(StepReply::Confirm).unwrap();
    let step = run_until(&plan, &core, &mut node, step, send_support::never);
    // The Starknet app's version request: the first APDU of `DeviceSigning`.
    assert_eq!(
        step,
        SendStep::Device {
            apdu: vec![0x5a, 0x00, 0x00, 0x00, 0x00]
        }
    );
    // Nothing was written or recorded before the device signs.
    let id = format!("SN_SEPOLIA:{}", send_support::ACCOUNT_0);
    assert!(core.activity(id).unwrap().is_empty());
}
