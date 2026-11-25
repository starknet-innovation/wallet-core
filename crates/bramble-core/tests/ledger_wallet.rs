//! P8: a wallet made from a Ledger account alone (`createLedgerVault`), and a Ledger account
//! added to any wallet (`addLedgerAccount`). The key is the published phrase's OpenZeppelin
//! account 0, as a device at Bramble's scheme index 0 would report it in these tests, so the
//! placed address is the one the canned node knows.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

mod send_support;

use bramble_core::ledger::LedgerScheme;
use bramble_core::testing::open_vault;
use bramble_core::{
    AccountFamily, CoreError, LedgerAccountRequest, NetworkId, SendStep, StepReply,
};
use send_support::{empty_core, reviewed, run_until, sepolia, ACCOUNT_0, ACCOUNT_1, PASSWORD};
use serde_json::{json, Value};

/// The published phrase's OpenZeppelin account 0 public key.
const KEY_0: &str = "0x426212993d56613e1886a4cbc5b58810570023581c2aab0b423277776b79d2e";

fn request(family: AccountFamily, scheme: LedgerScheme, index: u32) -> LedgerAccountRequest {
    LedgerAccountRequest {
        network: NetworkId::Sepolia,
        family,
        scheme,
        account_index: index,
        public_key_x: KEY_0.into(),
    }
}

fn oz_0() -> LedgerAccountRequest {
    request(AccountFamily::OpenZeppelin, LedgerScheme::Bramble, 0)
}

fn on_disk(dir: &tempfile::TempDir) -> Value {
    serde_json::from_str(&open_vault(&dir.path().to_string_lossy(), PASSWORD).unwrap()).unwrap()
}

#[test]
fn a_ledger_wallet_has_no_phrase_and_one_device_account() {
    let (dir, _clock, core) = empty_core();
    let record = core.create_ledger_wallet(PASSWORD.into(), oz_0()).unwrap();
    assert_eq!(record.id, format!("SN_SEPOLIA:{ACCOUNT_0}"));
    assert!(record.ledger && record.active);
    assert!(!core.has_recovery_phrase().unwrap());
    assert!(!core.recovery_pending().unwrap());
    // As the extension writes it: no phrase, no key, the account at its path, no provenance.
    let vault = on_disk(&dir);
    assert_eq!(vault["mnemonic"], Value::Null);
    assert_eq!(vault["signingKeys"], json!([]));
    assert_eq!(vault["schemaVersion"], 14);
    assert!(vault.get("accountProvenance").is_none());
    assert_eq!(
        vault["accounts"][0]["derivation"],
        json!({"accountIndex": 0, "eip2645application": "LedgerW", "kind": "ledger",
            "path": "m/2645'/1195502025'/734980059'/0'/0'/0"})
    );
    // Whatever needs a phrase is refused.
    assert!(matches!(
        core.reveal_recovery_phrase(PASSWORD.into()),
        Err(CoreError::InvalidInput { .. })
    ));
    assert!(matches!(
        core.add_regular_account(NetworkId::Sepolia),
        Err(CoreError::InvalidInput { .. })
    ));
    // It opens again as it was.
    core.lock();
    core.unlock(PASSWORD.into()).unwrap();
    let records = core.all_account_records().unwrap();
    assert_eq!(records.len(), 1);
    assert!(records[0].ledger);
    assert!(matches!(
        core.create_ledger_wallet(PASSWORD.into(), oz_0()),
        Err(CoreError::WalletExists)
    ));
}

#[test]
fn a_send_from_a_ledger_wallet_asks_its_device() {
    let (_dir, _clock, core) = empty_core();
    core.create_ledger_wallet(PASSWORD.into(), oz_0()).unwrap();
    let mut node = sepolia(true);
    let plan = reviewed(&core, &mut node, ACCOUNT_1);
    let step = plan.advance(StepReply::Confirm).unwrap();
    let step = run_until(&plan, &core, &mut node, step, send_support::never);
    assert!(matches!(step, SendStep::Device { .. }), "{step:?}");
}

#[test]
fn a_ledger_account_is_added_beside_the_phrase_accounts() {
    let (_dir, core) = send_support::wallet();
    let before = core.all_account_records().unwrap().len();
    // Ready's scheme at index 3, as an Argent account.
    let added = core
        .add_ledger_account(request(AccountFamily::Argent, LedgerScheme::Ready, 3))
        .unwrap();
    assert!(added.ledger && added.active);
    assert_eq!(added.account_index, 3);
    assert_eq!(core.all_account_records().unwrap().len(), before + 1);
    assert!(core.has_recovery_phrase().unwrap());
    // The same account again is refused.
    assert!(matches!(
        core.add_ledger_account(request(AccountFamily::Argent, LedgerScheme::Ready, 3)),
        Err(CoreError::InvalidInput { .. })
    ));
}

#[test]
fn a_request_bramble_would_not_store_is_refused() {
    let (_dir, core) = send_support::wallet();
    let refused = |request| {
        matches!(
            core.add_ledger_account(request),
            Err(CoreError::InvalidInput { .. })
        )
    };
    assert!(refused(request(
        AccountFamily::Braavos,
        LedgerScheme::Ready,
        0
    )));
    assert!(refused(request(
        AccountFamily::OpenZeppelin,
        LedgerScheme::Bramble,
        1 << 31
    )));
    let mut bad_key = oz_0();
    bad_key.public_key_x = "0x0".into();
    assert!(refused(bad_key));
    let mut not_hex = oz_0();
    not_hex.public_key_x = "words".into();
    assert!(refused(not_hex));
    // The phrase wallet already holds OpenZeppelin account 0 on Sepolia: its key at the device's
    // path would be the same account.
    assert!(refused(oz_0()));
}

/// A wallet with no phrase that the extension made from keys alone (`createKeyVault`): G7's
/// imported-key vault without its phrase and phrase account. Both raw-key accounts open usable
/// and sign with their sealed keys.
#[test]
fn a_key_only_wallet_opens_and_signs_with_its_keys() {
    let path = format!(
        "{}/tests/fixtures/vault-v13-imported.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let fixture: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut payload = fixture["payload"].clone();
    let phrase_ids: Vec<Value> = payload["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|account| account["derivation"]["kind"] != "imported-raw-key")
        .map(|account| account["id"].clone())
        .collect();
    payload["accounts"]
        .as_array_mut()
        .unwrap()
        .retain(|account| !phrase_ids.contains(&account["id"]));
    payload["signingKeys"]
        .as_array_mut()
        .unwrap()
        .retain(|key| !phrase_ids.contains(&key["accountId"]));
    payload["mnemonic"] = Value::Null;
    payload["activeAccountId"] = payload["accounts"][0]["id"].clone();
    let (dir, _clock, core) = empty_core();
    bramble_core::testing::seal_container_vault(
        &dir.path().to_string_lossy(),
        PASSWORD,
        13,
        &payload.to_string(),
    )
    .unwrap();
    core.unlock(PASSWORD.into()).unwrap();
    assert!(!core.has_recovery_phrase().unwrap());
    let records = core.all_account_records().unwrap();
    assert_eq!(records.len(), 2);
    assert!(records
        .iter()
        .all(|record| record.imported && !record.ledger));
    for record in &records {
        core.sign_hash_for_account(record.id.clone(), "0x1".into())
            .unwrap();
    }
    assert!(core.unusable_accounts().unwrap().is_empty());
}

/// A wallet with no phrase and nothing mobile operates (its one Ledger account is on the
/// extension's devnet) does not wait for discovery: with no phrase there is nothing to find.
#[test]
fn a_phrase_less_wallet_never_waits_for_discovery() {
    let (dir, _clock, core) = empty_core();
    core.create_ledger_wallet(PASSWORD.into(), oz_0()).unwrap();
    let mut payload = on_disk(&dir);
    let devnet = format!("SN_DEVNET:{ACCOUNT_0}");
    payload["accounts"][0]["networkId"] = "SN_DEVNET".into();
    payload["accounts"][0]["id"] = devnet.clone().into();
    payload["activeAccountId"] = devnet.into();
    let (dir, _clock, core) = empty_core();
    bramble_core::testing::seal_container_vault(
        &dir.path().to_string_lossy(),
        PASSWORD,
        14,
        &payload.to_string(),
    )
    .unwrap();
    core.unlock(PASSWORD.into()).unwrap();
    assert!(core.all_account_records().unwrap().is_empty());
    assert!(!core.recovery_pending().unwrap());
    assert_eq!(core.unusable_accounts().unwrap().len(), 1);
}

/// What needs only an unlocked wallet works without a phrase: the address book and the relay
/// settings, as in the extension.
#[test]
fn a_ledger_wallet_keeps_contacts_and_relay_settings() {
    let (_dir, _clock, core) = empty_core();
    core.create_ledger_wallet(PASSWORD.into(), oz_0()).unwrap();
    assert!(core.contacts(NetworkId::Sepolia).unwrap().is_empty());
    core.save_contact(
        NetworkId::Sepolia,
        "Rent".into(),
        ACCOUNT_1.into(),
        None,
        None,
    )
    .unwrap();
    assert_eq!(core.contacts(NetworkId::Sepolia).unwrap().len(), 1);
    core.relay_policy().unwrap();
}
