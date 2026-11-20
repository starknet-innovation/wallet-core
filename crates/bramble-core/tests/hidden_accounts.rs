//! Hidden accounts (`src/account_visibility.rs`), as the extension keeps them: `hiddenAccountIds`
//! in the preferences, never the vault; the last account a network shows cannot be hidden; hiding
//! the active account moves to a visible one first.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

mod common;

use bramble_core::{CoreError, NetworkId};
use common::{account_id, open, oz, pending, PASSWORD};
use serde_json::Value;

fn hidden_of(core: &bramble_core::WalletCore, network: NetworkId) -> Vec<bool> {
    core.account_records(network)
        .unwrap()
        .iter()
        .map(|record| record.hidden)
        .collect()
}

fn preferences(dir: &std::path::Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join("preferences.json")).unwrap()).unwrap()
}

#[test]
fn a_hidden_account_is_a_preference_kept_across_restart() {
    let (dir, clock, core) = pending();
    let restored = core
        .restore_recovered_accounts(NetworkId::Sepolia, vec![oz(0), oz(1)])
        .unwrap();
    let second = restored[1].id.clone();
    assert_eq!(hidden_of(&core, NetworkId::Sepolia), [false, false]);
    core.set_account_hidden(second.clone(), true).unwrap();
    core.set_account_hidden(second.clone(), true).unwrap();
    // A stale id is cleared without complaint, as the extension's reveal.
    core.set_account_hidden("SN_SEPOLIA:0x1".into(), false)
        .unwrap();
    assert_eq!(
        preferences(dir.path())["hiddenAccountIds"],
        serde_json::json!([second])
    );

    core.lock();
    let reopened = open(dir.path(), &clock);
    reopened.unlock(PASSWORD.into()).unwrap();
    assert_eq!(hidden_of(&reopened, NetworkId::Sepolia), [false, true]);
    // Hiding is a view filter: the account can still be selected.
    reopened.select_account(second.clone()).unwrap();
    assert_eq!(account_id(&reopened), second);
    reopened.set_account_hidden(second, false).unwrap();
    assert_eq!(hidden_of(&reopened, NetworkId::Sepolia), [false, false]);
}

#[test]
fn the_last_account_a_network_shows_cannot_be_hidden() {
    let (dir, _clock, core) = pending();
    let restored = core
        .restore_recovered_accounts(NetworkId::Sepolia, vec![oz(0), oz(1)])
        .unwrap();
    let (first, second) = (restored[0].id.clone(), restored[1].id.clone());
    core.set_account_hidden(second, true).unwrap();
    let refused =
        |result: Result<(), CoreError>| matches!(result, Err(CoreError::InvalidInput { .. }));
    assert!(refused(core.set_account_hidden(first.clone(), true)));
    assert!(refused(
        core.set_account_hidden("SN_SEPOLIA:0x1".into(), true)
    ));
    // Per network: Sepolia still shows one, but Mainnet's only account stays visible.
    let mainnet = core
        .ensure_network_account(NetworkId::Mainnet)
        .unwrap()
        .unwrap()
        .id;
    assert!(refused(core.set_account_hidden(mainnet, true)));
    assert_eq!(hidden_of(&core, NetworkId::Sepolia), [false, true]);
    assert_eq!(hidden_of(&core, NetworkId::Mainnet), [false]);
    assert_eq!(
        preferences(dir.path())["hiddenAccountIds"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn hiding_the_active_account_selects_a_visible_one_first() {
    let (dir, clock, core) = pending();
    let restored = core
        .restore_recovered_accounts(NetworkId::Sepolia, vec![oz(0), oz(1), oz(2)])
        .unwrap();
    let ids: Vec<String> = restored.iter().map(|record| record.id.clone()).collect();
    core.set_account_hidden(ids[1].clone(), true).unwrap();
    core.set_account_hidden(ids[0].clone(), true).unwrap();
    assert_eq!(
        account_id(&core),
        ids[2],
        "the first visible, skipping the hidden one"
    );

    // The selection is written before the hide: a failed preferences write leaves the wallet on
    // the successor with the account still shown, never on an account no list renders.
    core.set_account_hidden(ids[0].clone(), false).unwrap();
    core.select_account(ids[0].clone()).unwrap();
    std::fs::create_dir(dir.path().join("preferences.json.tmp")).unwrap();
    assert!(core.set_account_hidden(ids[0].clone(), true).is_err());
    assert_eq!(account_id(&core), ids[2]);
    assert_eq!(hidden_of(&core, NetworkId::Sepolia), [false, true, false]);
    std::fs::remove_dir(dir.path().join("preferences.json.tmp")).unwrap();
    core.lock();
    let reopened = open(dir.path(), &clock);
    reopened.unlock(PASSWORD.into()).unwrap();
    assert_eq!(account_id(&reopened), ids[2]);
    assert_eq!(
        hidden_of(&reopened, NetworkId::Sepolia),
        [false, true, false]
    );
}

#[test]
fn the_network_switch_lands_on_a_visible_account() {
    let (_dir, _clock, core) = pending();
    let restored = core
        .restore_recovered_accounts(NetworkId::Sepolia, vec![oz(0), oz(1)])
        .unwrap();
    core.ensure_network_account(NetworkId::Mainnet).unwrap();
    core.set_account_hidden(restored[0].id.clone(), true)
        .unwrap();
    let landed = core
        .ensure_network_account(NetworkId::Sepolia)
        .unwrap()
        .unwrap();
    assert_eq!(landed.id, restored[1].id);
}

#[test]
fn hidden_ids_drop_unknown_accounts_and_refuse_a_repeat() {
    let (dir, clock, core) = pending();
    let id = core
        .restore_recovered_accounts(NetworkId::Sepolia, vec![oz(0)])
        .unwrap()[0]
        .id
        .clone();
    core.lock();
    let write = |hidden: Value| {
        let preferences = serde_json::json!({ "backupPending": false, "version": 1,
            "groups": [], "labels": {}, "hiddenAccountIds": hidden });
        std::fs::write(dir.path().join("preferences.json"), preferences.to_string()).unwrap();
        let fresh = open(dir.path(), &clock);
        fresh.unlock(PASSWORD.into()).map(|()| fresh)
    };
    let fresh = write(serde_json::json!(["SN_SEPOLIA:0x1"])).unwrap();
    assert_eq!(hidden_of(&fresh, NetworkId::Sepolia), [false]);
    fresh.lock();
    assert!(matches!(
        write(serde_json::json!([id, id])),
        Err(CoreError::Storage { .. })
    ));
}
