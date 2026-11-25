//! P8: a Ledger record the vault holds is operated. It is listed and flagged, signs nothing with
//! a key, and a send from it hands its hash to the device. The vault is G4 with its Sepolia
//! OpenZeppelin account 0 held as a Ledger account (Bramble's scheme, index 0, no key, no
//! y-coordinate), the shape the extension's `addLedgerAccount` writes.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

mod send_support;
#[path = "vault_v13_support/mod.rs"]
mod support;

use std::sync::Arc;

use bramble_core::ledger::ledger_account_path;
use bramble_core::testing::{open_vault, seal_container_vault};
use bramble_core::{
    CoreError, NetworkId, PrivacyError, SendStep, StepReply, UnusableReason, WalletCore,
};
use send_support::{empty_core, reviewed, run_until, sepolia, ACCOUNT_1, PASSWORD};
use serde_json::{json, Value};
use starknet_wallet_core::ledger::{app_version_request, public_key_request};
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
    // The Starknet app's version, then the key at the record's own path: Bramble's scheme at
    // index 0, so the device asked is the one that owns the address.
    let SendStep::Device { apdu } = step else {
        panic!("expected the device's first APDU, got {step:?}");
    };
    assert_eq!(apdu, app_version_request());
    let step = plan
        .advance(StepReply::Device {
            reply: vec![2, 4, 0, 0x90, 0x00],
        })
        .unwrap();
    let path = ledger_account_path("LedgerW", 0).unwrap();
    assert_eq!(
        step,
        SendStep::Device {
            apdu: public_key_request(&path)
        }
    );
    // Nothing was written or recorded before the device signs.
    let id = format!("SN_SEPOLIA:{}", send_support::ACCOUNT_0);
    assert!(core.activity(id).unwrap().is_empty());
}

/// `payload` with every account on `network` and its key removed.
fn without_network(payload: &mut Value, network: &str) {
    let gone: Vec<Value> = payload["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|account| account["networkId"] == network)
        .map(|account| account["id"].clone())
        .collect();
    payload["accounts"]
        .as_array_mut()
        .unwrap()
        .retain(|account| !gone.contains(&account["id"]));
    payload["signingKeys"]
        .as_array_mut()
        .unwrap()
        .retain(|key| !gone.contains(&key["accountId"]));
}

/// The extension's `deriveLedgerAccount`: switching network with a Ledger account active places
/// the same device account there, its public key carried over and no key sealed.
#[test]
fn a_ledger_account_follows_a_network_switch() {
    let (mut payload, id) = with_ledger();
    without_network(&mut payload, "SN_MAIN");
    payload["activeAccountId"] = id.clone().into();
    let (dir, core) = unlocked(&payload);
    let projected = core
        .ensure_network_account(NetworkId::Mainnet)
        .unwrap()
        .unwrap();
    assert_eq!(projected.id, format!("SN_MAIN:{}", send_support::ACCOUNT_0));
    assert!(projected.ledger);
    let written: Value =
        serde_json::from_str(&open_vault(&dir.path().to_string_lossy(), PASSWORD).unwrap())
            .unwrap();
    let record = written["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|account| account["id"] == projected.id.as_str())
        .unwrap();
    assert_eq!(record["derivation"]["path"], LEDGERW_0);
    assert!(written["signingKeys"]
        .as_array()
        .unwrap()
        .iter()
        .all(|key| key["accountId"] != projected.id.as_str()));
}

#[test]
fn a_privacy_read_from_a_ledger_account_is_refused_as_such() {
    let (payload, id) = with_ledger();
    let (_dir, core) = unlocked(&payload);
    assert!(matches!(
        core.prepare_privacy_read(id),
        Err(PrivacyError::LedgerAccount)
    ));
}

/// A Ledger record its stored key does not place is kept, listed as a Ledger account mobile does
/// not operate, and written back as it was.
#[test]
fn a_ledger_record_its_key_does_not_place_is_kept() {
    let (mut payload, id) = with_ledger();
    let other = payload["accounts"][3]["publicKeyX"].clone();
    let record = payload["accounts"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|account| account["id"] == id.as_str())
        .unwrap();
    assert_ne!(record["publicKeyX"], other);
    record["publicKeyX"] = other;
    assert_eq!(support::round_trip(&payload).unwrap(), payload);
    let (_dir, core) = unlocked(&payload);
    assert!(core
        .all_account_records()
        .unwrap()
        .iter()
        .all(|record| record.id != id));
    let unusable = core.unusable_accounts().unwrap();
    assert_eq!(unusable.len(), 1);
    assert_eq!(unusable[0].reason, UnusableReason::Ledger);
}

/// `isLedgerApplicationForFamily`: a Braavos account only has a Ledger key at Ready's scheme,
/// so one at Bramble's is never operated.
#[test]
fn a_braavos_ledger_record_at_bramble_s_scheme_is_not_operated() {
    let mut payload = g4();
    let braavos = payload["accounts"][1].clone();
    assert_eq!(braavos["accountContractType"], "braavos");
    let id = braavos["id"].clone();
    let record = &mut payload["accounts"][1];
    record["derivation"] = json!({
        "accountIndex": 0, "eip2645application": "LedgerW", "kind": "ledger", "path": LEDGERW_0,
    });
    record.as_object_mut().unwrap().remove("publicKeyY");
    payload["signingKeys"]
        .as_array_mut()
        .unwrap()
        .retain(|key| key["accountId"] != id);
    // The extension's validator accepts the vault (the family rule is its creation path's), and
    // mobile keeps the record: written back as read, listed as a Ledger account not operated.
    assert_eq!(support::round_trip(&payload).unwrap(), payload);
    let (_dir, core) = unlocked(&payload);
    let id = id.as_str().unwrap();
    assert!(core
        .all_account_records()
        .unwrap()
        .iter()
        .all(|record| record.id != id));
    let unusable = core.unusable_accounts().unwrap();
    assert_eq!(unusable.len(), 1);
    assert_eq!(unusable[0].id, id);
    assert_eq!(unusable[0].reason, UnusableReason::Ledger);
}
