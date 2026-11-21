//! Private-key import (`docs/PLAN.md` P3): the key goes in once, is sealed in the vault as the
//! extension seals one (`imported-raw-key`, its key in `signingKeys`), signs from there after a
//! lock and unlock, and never comes back out. No extension-sealed vault holding an imported key
//! exists yet (G4 holds none), so the record shape is checked against the extension's code, not a
//! fixture. Every key here is derived at run time from a published BIP-39 test phrase.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

mod common;

#[path = "wallet_privacy_support/mod.rs"]
mod privacy_support;

use bramble_core::testing::{open_vault, seal_container_vault, v13_round_trip};
use bramble_core::{AccountFamily, AccountRecord, CoreError, NetworkId, WalletCore};
use common::{core, imported, open, own, pending, PASSWORD, PHRASE};
use serde_json::{json, Value};
use starknet_wallet_core::{derive_starknet_keypair, Zeroizing};

/// `scripts/check-krusty-adapter.ts`'s published phrase and the answers the reviewed extension
/// build gives for its key at index 0 (`parity.rs`, `known_answer_vectors_...`).
const LEGAL: &str = "legal winner thank year wave sausage worth useful legal winner thank yellow";
const LEGAL_PUBLIC_KEY: &str = "0xb7ff7bf7404503b6df17fa28ea02dd529e58ddd0cfb3232db83ef27630f6aa";
const LEGAL_MAINNET_ADDRESS: &str =
    "0x340cfbd6cf1d384e7029ccd7cdec84b22b53d2fcffdb005d499e580ec361069";
const LEGAL_R: &str = "0x4bbe22478347f1672ccd19d540c47d77ec9dc52d8fb21aeacf8889dfd5282d3";
const LEGAL_S: &str = "0x303baae9f9559ff686c55436408d771a51777c371f22e4db1e133a6c0384f4b";
const ORDER: &str = "0x800000000000010ffffffffffffffffb781126dcae7b2321e66a241adc64d2f";

/// The key at `index` on `phrase`'s own ladder, canonical.
fn key_of(phrase: &str, index: u32) -> Zeroizing<String> {
    derive_starknet_keypair(phrase, 0, index, None)
        .unwrap()
        .private_key
        .expose_secret_hex()
}

fn invalid(error: &CoreError, reason: &str) -> bool {
    matches!(error, CoreError::InvalidInput { reason: r } if r == reason)
}

fn payload(dir: &tempfile::TempDir) -> Value {
    serde_json::from_str(&open_vault(&dir.path().to_string_lossy(), PASSWORD).unwrap()).unwrap()
}

fn record_by_id(core: &WalletCore, id: &str) -> AccountRecord {
    core.all_account_records()
        .unwrap()
        .into_iter()
        .find(|record| record.id == id)
        .unwrap()
}

#[test]
fn an_imported_key_signs_the_extensions_answer_after_lock_and_unlock() {
    let (dir, clock, core) = imported();
    let key = key_of(LEGAL, 0);
    let record = core
        .import_private_key(NetworkId::Mainnet, key.to_string())
        .unwrap();
    assert!(record.imported && record.active);
    assert_eq!(record.family, AccountFamily::OpenZeppelin);
    assert_eq!(record.public_key, LEGAL_PUBLIC_KEY);
    assert_eq!(record.address, LEGAL_MAINNET_ADDRESS);
    assert_eq!(record.id, format!("SN_MAIN:{LEGAL_MAINNET_ADDRESS}"));
    assert_eq!(
        (record.account_index, record.deployment_salt.clone()),
        (0, None)
    );
    let digits = key.trim_start_matches("0x");
    assert!(
        !format!("{record:?}").contains(digits),
        "the record's Debug holds no key"
    );

    core.lock();
    let core = open(dir.path(), &clock);
    core.unlock(PASSWORD.into()).unwrap();
    assert_eq!(record_by_id(&core, &record.id), record);
    let signature = core
        .sign_hash_for_account(record.id.clone(), "0x1".into())
        .unwrap();
    assert_eq!(
        (signature.public_key, signature.r, signature.s),
        (LEGAL_PUBLIC_KEY.into(), LEGAL_R.into(), LEGAL_S.into())
    );

    // The phrase's account still signs with the phrase's key.
    let phrase_account = &core.account_records(NetworkId::Sepolia).unwrap()[0];
    assert!(!phrase_account.imported);
    let signed = core
        .sign_hash_for_account(phrase_account.id.clone(), "0x1".into())
        .unwrap();
    assert_eq!(signed.public_key, phrase_account.public_key);
    // The deprecated index shim does not list an imported account as an index.
    assert!(core.accounts(NetworkId::Mainnet).unwrap().is_empty());
}

#[test]
fn the_vault_holds_the_record_the_extension_writes() {
    let (dir, _clock, core) = imported();
    let key = key_of(LEGAL, 0);
    let record = core
        .import_private_key(NetworkId::Mainnet, key.to_string())
        .unwrap();
    let vault = payload(&dir);
    let account = vault["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|account| account["id"] == record.id.as_str())
        .unwrap();
    // `deriveImportedAccount`: the OZ contract fields, no salt and no y-coordinate.
    assert_eq!(
        *account,
        json!({
            "accountContractType": "openzeppelin",
            "address": LEGAL_MAINNET_ADDRESS,
            "classHash": record.class_hash,
            "derivation": { "kind": "imported-raw-key" },
            "id": record.id,
            "networkId": "SN_MAIN",
            "publicKeyX": LEGAL_PUBLIC_KEY,
        })
    );
    let sealed: Vec<&Value> = vault["signingKeys"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|sealed| sealed["accountId"] == record.id.as_str())
        .collect();
    // Boolean asserts: a failure must not print a key.
    assert!(sealed.len() == 1 && sealed[0]["privateKey"] == key.as_str());
    assert_eq!(vault["activeAccountId"], record.id.as_str());
    for unused in [
        "accountSecrets",
        "importedSeeds",
        "multisigStrk20KeyConflicts",
        "multisigStrk20ViewingKeys",
        "originGrants",
        "shadowAccountScopes",
    ] {
        assert_eq!(vault[unused], json!([]), "{unused}");
    }
    let written: Value =
        serde_json::from_str(&v13_round_trip(&vault.to_string()).unwrap()).unwrap();
    assert!(written == vault, "the reader writes back what it read");
}

#[test]
fn a_pasted_key_is_read_as_the_extensions_form_reads_it() {
    let key = key_of(LEGAL, 0);
    let digits = key.trim_start_matches("0x");
    let (_dir, _clock, core) = imported();
    let messy = format!(" \t0x000{}\n", digits.to_uppercase());
    let record = core.import_private_key(NetworkId::Mainnet, messy).unwrap();
    assert_eq!(record.public_key, LEGAL_PUBLIC_KEY);
    // Every spelling of that key names the account it already holds.
    for again in [key.to_string(), format!("0x{}", digits.to_uppercase())] {
        let error = core
            .import_private_key(NetworkId::Mainnet, again)
            .unwrap_err();
        assert!(invalid(&error, "account already exists"));
    }
}

#[test]
fn bad_keys_are_refused_without_echoing_them() {
    let (dir, _clock, core) = imported();
    let before = payload(&dir);
    let key = key_of(LEGAL, 0);
    let digits = key.trim_start_matches("0x").to_string();
    let prime = "0x800000000000011000000000000000000000000000000000000000000000001";
    // One above the prime is 1 once reduced: refused, never reinterpreted.
    let prime_plus_one = "0x800000000000011000000000000000000000000000000000000000000000002";
    let refused = [
        String::new(),
        "0x".into(),
        "0x0".into(),
        format!("0x{}", "0".repeat(70)),
        digits.clone(),
        format!("0X{digits}"),
        format!("0x {digits}"),
        format!("0x{digits}g"),
        format!("-{}", key.as_str()),
        format!("0x1{}", "0".repeat(64)),
        ORDER.into(),
        prime.into(),
        prime_plus_one.into(),
        // The decimal spelling of a key: no other base is reinterpreted.
        "123456789012345678901234567890".into(),
    ];
    for text in refused {
        let error = core
            .import_private_key(NetworkId::Mainnet, text.clone())
            .unwrap_err();
        assert!(invalid(&error, "invalid private key"));
        let shown = format!("{error} {error:?}");
        let echoed = text.trim_start_matches(['-', '0', 'x', 'X', ' ']);
        assert!(echoed.len() < 8 || !shown.contains(echoed));
    }
    assert!(payload(&dir) == before, "nothing refused was written");
}

#[test]
fn import_needs_an_unlocked_wallet_that_has_found_its_accounts() {
    let key = key_of(LEGAL, 0);
    let (_dir, _clock, fresh) = core();
    assert!(matches!(
        fresh.import_private_key(NetworkId::Mainnet, key.to_string()),
        Err(CoreError::NoWallet)
    ));
    let (_dir, _clock, waiting) = pending();
    let error = waiting
        .import_private_key(NetworkId::Mainnet, key.to_string())
        .unwrap_err();
    assert!(invalid(&error, "finish finding your accounts first"));
    let (_dir, _clock, locked) = imported();
    locked.lock();
    assert!(matches!(
        locked.import_private_key(NetworkId::Mainnet, key.to_string()),
        Err(CoreError::Locked)
    ));
}

#[test]
fn a_phrase_accounts_own_key_is_an_account_already_held() {
    let (dir, _clock, core) = imported();
    let before = payload(&dir);
    let error = core
        .import_private_key(NetworkId::Sepolia, key_of(PHRASE, 0).to_string())
        .unwrap_err();
    assert!(invalid(&error, "account already exists"));
    assert!(payload(&dir) == before);
    // On a network the phrase account is not on yet, the same key is a new account.
    let record = core
        .import_private_key(NetworkId::Mainnet, key_of(PHRASE, 0).to_string())
        .unwrap();
    assert!(record.imported);
}

/// The extension's `deriveNextFreeAccount` (Sherlock 226): an index whose account is already
/// here as an imported key is skipped, not refused forever.
#[test]
fn adding_an_account_skips_an_index_already_imported() {
    let (_dir, _clock, core) = imported();
    let held = core
        .import_private_key(NetworkId::Sepolia, key_of(PHRASE, 1).to_string())
        .unwrap();
    let added = core.add_regular_account(NetworkId::Sepolia).unwrap();
    assert_eq!(added.account_index, 2);
    assert!(!added.imported);
    assert_ne!(added.public_key, held.public_key);
}

/// `deriveNetworkAccount`: an imported key projected onto another network is the same key, and
/// its signing key is sealed again under the new record.
#[test]
fn the_network_switch_projects_an_imported_key() {
    let (dir, clock, core) = imported();
    let key = key_of(LEGAL, 0);
    let sepolia = core
        .import_private_key(NetworkId::Sepolia, key.to_string())
        .unwrap();
    let mainnet = core
        .ensure_network_account(NetworkId::Mainnet)
        .unwrap()
        .unwrap();
    assert!(mainnet.imported && mainnet.active);
    assert_eq!(mainnet.address, LEGAL_MAINNET_ADDRESS);
    assert_eq!(mainnet.public_key, sepolia.public_key);
    let vault = payload(&dir);
    let sealed: Vec<&Value> = vault["signingKeys"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|sealed| {
            sealed["accountId"] == sepolia.id.as_str() || sealed["accountId"] == mainnet.id.as_str()
        })
        .collect();
    assert!(
        sealed.len() == 2
            && sealed
                .iter()
                .all(|sealed| sealed["privateKey"] == key.as_str())
    );
    core.lock();
    let core = open(dir.path(), &clock);
    core.unlock(PASSWORD.into()).unwrap();
    let signature = core
        .sign_hash_for_account(mainnet.id, "0x1".into())
        .unwrap();
    assert_eq!((signature.r, signature.s), (LEGAL_R.into(), LEGAL_S.into()));
}

/// The extension's validator accepts an imported record of any family whose contract fields are
/// valid; mobile's adds that its public key and address are what the sealed key derives. A phrase
/// record re-tagged as imported (its sealed key is the phrase's) is such a record.
#[test]
fn an_imported_record_of_another_family_opens_when_its_sealed_key_derives_it() {
    let (dir, clock, core) = pending();
    let argent = bramble_core::account_classes()
        .into_iter()
        .find(|class| class.label == "argent 0.4.0")
        .unwrap()
        .class_hash;
    core.restore_recovered_accounts(
        NetworkId::Sepolia,
        vec![common::oz(0), own(AccountFamily::Argent, 1, Some(&argent))],
    )
    .unwrap();
    core.lock();
    let mut vault = payload(&dir);
    let account = &mut vault["accounts"][1];
    account["derivation"] = json!({ "kind": "imported-raw-key" });
    account.as_object_mut().unwrap().remove("publicKeyY");
    let id = account["id"].as_str().unwrap().to_string();
    let path = dir.path().to_string_lossy().to_string();
    seal_container_vault(&path, PASSWORD, 13, &vault.to_string()).unwrap();
    let core = open(dir.path(), &clock);
    core.unlock(PASSWORD.into()).unwrap();
    let record = record_by_id(&core, &id);
    assert!(record.imported && record.family == AccountFamily::Argent);
    let signed = core.sign_hash_for_account(id, "0x1".into()).unwrap();
    assert_eq!(signed.public_key, record.public_key);
}

#[test]
fn a_vault_whose_imported_record_does_not_match_its_sealed_key_fails_closed() {
    let (dir, clock, core) = imported();
    let key = key_of(LEGAL, 0);
    let record = core
        .import_private_key(NetworkId::Mainnet, key.to_string())
        .unwrap();
    core.lock();
    let good = payload(&dir);
    let path = dir.path().to_string_lossy().to_string();
    let unlock = |payload: &Value| {
        seal_container_vault(&path, PASSWORD, 13, &payload.to_string()).unwrap();
        open(dir.path(), &clock).unlock(PASSWORD.into())
    };
    unlock(&good).unwrap();
    let at = |vault: &Value, collection: &str, field: &str| {
        vault[collection]
            .as_array()
            .unwrap()
            .iter()
            .position(|entry| entry[field] == record.id.as_str())
            .unwrap()
    };
    let (account, sealed) = (
        at(&good, "accounts", "id"),
        at(&good, "signingKeys", "accountId"),
    );
    let phrase_account = &good["accounts"][1 - account];
    let other_key = key_of(PHRASE, 3);
    let digits = key.trim_start_matches("0x").to_string();
    let reject = |mutate: &dyn Fn(&mut Value), why: &str| {
        let mut payload = good.clone();
        mutate(&mut payload);
        let error = unlock(&payload).unwrap_err();
        assert!(matches!(error, CoreError::Storage { .. }), "{why}");
        assert!(!format!("{error} {error:?}").contains(&digits), "{why}");
    };
    reject(
        &|p| p["accounts"][account]["publicKeyX"] = phrase_account["publicKeyX"].clone(),
        "a public key the sealed key does not derive",
    );
    reject(
        &|p| p["accounts"][account]["address"] = phrase_account["address"].clone(),
        "an address the sealed key does not derive",
    );
    reject(
        &|p| p["signingKeys"][sealed]["privateKey"] = other_key.as_str().into(),
        "another valid key sealed under the record",
    );
    reject(
        &|p| p["signingKeys"][sealed]["privateKey"] = key.to_uppercase().replace("0X", "0x").into(),
        "a non-canonical sealed key",
    );
    reject(
        &|p| p["signingKeys"][sealed]["privateKey"] = format!("0x0{digits}").into(),
        "a sealed key with a leading zero",
    );
    reject(
        &|p| p["signingKeys"][sealed]["privateKey"] = ORDER.into(),
        "a sealed key at the curve order",
    );
    reject(
        &|p| {
            p["signingKeys"].as_array_mut().unwrap().remove(sealed);
        },
        "an imported record without its key",
    );
    reject(
        &|p| p["accounts"][account]["publicKeyY"] = phrase_account["publicKeyY"].clone(),
        "an imported record with a y-coordinate",
    );
    reject(
        &|p| p["accounts"][account]["derivation"]["addressIndex"] = 0.into(),
        "an imported derivation with a coordinate",
    );
    reject(
        &|p| p["accounts"][account]["deploymentSalt"] = "0x0".into(),
        "a salt on an OpenZeppelin record",
    );
    // Lifting the rule for imported keys lifts it for no other collection.
    let entry = json!({ "accountId": record.id, "id": "phrase-1", "mnemonic": LEGAL });
    for collection in [
        "accountSecrets",
        "importedSeeds",
        "multisigStrk20KeyConflicts",
        "multisigStrk20ViewingKeys",
        "originGrants",
        "shadowAccountScopes",
    ] {
        reject(
            &|p| p[collection] = json!([entry.clone()]),
            &format!("a non-empty {collection}"),
        );
    }
}

/// `deriveVaultStrk20ViewingKey`: an imported account's viewing key is derived from its sealed
/// key, and only that key reaches the discovery bodies.
#[test]
fn an_imported_accounts_viewing_key_comes_from_its_sealed_key() {
    use privacy_support::{drive, steady, wallet, wire, Script};
    let wire = wire();
    let keys = &wire["viewingKeys"];
    let (chain, pool) = (
        keys["chainId"].as_str().unwrap(),
        keys["pool"].as_str().unwrap(),
    );
    let (_dir, core) = wallet();
    let key = key_of(LEGAL, 0);
    let record = core
        .import_private_key(NetworkId::Mainnet, key.to_string())
        .unwrap();
    let secret = derive_starknet_keypair(LEGAL, 0, 0, None)
        .unwrap()
        .private_key;
    let viewing = starknet_wallet_core::derive_strk20_viewing_key(&secret, chain, pool)
        .unwrap()
        .expose_secret_hex();
    let phrase_viewing =
        bramble_core::testing::strk20_viewing_key(privacy_support::PHRASE, 0, chain, pool).unwrap();
    let plan = core.clone().prepare_privacy_read(record.id).unwrap();
    let case = &wire["cases"][0]["script"];
    let run = drive(&plan, Script::from_case(case), &mut steady);
    let bodies: Vec<String> = run
        .posts
        .iter()
        .map(|(_, body, _)| String::from_utf8(body.clone()).unwrap())
        .collect();
    assert!(bodies.iter().any(|body| body.contains(viewing.as_str())));
    assert!(bodies.iter().all(|body| !body.contains(&phrase_viewing)));
}
