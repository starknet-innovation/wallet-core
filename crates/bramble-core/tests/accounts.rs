//! Account management through the public core surface: signing by record (a shared key at one
//! index signs every family there; the send plan decides which account may use it), the network
//! switch, adding accounts, labels, groups, and fail-closed handling of a tampered vault. The
//! extension's own v13 vault is `vault_v13.rs`.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

mod common;

use bramble_core::testing::{open_vault, seal_container_vault};
use bramble_core::{account_classes, AccountFamily, CoreError, NetworkId};
use common::{open, own, oz, pending, selection, PASSWORD};
use serde_json::Value;

fn class_of(label: &str) -> String {
    account_classes()
        .into_iter()
        .find(|class| class.label == label)
        .unwrap()
        .class_hash
}

#[test]
fn every_record_signs_through_one_gate_with_its_index_key() {
    let (_dir, _clock, core) = pending();
    assert!(
        core.sign_hash_for_account("SN_SEPOLIA:0x1".into(), "0x1".into())
            .is_err(),
        "a pending vault signs nothing"
    );

    let braavos_class = class_of("braavos 1.0.0");
    let argent_class = class_of("argent 0.4.0");
    let restored = core
        .restore_recovered_accounts(
            NetworkId::Sepolia,
            vec![
                oz(0),
                selection(AccountFamily::Braavos, 0, Some(&braavos_class)),
                selection(AccountFamily::Braavos, 1, Some(&braavos_class)),
                own(AccountFamily::Argent, 0, Some(&argent_class)),
                selection(AccountFamily::Argent, 0, Some(&argent_class)),
            ],
        )
        .unwrap();
    let (ozr, braavos0, argent0, ready0) = (&restored[0], &restored[1], &restored[3], &restored[4]);
    assert_eq!(
        ozr.public_key, braavos0.public_key,
        "index 0 is the same key on this wallet's ladder and Braavos's"
    );
    assert_ne!(
        ready0.public_key, ozr.public_key,
        "Ready's ETH-seeded ladder is a different key (issue 505)"
    );
    assert_ne!(
        restored[2].public_key, ozr.public_key,
        "Braavos's index 1 is on its own address ladder"
    );
    assert_eq!(braavos0.type_label.as_deref(), Some("Braavos Account"));

    let signed = core
        .sign_hash_for_account(ozr.id.clone(), "0x1".into())
        .unwrap();
    assert_eq!(signed.public_key, ozr.public_key);
    // Braavos signs only inside a send plan, after its live class and signers are checked
    // (wallet-core's `transactions_plan_parity.rs`); a raw hash would skip those checks.
    assert!(matches!(
        core.sign_hash_for_account(braavos0.id.clone(), "0x1".into()),
        Err(CoreError::InvalidInput { .. })
    ));
    assert!(matches!(
        core.sign_hash_for_account("SN_SEPOLIA:0x1".into(), "0x1".into()),
        Err(CoreError::InvalidInput { .. })
    ));
    // Argent on this wallet's ladder shares index 0's key too and passes the same gate: the plan
    // wraps its signature in the `argent-signer-array` layout (wallet-core's plan parity).
    assert_eq!(argent0.public_key, ozr.public_key);
    assert_eq!(argent0.authorization_shape, "argent-signer-array");
    let ready = core
        .sign_hash_for_account(argent0.id.clone(), "0x1".into())
        .unwrap();
    assert_eq!(
        ready, signed,
        "one key per index; the record only chooses the layout"
    );
    // The deprecated OZ-only view never lists the Braavos records.
    let legacy = core.accounts(NetworkId::Sepolia).unwrap();
    assert_eq!(legacy.len(), 1);
    assert_eq!(legacy[0].address, ozr.address);
}

#[test]
fn switching_network_keeps_picks_or_projects_like_the_extension() {
    let (dir, clock, core) = pending();
    let argent_031 = class_of("argent 0.3.1");
    let braavos_base = class_of("braavos base");
    let restored = core
        .restore_recovered_accounts(
            NetworkId::Sepolia,
            vec![
                oz(0),
                selection(AccountFamily::Argent, 2, Some(&argent_031)),
                selection(AccountFamily::Braavos, 3, Some(&braavos_base)),
            ],
        )
        .unwrap();

    // Active is on Sepolia: kept.
    let kept = core
        .ensure_network_account(NetworkId::Sepolia)
        .unwrap()
        .unwrap();
    assert_eq!(kept.id, restored[0].id);

    // Argent 0.3.1 projects with its class and salt kept (same address on both chains).
    core.select_account(restored[1].id.clone()).unwrap();
    let argent = core
        .ensure_network_account(NetworkId::Mainnet)
        .unwrap()
        .unwrap();
    assert_eq!(argent.network, NetworkId::Mainnet);
    assert_eq!(argent.family, AccountFamily::Argent);
    assert_eq!(argent.class_hash, restored[1].class_hash);
    assert_eq!(argent.deployment_salt, restored[1].deployment_salt);
    assert_eq!(argent.address, restored[1].address);
    assert!(argent.active);

    // Back on Sepolia the first Sepolia record is picked, not projected again.
    let picked = core
        .ensure_network_account(NetworkId::Sepolia)
        .unwrap()
        .unwrap();
    assert_eq!(picked.id, restored[0].id);

    // A Mainnet record exists now, so switching back picks it rather than projecting OZ.
    let mainnet = core
        .ensure_network_account(NetworkId::Mainnet)
        .unwrap()
        .unwrap();
    assert_eq!(mainnet.id, argent.id);
    assert_eq!(core.account_records(NetworkId::Mainnet).unwrap().len(), 1);

    // Braavos projects too.
    let (_dir2, _clock2, other) = pending();
    other
        .restore_recovered_accounts(
            NetworkId::Sepolia,
            vec![selection(AccountFamily::Braavos, 3, Some(&braavos_base))],
        )
        .unwrap();
    let projected = other
        .ensure_network_account(NetworkId::Mainnet)
        .unwrap()
        .unwrap();
    assert_eq!(projected.family, AccountFamily::Braavos);
    assert_eq!(projected.class_hash, restored[2].class_hash);

    // Projection persists across lock and unlock.
    core.lock();
    let reopened = open(dir.path(), &clock);
    reopened.unlock(PASSWORD.into()).unwrap();
    assert_eq!(reopened.active_account().unwrap().unwrap().id, argent.id);
}

#[test]
fn oz_projects_with_the_other_networks_class() {
    let (_dir, _clock, core) = common::core();
    core.create_wallet_on_network(PASSWORD.into(), 12, NetworkId::Sepolia)
        .unwrap();
    let sepolia = core.active_account().unwrap().unwrap();
    let mainnet = core
        .ensure_network_account(NetworkId::Mainnet)
        .unwrap()
        .unwrap();
    assert_eq!(mainnet.family, AccountFamily::OpenZeppelin);
    assert_eq!(mainnet.account_index, 0);
    assert_eq!(mainnet.public_key, sepolia.public_key);
    assert_eq!(mainnet.id, format!("SN_MAIN:{}", mainnet.address));
    assert_eq!(mainnet.deployment_salt, None);
}

#[test]
fn regular_accounts_take_the_lowest_free_index_and_become_active() {
    let (_dir, _clock, core) = pending();
    let argent = class_of("argent 0.4.0");
    core.restore_recovered_accounts(
        NetworkId::Sepolia,
        vec![
            oz(0),
            own(AccountFamily::Argent, 1, Some(&argent)),
            // Ready's ladder is a separate ETH-seeded tree: its index 2 holds no index here.
            selection(AccountFamily::Argent, 2, Some(&argent)),
            oz(3),
        ],
    )
    .unwrap();
    let added = core.add_regular_account(NetworkId::Sepolia).unwrap();
    assert_eq!(
        added.account_index, 2,
        "index 1 is held by the own-ladder Argent account; Ready's index 2 is not"
    );
    assert!(added.active);
    assert_eq!(added.family, AccountFamily::OpenZeppelin);
    assert_eq!(
        added.type_label.as_deref(),
        Some("Bramble Account (OpenZeppelin AccountUpgradeable)")
    );
    let next = core.add_regular_account(NetworkId::Sepolia).unwrap();
    assert_eq!(next.account_index, 4);
    assert_eq!(
        core.add_regular_account(NetworkId::Mainnet)
            .unwrap()
            .account_index,
        0
    );
}

#[test]
fn labels_and_groups_are_kept_and_validated() {
    let (dir, clock, core) = pending();
    let restored = core
        .restore_recovered_accounts(NetworkId::Sepolia, vec![oz(0), oz(1)])
        .unwrap();
    let id = restored[0].id.clone();
    assert_eq!(
        restored[0].display_name,
        bramble_core::friendly_account_name(restored[0].address.clone())
    );
    assert_eq!(
        core.set_account_label(id.clone(), "  Savings ".into())
            .unwrap()
            .as_deref(),
        Some("Savings")
    );
    assert!(core.set_account_label(id.clone(), "x".repeat(65)).is_err());
    assert!(core
        .set_account_label("SN_MAIN:0x1".into(), "Other".into())
        .is_err());

    let group = core
        .create_account_group("Long term".into(), "piggyBank".into())
        .unwrap();
    assert_eq!(group.id, "long-term");
    core.set_account_group(id.clone(), Some(group.id.clone()))
        .unwrap();
    let renamed = core
        .update_account_group(group.id.clone(), "Savings pot".into(), "coins".into())
        .unwrap();
    assert_eq!(
        (renamed.id.as_str(), renamed.icon.as_str()),
        ("long-term", "coins")
    );

    core.lock();
    let reopened = open(dir.path(), &clock);
    reopened.unlock(PASSWORD.into()).unwrap();
    let record = &reopened.account_records(NetworkId::Sepolia).unwrap()[0];
    assert_eq!(record.label.as_deref(), Some("Savings"));
    assert_eq!(record.display_name, "Savings");
    assert_eq!(record.group_id.as_deref(), Some("long-term"));
    // The extension keeps both in its preferences, not its vault; so does mobile.
    let path = dir.path().to_string_lossy().to_string();
    assert!(!open_vault(&path, PASSWORD).unwrap().contains("Savings"));
    let preferences: Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join("preferences.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(preferences["labels"][&id], "Savings");

    reopened
        .set_account_label(id.clone(), "   ".into())
        .unwrap();
    reopened.delete_account_group("long-term".into()).unwrap();
    let record = &reopened.account_records(NetworkId::Sepolia).unwrap()[0];
    assert_eq!(record.label, None);
    assert_eq!(record.group_id, None);
}

#[test]
fn preferences_drop_unknown_accounts_and_refuse_anything_else_invalid() {
    let (dir, clock, core) = pending();
    let id = core
        .restore_recovered_accounts(NetworkId::Sepolia, vec![oz(0)])
        .unwrap()[0]
        .id
        .clone();
    core.lock();
    let write = |preferences: Value| {
        std::fs::write(dir.path().join("preferences.json"), preferences.to_string()).unwrap();
        let fresh = open(dir.path(), &clock);
        fresh.unlock(PASSWORD.into()).map(|()| fresh)
    };
    // The vault and the preferences are written one after the other: a name for an account the
    // vault does not hold is dropped, not fatal.
    let fresh = write(serde_json::json!({
        "backupPending": false, "version": 1,
        "labels": { "SN_SEPOLIA:0x1": "x" },
        "groups": [{ "accountIds": ["SN_SEPOLIA:0x1"], "icon": "folder", "id": "a", "name": "A" }],
    }))
    .unwrap();
    let record = &fresh.account_records(NetworkId::Sepolia).unwrap()[0];
    assert_eq!(
        (record.label.as_ref(), record.group_id.as_ref()),
        (None, None)
    );
    assert_eq!(fresh.account_groups().unwrap()[0].account_ids.len(), 0);
    for (preferences, why) in [
        (
            serde_json::json!({ "backupPending": false, "version": 1, "labels": {},
                "groups": [
                    { "accountIds": [id], "icon": "folder", "id": "a", "name": "A" },
                    { "accountIds": [id], "icon": "star", "id": "b", "name": "B" }
                ] }),
            "one account in two groups",
        ),
        (
            serde_json::json!({ "backupPending": false, "version": 1, "groups": [],
                "labels": { id.clone(): " padded " } }),
            "an untrimmed label",
        ),
        (
            serde_json::json!({ "backupPending": false, "version": 2, "groups": [], "labels": {} }),
            "another version",
        ),
        (
            serde_json::json!({ "backupPending": false, "version": 1, "groups": [], "labels": {},
                "extra": 1 }),
            "an unknown field",
        ),
    ] {
        assert!(
            matches!(write(preferences), Err(CoreError::Storage { .. })),
            "{why}"
        );
    }
}

#[test]
fn a_tampered_or_malformed_vault_fails_closed() {
    let (dir, clock, core) = pending();
    let argent = class_of("argent 0.4.0");
    let accounts = vec![oz(0), own(AccountFamily::Argent, 1, Some(&argent))];
    core.restore_recovered_accounts(NetworkId::Sepolia, accounts)
        .unwrap();
    core.lock();
    let path = dir.path().to_string_lossy().to_string();
    let good: Value = serde_json::from_str(&open_vault(&path, PASSWORD).unwrap()).unwrap();

    let unlock = |payload: &Value, version: u32| {
        seal_container_vault(&path, PASSWORD, version, &payload.to_string()).unwrap();
        open(dir.path(), &clock).unlock(PASSWORD.into())
    };
    // The round trip itself opens: the cases below each fail for their own reason.
    unlock(&good, 13).unwrap();
    let reject = |mutate: &dyn Fn(&mut Value), why: &str| {
        let mut payload = good.clone();
        mutate(&mut payload);
        assert!(
            matches!(unlock(&payload, 13), Err(CoreError::Storage { .. })),
            "{why}"
        );
    };
    let other = "0x2cd1d9d3c7ab3b52b2b61327e14c5cfa4e9f2ec5d2de8ac6e7a3c1c1c7e0e4b";
    reject(
        &|p| p["accounts"][0]["address"] = other.into(),
        "address that does not re-derive",
    );
    reject(
        &|p| p["accounts"][0]["id"] = "SN_MAIN:0x1".into(),
        "id not network:address",
    );
    reject(
        &|p| p["accounts"][0]["deploymentSalt"] = "0x0".into(),
        "salt on OZ",
    );
    reject(
        &|p| {
            p["accounts"][1]
                .as_object_mut()
                .unwrap()
                .remove("deploymentSalt");
        },
        "Argent without its salt",
    );
    reject(
        &|p| p["accounts"][0]["accountContractType"] = "braavos".into(),
        "family swapped",
    );
    reject(
        &|p| p["accounts"][0]["publicKeyY"] = other.into(),
        "a y-coordinate that does not re-derive",
    );
    reject(
        &|p| {
            p["accounts"][0]
                .as_object_mut()
                .unwrap()
                .remove("publicKeyY");
        },
        "starknet-bip44 without its y-coordinate",
    );
    reject(
        &|p| p["accounts"][0]["derivation"] = serde_json::json!({"kind": "imported-raw-key"}),
        "a derivation mobile does not hold",
    );
    reject(
        &|p| p["activeAccountId"] = "SN_SEPOLIA:0x1".into(),
        "unknown active account",
    );
    reject(&|p| p["activeAccountId"] = Value::Null, "no active account");
    reject(&|p| p["mnemonic"] = Value::Null, "no phrase");
    reject(
        &|p| p["signingKeys"][0]["privateKey"] = "0x1".into(),
        "a signing key the phrase does not derive",
    );
    reject(
        &|p| {
            let key = p["signingKeys"][0]["privateKey"]
                .as_str()
                .unwrap()
                .to_uppercase();
            p["signingKeys"][0]["privateKey"] = key.replace("0X", "0x").into();
        },
        "a non-canonical signing key",
    );
    reject(
        &|p| {
            p["signingKeys"].as_array_mut().unwrap().pop();
        },
        "an account without its signing key",
    );
    reject(
        &|p| {
            let spare = p["signingKeys"][0].clone();
            p["signingKeys"].as_array_mut().unwrap().push(spare);
        },
        "a spare signing key",
    );
    reject(
        &|p| {
            let id = p["accounts"][0]["id"].clone();
            p["originGrants"] =
                serde_json::json!([{ "accountIds": [id], "origin": "https://example.com" }]);
        },
        "an origin grant",
    );
    reject(&|p| p["extra"] = 1.into(), "unknown field");
    reject(&|p| p["schemaVersion"] = 12.into(), "another schema");
    // A v13 payload in an earlier build's container is refused too.
    assert!(matches!(unlock(&good, 5), Err(CoreError::Storage { .. })));
}
