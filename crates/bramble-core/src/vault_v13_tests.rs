//! `vault_v13`'s unit tests: derivations and their spellings.

use super::*;

#[test]
fn derivations_round_trip_through_ladders() {
    for (ladder, index) in [
        (Ladder::Account, 0),
        (Ladder::Account, 7),
        (Ladder::Address, 3),
        (Ladder::ArgentX, 0),
        (Ladder::ArgentX, 2),
    ] {
        let key = KeySource::Phrase(ladder);
        assert_eq!(Derivation::of(key, index).key(), Some((key, index)));
    }
    // Index 0 on the address ladder is the account ladder's index 0.
    assert_eq!(
        Derivation::of(KeySource::Phrase(Ladder::Address), 0).key(),
        Some((KeySource::Phrase(Ladder::Account), 0))
    );
    let both = Derivation::StarknetBip44 {
        account_index: 1,
        address_index: 1,
        seed_id: None,
    };
    assert_eq!(both.key(), None);
    // An imported phrase's account and an index past a `u32` are not records mobile operates.
    let seeded = Derivation::StarknetBip44 {
        account_index: 1,
        address_index: 0,
        seed_id: Some("phrase-1".into()),
    };
    assert_eq!(seeded.key(), None);
    let wide = Derivation::ArgentxBip44 {
        address_index: u64::from(u32::MAX) + 1,
        seed_id: None,
    };
    assert_eq!(wide.key(), None);
    // A Ledger account is, at its scheme's path for its index and no other.
    let ledger = |application: &str, index: u64, path: &str| Derivation::Ledger {
        account_index: index,
        eip2645_application: application.into(),
        path: path.into(),
    };
    let bramble_0 = "m/2645'/1195502025'/734980059'/0'/0'/0";
    let ready_3 = "m/2645'/1195502025'/1148870696'/0'/0'/3";
    assert_eq!(
        ledger("LedgerW", 0, bramble_0).key(),
        Some((KeySource::Ledger(LedgerScheme::Bramble), 0))
    );
    assert_eq!(
        ledger("argentx", 3, ready_3).key(),
        Some((KeySource::Ledger(LedgerScheme::Ready), 3))
    );
    assert_eq!(ledger("LedgerW", 1, bramble_0).key(), None);
    assert_eq!(ledger("argentx", 0, bramble_0).key(), None);
    assert_eq!(ledger("other", 0, bramble_0).key(), None);
    // Written back at its scheme's path.
    assert_eq!(
        Derivation::of(KeySource::Ledger(LedgerScheme::Ready), 3),
        ledger("argentx", 3, ready_3)
    );
    assert_eq!(
        Derivation::of(KeySource::Imported, 0).key(),
        Some((KeySource::Imported, 0))
    );
}

#[test]
fn derivations_keep_the_extensions_spelling() {
    let json = serde_json::to_value(Derivation::of(KeySource::Phrase(Ladder::Address), 1)).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"kind": "starknet-bip44", "accountIndex": 0, "addressIndex": 1})
    );
    let json = serde_json::to_value(Derivation::of(KeySource::Phrase(Ladder::ArgentX), 4)).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"kind": "argentx-bip44", "addressIndex": 4})
    );
    let json = serde_json::to_value(Derivation::of(KeySource::Imported, 0)).unwrap();
    assert_eq!(json, serde_json::json!({"kind": "imported-raw-key"}));
    let imported: Derivation =
        serde_json::from_value(serde_json::json!({"kind": "imported-raw-key"})).unwrap();
    assert_eq!(imported, Derivation::ImportedRawKey {});
    let ledger = serde_json::json!({
        "accountIndex": 3,
        "eip2645application": "argentx",
        "kind": "ledger",
        "path": "m/2645'/1195502025'/1148870696'/0'/0'/3",
    });
    let read: Derivation = serde_json::from_value(ledger.clone()).unwrap();
    assert_eq!(serde_json::to_value(read).unwrap(), ledger);
    let seeded =
        serde_json::json!({"kind": "argentx-bip44", "addressIndex": 2, "seedId": "phrase-3"});
    let read: Derivation = serde_json::from_value(seeded.clone()).unwrap();
    assert_eq!(serde_json::to_value(read).unwrap(), seeded);
    for refused in [
        serde_json::json!({"kind": "ledger", "accountIndex": 0}),
        serde_json::json!({"kind": "starknet-bip44", "accountIndex": 0, "addressIndex": 0, "seedId": null}),
        serde_json::json!({"kind": "starknet-bip44", "accountIndex": 1.0, "addressIndex": 0}),
        serde_json::json!({"kind": "starknet-bip44", "accountIndex": -1, "addressIndex": 0}),
        serde_json::json!({"kind": "imported-raw-key", "addressIndex": 0}),
        serde_json::json!({"kind": "imported-raw-key", "seedId": "phrase-1"}),
        serde_json::json!({"kind": "argentx-bip44", "addressIndex": 0, "extra": 1}),
        serde_json::json!({"kind": "starknet-bip44", "accountIndex": 0}),
    ] {
        assert!(serde_json::from_value::<Derivation>(refused).is_err());
    }
}

/// An optional key is absent or holds a value: `null` is refused where the extension's
/// checks refuse it (`deploymentSalt === undefined`, `hasOnlyKnownKeys` for `publicKeyY`), and
/// the two keys that may be `null` must still be present.
#[test]
fn optional_keys_are_absent_never_null() {
    let account = serde_json::json!({
        "accountContractType": "openzeppelin",
        "address": "0x1",
        "classHash": "0x2",
        "derivation": {"kind": "imported-raw-key"},
        "id": "SN_MAIN:0x1",
        "networkId": "SN_MAIN",
        "publicKeyX": "0x3",
    });
    assert!(serde_json::from_value::<Account>(account.clone()).is_ok());
    for key in ["deploymentSalt", "publicKeyY"] {
        let mut nulled = account.clone();
        nulled[key] = serde_json::Value::Null;
        assert!(serde_json::from_value::<Account>(nulled).is_err(), "{key}");
    }
    let vault = serde_json::json!({
        "accounts": [], "accountSecrets": [], "activeAccountId": null, "importedSeeds": [],
        "mnemonic": null, "multisigStrk20KeyConflicts": [], "multisigStrk20ViewingKeys": [],
        "originGrants": [], "schemaVersion": 13, "shadowAccountScopes": [], "signingKeys": [],
    });
    assert!(serde_json::from_value::<Vault>(vault.clone()).is_ok());
    for key in ["activeAccountId", "mnemonic", "originGrants"] {
        let mut missing = vault.clone();
        missing.as_object_mut().unwrap().remove(key);
        assert!(serde_json::from_value::<Vault>(missing).is_err(), "{key}");
    }
}
