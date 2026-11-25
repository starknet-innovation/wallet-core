//! P7 migrate, against G7b: the full wallet the extension's own vault service wrote at 13
//! (`vault-v13-full.json`) and at 14 (`vault-v14.json`), and the 13→14 migration its unlock runs
//! (`vault-v13-to-v14.json`: the v13 input and what `readMigratedVault` wrote back). The
//! extension's step is the version alone (`schema/migrations.ts`, `fromVersion: 13`); every
//! collection, a kept one's included, survives as it was. Payloads are compared as JSON values:
//! the ciphertexts differ by design (each seal draws its own IV and salt).
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

mod common;

use std::sync::Arc;

use bramble_core::testing::{open_vault, seal_container_vault, v13_round_trip};
use bramble_core::{CoreError, WalletCore, WalletStatus};
use common::{core, PASSWORD};
use serde_json::Value;
use starknet_wallet_core::envelope::{open, Envelope};

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// The payload the extension sealed in `case`, checked against the one it records beside it.
fn unsealed(case: &Value) -> Value {
    let envelope: Envelope = serde_json::from_value(case["sealed"].clone()).unwrap();
    let text = |value: &Value| value.as_str().unwrap().to_string();
    let (plaintext, _) = open(&envelope, &text(&case["password"]), &text(&case["scope"])).unwrap();
    let sealed: Value = serde_json::from_slice(&plaintext).unwrap();
    assert_eq!(sealed, case["payload"]);
    sealed
}

/// A core over a fresh directory holding `payload` in a container of its schema, still locked.
fn sealed(payload: &Value) -> (tempfile::TempDir, Arc<WalletCore>) {
    let (dir, _clock, core) = core();
    let version = payload["schemaVersion"].as_u64().unwrap() as u32;
    let path = dir.path().to_string_lossy().to_string();
    seal_container_vault(&path, PASSWORD, version, &payload.to_string()).unwrap();
    (dir, core)
}

fn on_disk(dir: &tempfile::TempDir) -> Value {
    serde_json::from_str(&open_vault(&dir.path().to_string_lossy(), PASSWORD).unwrap()).unwrap()
}

fn file(dir: &tempfile::TempDir) -> Vec<u8> {
    std::fs::read(dir.path().join("vault.json")).unwrap()
}

fn container_version(dir: &tempfile::TempDir) -> Value {
    serde_json::from_slice::<Value>(&file(dir)).unwrap()["version"].clone()
}

/// Every v13 collection filled, as the extension wrote it: it opens, and writes back unchanged.
#[test]
fn the_full_v13_vault_opens_and_writes_back_unchanged() {
    let payload = unsealed(&fixture("vault-v13-full.json"));
    assert_eq!(payload["schemaVersion"], 13);
    let written = v13_round_trip(&payload.to_string()).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&written).unwrap(), payload);
}

/// The same wallet at 14 with its `accountProvenance` opens, round-trips, and unlocking it
/// rewrites nothing: a v14 vault is not migrated again.
#[test]
fn the_v14_vault_opens_round_trips_and_is_not_migrated() {
    let payload = unsealed(&fixture("vault-v14.json"));
    assert_eq!(payload["schemaVersion"], 14);
    assert!(payload["accountProvenance"].is_object());
    let written = v13_round_trip(&payload.to_string()).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&written).unwrap(), payload);

    let (dir, core) = sealed(&payload);
    let before = file(&dir);
    core.unlock(PASSWORD.into()).unwrap();
    assert_eq!(file(&dir), before);
}

/// Unlocking the extension's v13 input writes back the extension's output: 14 in payload and
/// container, nothing else changed, no provenance inferred. A second unlock rewrites nothing.
#[test]
fn unlock_raises_a_v13_vault_to_14_as_the_extension_does() {
    let case = fixture("vault-v13-to-v14.json");
    let input = unsealed(&case["input"]);
    let output = unsealed(&case["output"]);
    assert_eq!(input, unsealed(&fixture("vault-v13-full.json")));

    let (dir, core) = sealed(&input);
    core.unlock(PASSWORD.into()).unwrap();
    assert_eq!(on_disk(&dir), output);
    assert!(on_disk(&dir).get("accountProvenance").is_none());
    assert_eq!(container_version(&dir), 14);

    let migrated = file(&dir);
    core.lock();
    core.unlock(PASSWORD.into()).unwrap();
    assert_eq!(file(&dir), migrated);
}

/// A write-back that fails leaves the v13 file as it was and the wallet locked, as the
/// extension's does (its primary is untouched before commit); the next unlock migrates.
#[test]
fn a_failed_migration_leaves_the_v13_vault_intact() {
    let input = unsealed(&fixture("vault-v13-to-v14.json")["input"]);
    let (dir, core) = sealed(&input);
    let before = file(&dir);
    // The atomic write's temporary path, taken by a directory: the write fails before rename.
    let blocker = dir.path().join("vault.json.tmp");
    std::fs::create_dir(&blocker).unwrap();
    assert!(matches!(
        core.unlock(PASSWORD.into()),
        Err(CoreError::Storage { .. })
    ));
    assert_eq!(file(&dir), before);
    assert_eq!(core.status(), WalletStatus::Locked);

    std::fs::remove_dir(&blocker).unwrap();
    core.unlock(PASSWORD.into()).unwrap();
    assert_eq!(container_version(&dir), 14);
}
