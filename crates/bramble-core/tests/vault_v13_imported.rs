//! Private-key import against the extension's own vault (`docs/PLAN.md` P3):
//! `fixtures/vault-v13-imported.json` (G7) is a v13 vault the extension's store sealed, holding
//! OpenZeppelin index 0 from the published phrase and two accounts imported from raw keys
//! (`imported-raw-key`, an OpenZeppelin and an Argent one; the keys are the phrase's own at
//! indexes 7 and 8). The core reads it under every rule and writes it back unchanged, opens it
//! with each imported account usable and signing with its sealed key, and its own import of the
//! same OpenZeppelin key seals the record the extension sealed.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

use crate::common;

use bramble_core::testing::{open_vault, seal_container_vault, v13_round_trip};
use bramble_core::{AccountFamily, NetworkId};
use common::{core, open, oz, FakeClock};
use serde_json::Value;
use starknet_wallet_core::envelope::{open as open_envelope, Envelope};

fn fixture() -> Value {
    let path = format!(
        "{}/tests/fixtures/vault-v13-imported.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap()
}

/// The fixture's raw-key accounts, in the order the extension imported them.
fn imported_accounts(payload: &Value) -> Vec<&Value> {
    payload["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|account| account["derivation"]["kind"] == "imported-raw-key")
        .collect()
}

/// The key the extension sealed for `id` (the key that was pasted).
fn sealed_key<'a>(payload: &'a Value, id: &str) -> &'a str {
    let keys = payload["signingKeys"].as_array().unwrap();
    let entry = keys.iter().find(|entry| entry["accountId"] == id).unwrap();
    text(&entry["privateKey"])
}

#[test]
fn the_extensions_sealed_vault_holds_a_payload_the_core_reads_and_writes_back() {
    let fixture = fixture();
    let envelope: Envelope = serde_json::from_value(fixture["sealed"].clone()).unwrap();
    let (plaintext, _) = open_envelope(
        &envelope,
        text(&fixture["password"]),
        text(&fixture["scope"]),
    )
    .unwrap();
    let sealed: Value = serde_json::from_slice(&plaintext).unwrap();
    assert_eq!(sealed, fixture["payload"]);
    assert_eq!(imported_accounts(&sealed).len(), 2);

    let written = v13_round_trip(std::str::from_utf8(&plaintext).unwrap()).unwrap();
    assert!(
        serde_json::from_str::<Value>(&written).unwrap() == sealed,
        "the reader writes back what it read"
    );
}

#[test]
fn each_imported_account_opens_usable_and_signs_with_its_sealed_key() {
    let fixture = fixture();
    let payload = &fixture["payload"];
    let password = text(&fixture["password"]);
    let dir = tempfile::tempdir().unwrap();
    let clock = FakeClock::new();
    seal_container_vault(
        &dir.path().to_string_lossy(),
        password,
        13,
        &payload.to_string(),
    )
    .unwrap();
    let core = open(dir.path(), &clock);
    core.unlock(password.into()).unwrap();
    assert!(core.unusable_accounts().unwrap().is_empty());

    let records = core.all_account_records().unwrap();
    assert_eq!(records.len(), payload["accounts"].as_array().unwrap().len());
    for expected in imported_accounts(payload) {
        let id = text(&expected["id"]);
        let record = records.iter().find(|record| record.id == id).unwrap();
        let family = match text(&expected["accountContractType"]) {
            "openzeppelin" => AccountFamily::OpenZeppelin,
            "argent" => AccountFamily::Argent,
            other => panic!("{other}"),
        };
        assert!(record.imported, "{id}");
        assert_eq!(record.family, family);
        assert_eq!(record.network, NetworkId::Mainnet);
        assert_eq!(record.address, text(&expected["address"]));
        assert_eq!(record.public_key, text(&expected["publicKeyX"]));
        assert_eq!(record.class_hash, text(&expected["classHash"]));
        assert_eq!(
            record.deployment_salt.as_deref(),
            expected["deploymentSalt"].as_str()
        );
        assert_eq!(record.active, payload["activeAccountId"] == id);
        let signed = core.sign_hash_for_account(id.into(), "0x1".into()).unwrap();
        assert_eq!(signed.public_key, record.public_key);
    }
    // The phrase's account is not imported.
    let phrase_account = records.iter().find(|record| !record.imported).unwrap();
    assert_eq!(phrase_account.family, AccountFamily::OpenZeppelin);
}

/// The core imports OpenZeppelin keys only (P3 leaves Argent import out), so the wallet built
/// here is the fixture's without the Argent import, active on the OpenZeppelin one.
#[test]
fn importing_the_same_key_here_seals_the_extensions_record() {
    let fixture = fixture();
    let payload = &fixture["payload"];
    let imported = imported_accounts(payload);
    let (oz_import, argent_import) = (imported[0], imported[1]);
    assert_eq!(oz_import["accountContractType"], "openzeppelin");
    assert_eq!(argent_import["accountContractType"], "argent");

    let (dir, _clock, core) = core();
    core.import_wallet(common::PASSWORD.into(), text(&fixture["mnemonic"]).into())
        .unwrap();
    core.restore_recovered_accounts(NetworkId::Mainnet, vec![oz(0)])
        .unwrap();
    let key = sealed_key(payload, text(&oz_import["id"]));
    let record = core
        .import_private_key(NetworkId::Mainnet, key.into())
        .unwrap();
    assert!(record.imported && record.active);
    assert_eq!(record.id, text(&oz_import["id"]));

    let mut expected = payload.clone();
    // A new wallet here is written at 14, where the pasted key is recorded `imported`, as the
    // extension's `importRawKeyAccount` records it at 14.
    expected["schemaVersion"] = 14.into();
    expected["accountProvenance"] = serde_json::json!({ text(&oz_import["id"]): "imported" });
    let argent_id = text(&argent_import["id"]).to_string();
    for (collection, field) in [("accounts", "id"), ("signingKeys", "accountId")] {
        expected[collection]
            .as_array_mut()
            .unwrap()
            .retain(|entry| entry[field] != argent_id.as_str());
    }
    expected["activeAccountId"] = oz_import["id"].clone();
    let built: Value =
        serde_json::from_str(&open_vault(&dir.path().to_string_lossy(), common::PASSWORD).unwrap())
            .unwrap();
    // Boolean assert: a failure must not print a key.
    assert!(built == expected, "the core seals the extension's payload");
}
