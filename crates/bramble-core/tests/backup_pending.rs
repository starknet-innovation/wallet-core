//! The backup-pending flag (`src/backup.rs`, kept in `preferences.json`): set when a wallet is
//! created, kept across lock and restart, cleared only by `confirm_backup`; never set by an
//! import. And a vault an earlier test build wrote, a real file from the version-3 core: refused
//! once the password is proven, and still deletable.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

use crate::common;

use bramble_core::testing::seal_envelope_vault;
use bramble_core::{CoreError, NetworkId, WalletStatus};
use common::{core, open, pending, FakeClock, PASSWORD};
use serde_json::Value;

/// Written by the version-3 core (88666b49): the published phrase imported, OZ 0 and 1 restored
/// on Sepolia, and a biometric slot under the wrap secret `[7; 32]` with no domain. Public test
/// data.
const V3_FIXTURE: &str = include_str!("regression/vault-v3-container.json");
const V3_PASSWORD: &str = "v3 fixture passphrase";
const V3_WRAP_SECRET: [u8; 32] = [7; 32];

fn read(dir: &tempfile::TempDir) -> Value {
    serde_json::from_str(&std::fs::read_to_string(dir.path().join("vault.json")).unwrap()).unwrap()
}

fn preferences(dir: &tempfile::TempDir) -> Value {
    let text = std::fs::read_to_string(dir.path().join("preferences.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

#[test]
fn a_created_wallet_stays_backup_pending_until_confirmed() {
    let (dir, clock, core) = core();
    let words = core
        .create_wallet_on_network(PASSWORD.into(), 12, NetworkId::Sepolia)
        .unwrap()
        .words;
    assert!(core.backup_pending().unwrap());
    assert_eq!(preferences(&dir)["backupPending"], true);

    // A lock (or a restart) before Confirm backup leaves it owed.
    core.lock();
    assert!(matches!(core.backup_pending(), Err(CoreError::Locked)));
    assert!(matches!(core.confirm_backup(), Err(CoreError::Locked)));
    let restarted = open(dir.path(), &clock);
    restarted.unlock(PASSWORD.into()).unwrap();
    assert!(restarted.backup_pending().unwrap());
    // The words come back only through the password-checked path.
    assert_eq!(
        restarted
            .reveal_recovery_phrase(PASSWORD.into())
            .unwrap()
            .words,
        words
    );

    let sealed = read(&dir);
    restarted.confirm_backup().unwrap();
    assert!(!restarted.backup_pending().unwrap());
    // Only the preferences moved, so only they are written: the vault is byte for byte the same.
    assert_eq!(read(&dir), sealed);
    restarted.confirm_backup().unwrap(); // idempotent
    assert_eq!(preferences(&dir)["backupPending"], false);
    restarted.lock();
    let again = open(dir.path(), &clock);
    again.unlock(PASSWORD.into()).unwrap();
    assert!(!again.backup_pending().unwrap(), "cleared for good");
    assert_eq!(read(&dir)["version"], 14);
}

#[test]
fn an_imported_wallet_is_never_backup_pending() {
    let (dir, _clock, core) = pending();
    assert!(!core.backup_pending().unwrap());
    assert_eq!(preferences(&dir)["backupPending"], false);
}

#[test]
fn no_wallet_has_no_backup_state() {
    let (_dir, _clock, core) = core();
    assert!(matches!(core.backup_pending(), Err(CoreError::NoWallet)));
}

#[test]
fn an_earlier_builds_vault_is_refused_but_can_be_deleted() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("vault.json"), V3_FIXTURE).unwrap();
    let before = read(&dir);
    assert_eq!(before["version"], 3);
    let core = open(dir.path(), &FakeClock::new());
    assert_eq!(core.status(), WalletStatus::Locked);
    // A wrong password still reads as one, and counts.
    assert!(matches!(
        core.unlock("not the password".into()),
        Err(CoreError::WrongPassword)
    ));
    assert_eq!(core.security_status().failed_attempts, 1);
    // The right one proves it, resets the counter, and the vault is refused untouched.
    assert!(matches!(
        core.unlock(V3_PASSWORD.into()),
        Err(CoreError::Storage { .. })
    ));
    assert_eq!(core.security_status().failed_attempts, 0);
    assert!(matches!(
        core.unlock_with_biometric(V3_WRAP_SECRET.to_vec()),
        Err(CoreError::Storage { .. })
    ));
    assert_eq!(read(&dir), before);
    core.delete_wallet(V3_PASSWORD.into()).unwrap();
    assert_eq!(core.status(), WalletStatus::NoWallet);
}

#[test]
fn a_pre_container_vault_is_refused_but_can_be_deleted() {
    let (dir, _clock, core) = core();
    let path = dir.path().to_string_lossy().to_string();
    seal_envelope_vault(&path, PASSWORD, "{\"version\":1}").unwrap();
    assert!(matches!(
        core.unlock("not the password".into()),
        Err(CoreError::WrongPassword)
    ));
    assert!(matches!(
        core.unlock(PASSWORD.into()),
        Err(CoreError::Storage { .. })
    ));
    core.delete_wallet(PASSWORD.into()).unwrap();
    assert_eq!(core.status(), WalletStatus::NoWallet);
}
