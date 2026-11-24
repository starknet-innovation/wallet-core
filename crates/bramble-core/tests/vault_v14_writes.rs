//! P7, second half: what mobile's writes do to a vault's schema and `accountProvenance`, as the
//! extension's service on `release/v0.10.11` does it. A v13 vault is raised to 14 on unlock (no
//! entry inferred, `vault_migration.rs`); at 14 an account added here is `created`
//! (`addDerivedAccount`), a pasted key `imported` (`importRawKeyAccount`), a projection onto
//! another network takes its source's origin or none (`ensureNetworkAccount`), and a restore from
//! the phrase records none (`recoverVaultAccounts`). Every other entry, a kept record's included,
//! stays as it was.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

mod common;
#[path = "vault_v13_support/mod.rs"]
mod support;

use std::sync::Arc;

use bramble_core::testing::{open_vault, seal_container_vault};
use bramble_core::{NetworkId, WalletCore};
use common::{core, PASSWORD};
use serde_json::{json, Value};
use starknet_wallet_core::derive_starknet_keypair;
use support::{full, g4, kept_ids, text};

/// `scripts/check-krusty-adapter.ts`'s published phrase (`key_import.rs`): a key no vault here
/// holds.
const LEGAL: &str = "legal winner thank year wave sausage worth useful legal winner thank yellow";

fn at_14(payload: &Value, provenance: Option<Value>) -> Value {
    let mut payload = payload.clone();
    payload["schemaVersion"] = 14.into();
    if let Some(provenance) = provenance {
        payload["accountProvenance"] = provenance;
    }
    payload
}

/// A core over a fresh directory holding `payload` in a container of its schema, unlocked.
fn unlocked(payload: &Value) -> (tempfile::TempDir, Arc<WalletCore>) {
    let (dir, _clock, core) = core();
    let path = dir.path().to_string_lossy().to_string();
    let version = payload["schemaVersion"].as_u64().unwrap() as u32;
    seal_container_vault(&path, PASSWORD, version, &payload.to_string()).unwrap();
    core.unlock(PASSWORD.into()).unwrap();
    (dir, core)
}

fn on_disk(dir: &tempfile::TempDir) -> Value {
    serde_json::from_str(&open_vault(&dir.path().to_string_lossy(), PASSWORD).unwrap()).unwrap()
}

/// The container's `version` on disk.
fn container_version(dir: &tempfile::TempDir) -> u64 {
    let file = std::fs::read_to_string(dir.path().join("vault.json")).unwrap();
    serde_json::from_str::<Value>(&file).unwrap()["version"]
        .as_u64()
        .unwrap()
}

/// `written` without the record `id`, its key and its origin, and with `active` active.
fn without(written: &Value, id: &str, active: &Value) -> Value {
    let mut payload = written.clone();
    for (collection, field) in [("accounts", "id"), ("signingKeys", "accountId")] {
        payload[collection]
            .as_array_mut()
            .unwrap()
            .retain(|entry| entry[field] != id);
    }
    if let Some(entries) = payload
        .get_mut("accountProvenance")
        .and_then(Value::as_object_mut)
    {
        entries.remove(id);
    }
    payload["activeAccountId"] = active.clone();
    payload
}

fn kept_provenance() -> Value {
    let kept = kept_ids(&full());
    let mut entries = serde_json::Map::new();
    entries.insert(kept.ledger, "imported".into());
    entries.insert(kept.devnet, "created".into());
    Value::Object(entries)
}

/// An account added here is `created`; every other entry and record stays, and the vault stays
/// at 14, in a container of 14. A selection writes the map back as it was.
#[test]
fn an_added_account_is_created() {
    let base = at_14(&full(), Some(kept_provenance()));
    let (dir, core) = unlocked(&base);
    let added = core.add_regular_account(NetworkId::Sepolia).unwrap();
    let written = on_disk(&dir);
    assert_eq!(written["accountProvenance"][&added.id], "created");
    assert_eq!(without(&written, &added.id, &base["activeAccountId"]), base);
    assert_eq!(container_version(&dir), 14);
    // With no map yet, the first entry makes one (`{ ...undefined, [id]: "created" }`).
    let (dir, core) = unlocked(&at_14(&full(), None));
    let added = core.add_regular_account(NetworkId::Sepolia).unwrap();
    let one = json!({ added.id: "created" });
    assert_eq!(on_disk(&dir)["accountProvenance"], one);
    // A selection leaves it as it is.
    let main = text(&g4()["accounts"][0]["id"]);
    core.select_account(main.clone()).unwrap();
    let selected = on_disk(&dir);
    assert_eq!(selected["accountProvenance"], one);
    assert_eq!(selected["activeAccountId"], json!(main));
}

/// A pasted key is `imported`.
#[test]
fn an_imported_key_is_imported() {
    let (dir, core) = unlocked(&at_14(&g4(), None));
    let key = derive_starknet_keypair(LEGAL, 0, 0, None)
        .unwrap()
        .private_key
        .expose_secret_hex();
    let record = core
        .import_private_key(NetworkId::Mainnet, key.to_string())
        .unwrap();
    assert_eq!(
        on_disk(&dir)["accountProvenance"],
        json!({ record.id: "imported" })
    );
}

/// The network switch's projection takes its source's origin, and records none for a source the
/// vault has none for: an empty map stays empty, an absent one absent.
#[test]
fn a_projection_takes_its_sources_origin() {
    // G4 without its Sepolia record, so the switch projects the active account (Mainnet OZ 1).
    let mut mainnet = g4();
    let sepolia = text(&mainnet["accounts"][5]["id"]);
    for (collection, field) in [("accounts", "id"), ("signingKeys", "accountId")] {
        mainnet[collection]
            .as_array_mut()
            .unwrap()
            .retain(|entry| entry[field] != sepolia.as_str());
    }
    let source = text(&mainnet["activeAccountId"]);
    for (provenance, expected) in [
        (
            Some(json!({ source.clone(): "imported" })),
            Some("imported"),
        ),
        (Some(json!({ source.clone(): "created" })), Some("created")),
        (Some(json!({})), None),
        (None, None),
    ] {
        let base = at_14(&mainnet, provenance.clone());
        let (dir, core) = unlocked(&base);
        let projected = core
            .ensure_network_account(NetworkId::Sepolia)
            .unwrap()
            .unwrap();
        assert_eq!(projected.network, NetworkId::Sepolia);
        let written = on_disk(&dir);
        let origin = written
            .get("accountProvenance")
            .and_then(|entries| entries.get(&projected.id));
        assert_eq!(origin.and_then(Value::as_str), expected, "{provenance:?}");
        assert_eq!(
            without(&written, &projected.id, &base["activeAccountId"]),
            base
        );
    }
}

/// A restore from the phrase records no origin (`recoverVaultAccounts` writes none); a kept
/// record's entry stays.
#[test]
fn a_restore_records_no_origin() {
    let full = full();
    let kept = kept_ids(&full);
    let mut only = at_14(&full, Some(json!({ kept.ledger.clone(): "imported" })));
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
    let (dir, core) = unlocked(&only);
    assert!(core.recovery_pending().unwrap());
    let restored = core
        .restore_recovered_accounts(NetworkId::Sepolia, vec![common::oz(0)])
        .unwrap();
    let written = on_disk(&dir);
    assert_eq!(written["accountProvenance"], only["accountProvenance"]);
    assert_eq!(
        without(&written, &restored[0].id, &only["activeAccountId"]),
        only
    );
}

/// A v13 vault is raised to 14 on unlock with no map (the extension's step infers none), so an
/// account added after it is recorded `created` like any other at 14.
#[test]
fn a_v13_vault_is_raised_to_14_and_records_from_then_on() {
    let (dir, core) = unlocked(&full());
    let added = core.add_regular_account(NetworkId::Sepolia).unwrap();
    let written = on_disk(&dir);
    assert_eq!(
        written["accountProvenance"],
        json!({ added.id.clone(): "created" })
    );
    assert_eq!(
        without(&written, &added.id, &full()["activeAccountId"]),
        at_14(&full(), Some(json!({})))
    );
    assert_eq!(container_version(&dir), 14);
}

/// A new wallet is written at `NEW_VAULT_SCHEMA`, 14, created or imported.
#[test]
fn a_new_wallet_is_written_at_14() {
    let (dir, _clock, core) = core();
    core.create_wallet(PASSWORD.into(), 12).unwrap();
    let (imported, _clock, _core) = common::imported();
    for dir in [&dir, &imported] {
        assert_eq!(on_disk(dir)["schemaVersion"], 14);
        assert_eq!(container_version(dir), 14);
    }
    // A created wallet's first account is `created`, as the extension's creation records it.
    let created = on_disk(&dir);
    let first = created["accounts"][0]["id"].as_str().unwrap();
    assert_eq!(created["accountProvenance"], json!({ first: "created" }));
}
