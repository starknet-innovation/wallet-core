//! P7, second half: the extension's schema 14 (`release/v0.10.11`), which is v13 and an optional
//! `accountProvenance`. No extension-sealed v14 vault exists yet (G4 is v13, and the v14 one is
//! owed by a Bramble generator), so the vaults are built in test code over G4's payload
//! (`vault_v13_support`), and the cases are `schema-v14.test.ts`'s and `validAccountProvenance`'s
//! rules, one refusal per guard. `vault_v14_writes.rs` covers what writes do to the map.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

mod common;
#[path = "vault_v13_support/mod.rs"]
mod support;

use std::sync::Arc;

use bramble_core::testing::{open_vault, seal_container_vault, v13_round_trip};
use bramble_core::{CoreError, NetworkId, WalletCore};
use common::{core, PASSWORD};
use serde_json::{json, Map, Value};
use support::{full, g4, kept_ids, opens, round_trip, text};

/// `payload` at schema 14, with `provenance` as its `accountProvenance` (absent for `None`).
fn at_14(payload: &Value, provenance: Option<Value>) -> Value {
    let mut payload = payload.clone();
    payload["schemaVersion"] = 14.into();
    if let Some(provenance) = provenance {
        payload["accountProvenance"] = provenance;
    }
    payload
}

/// G4's first record as created here, and `full()`'s kept records as the extension records them:
/// operated and kept ids alike.
fn mixed() -> Value {
    let kept = kept_ids(&full());
    let mut entries = Map::new();
    entries.insert(text(&g4()["accounts"][0]["id"]), "created".into());
    entries.insert(kept.ledger, "imported".into());
    entries.insert(kept.seeded, "imported".into());
    entries.insert(kept.devnet, "created".into());
    Value::Object(entries)
}

/// Every record of `payload` as `origin`.
fn all_as(payload: &Value, origin: &str) -> Value {
    payload["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|account| (text(&account["id"]), Value::from(origin)))
        .collect::<Map<_, _>>()
        .into()
}

/// A core over a fresh directory holding `payload` in a container of `version`, and its unlock.
fn sealed_at(version: u32, payload: &Value) -> (tempfile::TempDir, Arc<WalletCore>, bool) {
    let (dir, _clock, core) = core();
    let path = dir.path().to_string_lossy().to_string();
    seal_container_vault(&path, PASSWORD, version, &payload.to_string()).unwrap();
    let unlocked = match core.unlock(PASSWORD.into()) {
        Ok(()) => true,
        Err(CoreError::Storage { .. }) => false,
        Err(other) => panic!("unexpected refusal: {other:?}"),
    };
    (dir, core, unlocked)
}

#[test]
fn a_v14_vault_opens_and_writes_back_unchanged() {
    for payload in [
        // Absent stays absent, empty stays empty: neither is read as the other.
        at_14(&full(), None),
        at_14(&full(), Some(json!({}))),
        // Kept records keep their entries beside the operated ones.
        at_14(&full(), Some(mixed())),
        at_14(&g4(), Some(all_as(&g4(), "imported"))),
    ] {
        assert_eq!(round_trip(&payload).unwrap(), payload);
    }
}

/// A named change to a payload, which may borrow from the test.
type Case<'a> = (&'static str, Box<dyn Fn(&mut Value) + 'a>);

/// `isVaultV14`: v13's rules over the rest, and `validAccountProvenance` over the map.
#[test]
fn the_v14_validator_refuses_what_the_extension_refuses() {
    let base = at_14(&full(), Some(mixed()));
    assert!(opens(&base));
    let main = text(&g4()["accounts"][0]["id"]);
    let cases: Vec<Case> = vec![
        (
            "an account the vault does not hold (`schema-v14.test.ts`)",
            Box::new(|p| p["accountProvenance"]["SN_MAIN:0x123"] = "imported".into()),
        ),
        (
            "a held account spelt another way",
            Box::new(|p| p["accountProvenance"][main.replace("0x", "0X")] = "created".into()),
        ),
        (
            "an origin it does not know (`schema-v14.test.ts`'s `guessed`)",
            Box::new(|p| p["accountProvenance"][&main] = "guessed".into()),
        ),
        (
            "an origin in another case",
            Box::new(|p| p["accountProvenance"][&main] = "Created".into()),
        ),
        (
            "an origin in serde's map form",
            Box::new(|p| p["accountProvenance"][&main] = json!({ "created": null })),
        ),
        (
            "a null origin",
            Box::new(|p| p["accountProvenance"][&main] = Value::Null),
        ),
        (
            "a null map: an optional key is absent or set",
            Box::new(|p| p["accountProvenance"] = Value::Null),
        ),
        (
            "an array (`isRecord`)",
            Box::new(|p| p["accountProvenance"] = json!([])),
        ),
        (
            "a v13 rule broken at 14: a record without its key",
            Box::new(|p| {
                p["signingKeys"].as_array_mut().unwrap().remove(0);
            }),
        ),
        (
            "an unknown key at 14",
            Box::new(|p| p["label"] = "unexpected".into()),
        ),
        (
            "provenance in a v13 vault",
            Box::new(|p| p["schemaVersion"] = 13.into()),
        ),
        (
            "a schema after 14",
            Box::new(|p| p["schemaVersion"] = 15.into()),
        ),
        (
            "a schema before 13",
            Box::new(|p| p["schemaVersion"] = 12.into()),
        ),
    ];
    for (why, change) in cases {
        let mut payload = base.clone();
        change(&mut payload);
        assert!(!opens(&payload), "{why}");
    }
}

/// `JSON.parse` keeps the last of two equal keys and the extension then accepts the map; mobile
/// refuses it, as serde refuses a repeated key everywhere else in the vault.
#[test]
fn a_repeated_provenance_key_is_refused() {
    let main = text(&g4()["accounts"][0]["id"]);
    let once = at_14(&full(), Some(json!({ main.clone(): "created" }))).to_string();
    let needle = r#""accountProvenance":{"#;
    let twice = once.replacen(needle, &format!(r#"{needle}"{main}":"created","#), 1);
    assert_ne!(once, twice);
    assert!(v13_round_trip(&once).is_ok());
    assert!(matches!(
        v13_round_trip(&twice),
        Err(CoreError::Storage { .. })
    ));
}

/// The container's version is the payload's schema: 13 and 14 open, each only around a payload
/// of its own schema, and nothing later does.
#[test]
fn the_container_and_the_payload_agree_on_the_schema() {
    let v13 = full();
    let v14 = at_14(&full(), Some(mixed()));
    assert!(sealed_at(13, &v13).2);
    assert!(sealed_at(14, &v14).2);
    assert!(!sealed_at(14, &v13).2);
    assert!(!sealed_at(13, &v14).2);
    let (dir, _core, _) = sealed_at(15, &v14);
    // A container past what mobile knows does not even parse.
    assert!(open_vault(&dir.path().to_string_lossy(), PASSWORD).is_err());
}

/// The largest G4 at 14 the budget admits, filled with imported phrases.
fn at_capacity() -> Value {
    let with_seeds = |count: usize| {
        let mut payload = at_14(&g4(), None);
        let mnemonic = support::fixture()["mnemonic"].clone();
        payload["importedSeeds"] = (1..=count)
            .map(|n| json!({ "id": format!("phrase-{n}"), "mnemonic": mnemonic }))
            .collect();
        payload
    };
    let (mut fits, mut over) = (1, 4_000);
    while over - fits > 1 {
        let middle = (fits + over) / 2;
        if opens(&with_seeds(middle)) {
            fits = middle;
        } else {
            over = middle;
        }
    }
    with_seeds(fits)
}

/// `isVaultV14` sets the map aside before `isVaultV13` (and so `isVaultV11`'s budget) sees the
/// rest, so a vault at capacity still opens with an entry for every account. A write past the
/// budget fails and is rolled back with its new entry, so the next write (a selection) neither
/// carries an entry for the account that was not added nor fails on one.
#[test]
fn provenance_is_outside_the_budget_and_rolls_back_with_a_failed_write() {
    let mut at_capacity = at_capacity();
    at_capacity["accountProvenance"] = all_as(&at_capacity, "created");
    assert_eq!(round_trip(&at_capacity).unwrap(), at_capacity);
    let (dir, core, unlocked) = sealed_at(14, &at_capacity);
    assert!(unlocked);
    assert!(matches!(
        core.add_regular_account(NetworkId::Mainnet),
        Err(CoreError::Storage { reason }) if reason == "vault storage limit reached"
    ));
    let other = text(&g4()["accounts"][0]["id"]);
    core.select_account(other.clone()).unwrap();
    let written: Value =
        serde_json::from_str(&open_vault(&dir.path().to_string_lossy(), PASSWORD).unwrap())
            .unwrap();
    let mut expected = at_capacity.clone();
    expected["activeAccountId"] = other.into();
    assert_eq!(written, expected);
}
