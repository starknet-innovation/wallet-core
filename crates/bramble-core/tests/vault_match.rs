//! Mobile opens every vault the extension opens (PLAN.md P7): a record mobile cannot operate
//! is kept unused and listed (`unusable_accounts`), never a reason to refuse the vault, and a
//! Ready account whose key an earlier Krusty derived signs with the key the vault sealed, as the
//! extension keeps signing with it.
#![cfg(feature = "testing")]

use crate::common;

use bramble_core::testing::{open_vault, seal_container_vault};
use bramble_core::{AccountFamily, NetworkId, UnusableReason, WalletCore};
use common::{open, oz, pending, selection, FakeClock, PASSWORD, PHRASE};
use serde_json::{json, Value};
use std::sync::Arc;

/// Another valid phrase, standing in for the key an earlier Krusty derived.
const OTHER_PHRASE: &str =
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
const HASH: &str = "0x1234";

fn path(dir: &tempfile::TempDir) -> String {
    dir.path().to_string_lossy().to_string()
}

fn on_disk(dir: &tempfile::TempDir) -> Value {
    serde_json::from_str(&open_vault(&path(dir), PASSWORD).unwrap()).unwrap()
}

/// Seals `payload` at 14 (a new wallet's schema) over the wallet in `dir` and unlocks it again.
fn reopened(dir: &tempfile::TempDir, clock: &Arc<FakeClock>, payload: &Value) -> Arc<WalletCore> {
    seal_container_vault(&path(dir), PASSWORD, 14, &payload.to_string()).unwrap();
    let core = open(dir.path(), clock);
    core.unlock(PASSWORD.into()).unwrap();
    core
}

/// A vault of `phrase` holding Ready (`argentx-bip44`) index 0 on Sepolia.
fn ready_vault(phrase: &str) -> (tempfile::TempDir, Arc<FakeClock>, Value) {
    let (dir, clock, core) = common::core();
    core.import_wallet(PASSWORD.into(), phrase.into()).unwrap();
    core.restore_recovered_accounts(
        NetworkId::Sepolia,
        vec![selection(AccountFamily::Argent, 0, None)],
    )
    .unwrap();
    core.lock();
    let payload = on_disk(&dir);
    assert_eq!(
        payload["accounts"][0]["derivation"]["kind"],
        "argentx-bip44"
    );
    (dir, clock, payload)
}

#[test]
fn a_pre_fix_ready_key_signs_with_the_key_the_vault_sealed() {
    // The record and key another derivation made, at this phrase's Ready coordinate: what an
    // extension vault restored under Krusty 2.0 holds for an affected phrase.
    let (_donor_dir, _, donor) = ready_vault(OTHER_PHRASE);
    let (dir, clock, mut payload) = ready_vault(PHRASE);
    let own_id = payload["accounts"][0]["id"].clone();
    payload["accounts"][0] = donor["accounts"][0].clone();
    payload["signingKeys"][0] = donor["signingKeys"][0].clone();
    payload["activeAccountId"] = donor["accounts"][0]["id"].clone();
    assert_ne!(payload["accounts"][0]["id"], own_id);

    let core = reopened(&dir, &clock, &payload);
    let records = core.account_records(NetworkId::Sepolia).unwrap();
    assert_eq!(records.len(), 1);
    let record = &records[0];
    assert_eq!(json!(record.id), donor["accounts"][0]["id"]);
    assert!(record.pre_fix_ready_key && !record.imported);
    assert!(core.unusable_accounts().unwrap().is_empty());
    // It signs with the sealed key, not the one the phrase derives at that coordinate.
    let signature = core
        .sign_hash_for_account(record.id.clone(), HASH.into())
        .unwrap();
    assert_eq!(signature.public_key, record.public_key);

    // A write keeps the sealed key, and the network switch seals a copy under the projection,
    // as the extension's `deriveNetworkAccount` copies the source's stored key.
    let mainnet = core
        .ensure_network_account(NetworkId::Mainnet)
        .unwrap()
        .unwrap();
    assert!(mainnet.pre_fix_ready_key);
    assert_eq!(mainnet.public_key, record.public_key);
    let written = on_disk(&dir);
    assert_eq!(written["accounts"][0], donor["accounts"][0]);
    let sealed = donor["signingKeys"][0]["privateKey"].clone();
    let keys = written["signingKeys"].as_array().unwrap();
    assert_eq!(keys.len(), 2);
    assert!(keys.iter().all(|key| key["privateKey"] == sealed));

    // Unlocked again, both still open and sign with that key.
    core.lock();
    core.unlock(PASSWORD.into()).unwrap();
    let signature = core.sign_hash_for_account(mainnet.id, HASH.into()).unwrap();
    assert_eq!(signature.public_key, record.public_key);
}

/// A record whose sealed key places a different record (here a Ready record carrying another
/// key's address) is kept, not signed with.
#[test]
fn a_ready_record_its_sealed_key_does_not_place_is_kept() {
    let (_donor_dir, _, donor) = ready_vault(OTHER_PHRASE);
    let (dir, clock, mut payload) = ready_vault(PHRASE);
    let own = payload["accounts"][0].clone();
    payload["accounts"][0] = donor["accounts"][0].clone();
    payload["activeAccountId"] = donor["accounts"][0]["id"].clone();
    payload["signingKeys"][0]["accountId"] = donor["accounts"][0]["id"].clone();
    assert_ne!(payload["accounts"][0], own);

    let core = reopened(&dir, &clock, &payload);
    assert!(core.account_records(NetworkId::Sepolia).unwrap().is_empty());
    let unusable = core.unusable_accounts().unwrap();
    assert_eq!(unusable.len(), 1);
    assert_eq!(unusable[0].reason, UnusableReason::NotDerivedHere);
    assert!(core.active_account().unwrap().is_none());
}

/// The extension imports an Argent key at an address the person picks, which the key does not
/// place: mobile keeps it unused, lists it, and writes it back unchanged.
#[test]
fn a_record_at_an_address_its_key_does_not_place_is_kept_and_written_back() {
    let (dir, clock, core) = pending();
    core.restore_recovered_accounts(NetworkId::Sepolia, vec![oz(0), oz(1)])
        .unwrap();
    core.lock();
    let mut payload = on_disk(&dir);
    let picked = "0x2cd1d9d3c7ab3b52b2b61327e14c5cfa4e9f2ec5d2de8ac6e7a3c1c1c7e0e4b";
    let picked_id = format!("SN_SEPOLIA:{picked}");
    let old_id = payload["accounts"][0]["id"].clone();
    payload["accounts"][0]["address"] = picked.into();
    payload["accounts"][0]["id"] = picked_id.clone().into();
    for key in payload["signingKeys"].as_array_mut().unwrap() {
        if key["accountId"] == old_id {
            key["accountId"] = picked_id.clone().into();
        }
    }
    if payload["activeAccountId"] == old_id {
        payload["activeAccountId"] = picked_id.clone().into();
    }

    let core = reopened(&dir, &clock, &payload);
    let records = core.account_records(NetworkId::Sepolia).unwrap();
    assert_eq!(records.len(), 1);
    assert!(records.iter().all(|record| record.id != picked_id));
    let unusable = core.unusable_accounts().unwrap();
    assert_eq!(unusable.len(), 1);
    assert_eq!(
        (unusable[0].id.as_str(), unusable[0].network_id.as_str()),
        (picked_id.as_str(), "SN_SEPOLIA")
    );
    assert_eq!(unusable[0].reason, UnusableReason::NotDerivedHere);
    assert!(core
        .sign_hash_for_account(picked_id.clone(), HASH.into())
        .is_err());
    assert!(core.select_account(picked_id).is_err());

    // A write puts the kept record back where it was, with its key.
    core.select_account(records[0].id.clone()).unwrap();
    let written = on_disk(&dir);
    assert_eq!(written["accounts"], payload["accounts"]);
    assert_eq!(written["signingKeys"], payload["signingKeys"]);
}

/// Mobile re-derives at most 128 records on open; a vault holding more (the extension's only
/// bound is its JSON budget) opens with the rest kept.
#[test]
fn records_past_the_ones_mobile_operates_are_kept() {
    let (dir, clock, core) = pending();
    core.restore_recovered_accounts(NetworkId::Sepolia, vec![oz(0)])
        .unwrap();
    for _ in 1..128 {
        core.add_regular_account(NetworkId::Sepolia).unwrap();
    }
    core.lock();
    let (other_dir, _, other) = pending();
    other
        .restore_recovered_accounts(NetworkId::Mainnet, vec![oz(0)])
        .unwrap();
    other.lock();
    let extra = on_disk(&other_dir);
    let mut payload = on_disk(&dir);
    payload["accounts"]
        .as_array_mut()
        .unwrap()
        .push(extra["accounts"][0].clone());
    payload["signingKeys"]
        .as_array_mut()
        .unwrap()
        .push(extra["signingKeys"][0].clone());

    let core = reopened(&dir, &clock, &payload);
    assert_eq!(core.all_account_records().unwrap().len(), 128);
    let unusable = core.unusable_accounts().unwrap();
    assert_eq!(unusable.len(), 1);
    assert_eq!(json!(unusable[0].id), extra["accounts"][0]["id"]);
    assert_eq!(unusable[0].reason, UnusableReason::NotDerivedHere);
    // Mobile adds no record past its own bound, and the vault stays as it was.
    assert!(core.add_regular_account(NetworkId::Sepolia).is_err());
    assert_eq!(on_disk(&dir), payload);
}

/// Only Ready's derivation changed under an earlier Krusty: a `starknet-bip44` record whose
/// sealed key is not the phrase's is kept, even when that key places it.
#[test]
fn another_ladders_record_under_another_key_is_kept() {
    let donor_vault = |phrase: &str| {
        let (dir, clock, core) = common::core();
        core.import_wallet(PASSWORD.into(), phrase.into()).unwrap();
        core.restore_recovered_accounts(NetworkId::Sepolia, vec![oz(0)])
            .unwrap();
        core.lock();
        (on_disk(&dir), dir, clock)
    };
    let (donor, _donor_dir, _) = donor_vault(OTHER_PHRASE);
    let (mut payload, dir, clock) = donor_vault(PHRASE);
    payload["accounts"][0] = donor["accounts"][0].clone();
    payload["signingKeys"][0] = donor["signingKeys"][0].clone();
    payload["activeAccountId"] = donor["accounts"][0]["id"].clone();

    let core = reopened(&dir, &clock, &payload);
    assert!(core.all_account_records().unwrap().is_empty());
    let unusable = core.unusable_accounts().unwrap();
    assert_eq!(unusable.len(), 1);
    assert_eq!(unusable[0].reason, UnusableReason::NotDerivedHere);
}
