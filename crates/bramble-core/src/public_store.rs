//! Public, non-secret wallet metadata files kept beside `vault.json`: contacts, shared accounts,
//! coordination checkpoints, and the wallet's preferences (labels, groups, hidden accounts, the
//! backup flag). They inherit the data directory's protection (iOS complete file protection and
//! backup exclusion; Android `noBackupFilesDir`).
//!
//! Every file is `{"version":1, …}`, written with the vault's temp-file, `sync_all`, rename
//! sequence, so an interrupted write never leaves a truncated file. Reading fails closed: an
//! unreadable or invalid file is an error that names the file, never silently reset to empty.

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::account_store::{self, VaultPayload};
use crate::groups::StoredGroup;
use crate::CoreError;

/// The contacts file (the extension's address book).
pub const CONTACTS: &str = "contacts.json";
/// The shared-account registry.
pub const MULTISIGS: &str = "multisigs.json";
/// Relay choice, sync checkpoints, and held packages.
pub const COORDINATION: &str = "coordination.json";
/// Account labels, groups, hidden accounts and the backup flag: the extension keeps the first
/// three in its preferences rather than the vault, and the flag is mobile's own.
pub const PREFERENCES: &str = "preferences.json";
/// Largest metadata file the core will read.
const MAX_FILE_BYTES: u64 = 1024 * 1024;

fn unreadable(file: &str) -> CoreError {
    CoreError::Storage {
        reason: format!("{file} unreadable"),
    }
}

/// Reads `file`; `Ok(None)` when it does not exist yet.
pub fn read<T: DeserializeOwned>(dir: &Path, file: &str) -> Result<Option<T>, CoreError> {
    let path = dir.join(file);
    match fs::metadata(&path) {
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(unreadable(file)),
        Ok(meta) if meta.len() > MAX_FILE_BYTES => return Err(unreadable(file)),
        Ok(_) => {}
    }
    let text = fs::read_to_string(&path).map_err(|_| unreadable(file))?;
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|_| unreadable(file))
}

/// Writes `file` atomically, through the vault's one durable-write path (`files::write_atomic`).
pub fn write<T: Serialize>(dir: &Path, file: &str, value: &T) -> Result<(), CoreError> {
    let failed = || CoreError::Storage {
        reason: format!("{file} not written"),
    };
    let body = serde_json::to_vec(value).map_err(|_| failed())?;
    crate::files::write_atomic(dir, file, &body).map_err(|_| failed())
}

/// Removes every metadata file but the preferences; called when the wallet is deleted, before
/// the vault goes. The preferences hold the backup flag, so a delete that fails after this still
/// leaves a wallet that owes its backup; they go after the vault (`remove`), and a new wallet
/// overwrites any left behind.
pub fn remove_all(dir: &Path) -> Result<(), CoreError> {
    [CONTACTS, MULTISIGS, COORDINATION]
        .into_iter()
        .try_for_each(|file| remove(dir, file))
}

/// Removes `file` and any interrupted write of it.
pub fn remove(dir: &Path, file: &str) -> Result<(), CoreError> {
    for name in [file.to_string(), format!("{file}.tmp")] {
        match fs::remove_file(dir.join(&name)) {
            Err(error) if error.kind() != ErrorKind::NotFound => {
                return Err(CoreError::Storage {
                    reason: format!("{file} not removed"),
                })
            }
            _ => {}
        }
    }
    Ok(())
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Preferences {
    backup_pending: bool,
    groups: Vec<StoredGroup>,
    /// Absent from files written before hiding existed.
    #[serde(default)]
    hidden_account_ids: Vec<String>,
    labels: BTreeMap<String, String>,
    version: u32,
}

/// Writes `payload`'s preferences.
pub(crate) fn save_preferences(dir: &Path, payload: &VaultPayload) -> Result<(), CoreError> {
    let preferences = Preferences {
        backup_pending: payload.backup_pending,
        groups: payload.groups.clone(),
        hidden_account_ids: payload.hidden_account_ids.clone(),
        labels: payload.labels.clone(),
        version: 1,
    };
    write(dir, PREFERENCES, &preferences)
}

/// Loads the preferences into an opened `payload`; none on disk means none set. The vault and
/// this file are written one after the other, so a label, group member or hidden id naming an
/// account the vault does not hold is dropped; anything else invalid (a repeated hidden id
/// included, as the extension's `isHiddenAccountIds`) fails closed.
pub(crate) fn load_preferences(dir: &Path, payload: &mut VaultPayload) -> Result<(), CoreError> {
    let Some(preferences) = read::<Preferences>(dir, PREFERENCES)? else {
        return Ok(());
    };
    if preferences.version != 1 {
        return Err(unreadable(PREFERENCES));
    }
    let held = |id: &String| payload.record(id).is_some();
    let labels = preferences
        .labels
        .into_iter()
        .filter(|(id, _)| held(id))
        .collect();
    let mut groups = preferences.groups;
    for group in &mut groups {
        group.account_ids.retain(held);
    }
    let mut hidden = preferences.hidden_account_ids;
    hidden.retain(held);
    payload.backup_pending = preferences.backup_pending;
    payload.hidden_account_ids = hidden;
    payload.labels = labels;
    payload.groups = groups;
    account_store::check_consistent(payload).map_err(|_| unreadable(PREFERENCES))
}
