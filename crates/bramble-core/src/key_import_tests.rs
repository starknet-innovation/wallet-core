//! An unlock that opened the vault before a write (here, an import) landed must not install its
//! stale copy over the session that made the write, nor over a disk that has moved on.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::{CoreError, NetworkId, WalletCore};

const PASSWORD: &str = "a strong password";
/// `scripts/check-krusty-adapter.ts`'s published phrase; only its key at index 0 is imported.
const LEGAL: &str = "legal winner thank year wave sausage worth useful legal winner thank yellow";

/// A host clock that never moves, so no session idles out mid-test.
struct StillClock;

impl crate::HostClock for StillClock {
    fn monotonic_ms(&self) -> u64 {
        1_000_000
    }

    fn boot_id(&self) -> String {
        "boot".into()
    }
}

/// A new wallet, and an unlock opened (KDF done) but not yet installed.
fn wallet_mid_unlock() -> (
    tempfile::TempDir,
    Arc<WalletCore>,
    u64,
    crate::vault_v13::Opened,
) {
    let dir = tempfile::tempdir().unwrap();
    let core = WalletCore::new(dir.path().to_string_lossy().into(), Arc::new(StillClock)).unwrap();
    drop(core.create_wallet(PASSWORD.into(), 12).unwrap());
    let epoch = core.lock_epoch.load(Ordering::SeqCst);
    let opened = core.passphrase_attempt(PASSWORD, Ok).unwrap();
    (dir, core, epoch, opened)
}

fn import(core: &WalletCore) -> String {
    let key = wallet_core::derive_starknet_keypair(LEGAL, 0, 0, None).unwrap();
    let key = key.private_key.expose_secret_hex().to_string();
    core.import_private_key(NetworkId::Mainnet, key).unwrap().id
}

#[test]
fn an_unlock_over_a_live_session_keeps_a_key_imported_during_its_kdf() {
    let (_dir, core, epoch, opened) = wallet_mid_unlock();
    let id = import(&core);
    core.install(epoch, opened).unwrap();
    core.add_regular_account(NetworkId::Mainnet).unwrap(); // The next vault write.
    core.lock();
    core.unlock(PASSWORD.into()).unwrap();
    let records = core.all_account_records().unwrap();
    assert!(records.iter().any(|record| record.id == id));
}

#[test]
fn an_unlock_after_idle_expiry_refuses_a_vault_that_moved_during_its_kdf() {
    let (_dir, core, epoch, opened) = wallet_mid_unlock();
    import(&core);
    *core.lock_session() = None; // Idle expiry: the session goes, the epoch stays.
    let error = core.install(epoch, opened).unwrap_err();
    assert!(matches!(error, CoreError::Storage { reason } if reason == "vault changed on disk"));
}
