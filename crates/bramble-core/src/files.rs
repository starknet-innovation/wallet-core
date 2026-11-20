//! Durable file writes for the vault directory.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::Path;

use crate::CoreError;

pub(crate) fn storage(reason: &str) -> CoreError {
    CoreError::Storage {
        reason: reason.into(),
    }
}

/// The write itself; the error is the fixed name of the step that failed.
fn commit(dir: &Path, name: &str, body: &[u8]) -> Result<(), &'static str> {
    let temporary = dir.join(format!("{name}.tmp"));
    let mut file = fs::File::create(&temporary).map_err(|_| "create file")?;
    file.write_all(body)
        .and_then(|()| file.sync_all())
        .map_err(|_| "write file")?;
    fs::rename(&temporary, dir.join(name)).map_err(|_| "commit file")?;
    sync_directory(dir)
}

/// Syncs `dir` so a rename within it has reached the disk too. The rename has already happened
/// when this fails, so the error means "maybe not durable", never "nothing changed".
#[cfg(unix)]
fn sync_directory(dir: &Path) -> Result<(), &'static str> {
    fs::File::open(dir)
        .and_then(|dir| dir.sync_all())
        .map_err(|_| "sync directory")
}

/// Windows cannot open a directory as a file; no target ships there, so only the build is kept.
#[cfg(not(unix))]
fn sync_directory(_: &Path) -> Result<(), &'static str> {
    Ok(())
}

/// Writes `body` to `dir/name` through a synced temporary file, a rename and a sync of `dir`, so
/// an interrupted write never leaves a truncated file and a completed one has reached the disk.
/// An error after the rename leaves the new file in place: callers must be safe to retry.
pub(crate) fn write_atomic(dir: &Path, name: &str, body: &[u8]) -> Result<(), CoreError> {
    commit(dir, name, body).map_err(storage)
}

/// The wallet directory as the shared core's `Storage` (the vault and the activity log): a key is a
/// file name in it, a write is `write_atomic`'s, and a rename is `fs::rename` within it followed by a
/// sync of the directory.
pub(crate) struct Directory<'a>(pub &'a Path);

impl wallet_core::host::Storage for Directory<'_> {
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, wallet_core::Error> {
        match fs::read(self.0.join(key)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(_) => Err(wallet_core::Error::storage("read vault")),
        }
    }

    fn write(&self, key: &str, value: &[u8]) -> Result<(), wallet_core::Error> {
        commit(self.0, key, value).map_err(wallet_core::Error::storage)
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), wallet_core::Error> {
        fs::rename(self.0.join(from), self.0.join(to))
            .map_err(|_| "rename file")
            .and_then(|()| sync_directory(self.0))
            .map_err(wallet_core::Error::storage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wallet_core::host::Storage;

    #[test]
    fn writes_and_renames_round_trip_and_leave_no_temporary() {
        let dir = tempfile::tempdir().unwrap();
        write_atomic(dir.path(), "a.json", b"one").unwrap();
        write_atomic(dir.path(), "a.json", b"two").unwrap();
        assert_eq!(fs::read(dir.path().join("a.json")).unwrap(), b"two");
        assert!(!dir.path().join("a.json.tmp").exists());
        let storage = Directory(dir.path());
        storage.write("b.json", b"three").unwrap();
        storage.rename("b.json", "c.json").unwrap();
        assert_eq!(storage.read("b.json").unwrap(), None);
        assert_eq!(storage.read("c.json").unwrap().unwrap(), b"three");
    }

    // A directory without read permission still takes the temporary file and the rename, but
    // cannot be opened to sync: the write is reported failed with the new file in place.
    #[cfg(unix)]
    #[test]
    fn a_directory_that_cannot_be_synced_fails_the_write() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        fs::set_permissions(path, fs::Permissions::from_mode(0o300)).unwrap();
        let unreadable = fs::File::open(path).is_err(); // root reads it anyway
        let outcome = commit(path, "a.json", b"body");
        let renamed = Directory(path).rename("a.json", "b.json");
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        if unreadable {
            assert_eq!(outcome, Err("sync directory"));
            assert!(renamed.is_err());
            assert_eq!(fs::read(path.join("b.json")).unwrap(), b"body");
        }
    }
}
