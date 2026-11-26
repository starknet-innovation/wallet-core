//! Coordination metadata (`coordination.json`) and the co-signer trust judgement, mirroring
//! `packages/features/multisig/src/coordination.ts` and `multisig-coordination-ops.ts`.
//!
//! Checkpoints keep the extension's semantics exactly: a sync receipts only the wraps it is
//! finished with (imported, a no-op, or refused for good), and `syncedAt` stays just behind the
//! oldest wrap it will try again (`retryable_synced_at`). The `held` list is an earlier mobile
//! build's, from before a sync imported: it is still read, so such a file loads, and a sync
//! empties it.

use std::path::Path;

use serde::{Deserialize, Serialize};
use wallet_core::nostr::sync_window::SyncLookbacks;

use crate::argent_multisig;
use crate::contacts;
use crate::multisig_store;
use crate::nostr;
use crate::nostr_relay::{is_relay_url, stored_relay_url};
use crate::{multisig_constants, public_store, CoreError};

/// The most held packages an earlier build stored.
const MAX_HELD: usize = 50;

/// `NostrSyncCheckpoint`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Checkpoint {
    pub account_id: String,
    pub event_ids: Vec<String>,
    pub relay_urls: Vec<String>,
    pub synced_at: u64,
}

/// A trusted package an earlier build received before a sync imported. Ids only, never content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Held {
    pub account_id: String,
    pub created_at: u64,
    pub event_id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct File {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relay_url: Option<String>,
    pub checkpoints: Vec<Checkpoint>,
    pub held: Vec<Held>,
}

fn is_relay_selection(urls: &[String]) -> bool {
    !urls.is_empty()
        && urls.len() <= 2
        && urls
            .iter()
            .enumerate()
            .all(|(i, url)| !urls[..i].contains(url))
        && urls.iter().all(|url| is_relay_url(url))
}

fn is_account_id(value: &str) -> bool {
    value
        .split_once(':')
        .and_then(|(network, address)| {
            multisig_store::record_id(contacts::network_from_catalog_id(network)?, address).ok()
        })
        .is_some_and(|id| id == value)
}

/// `isCheckpoint`.
pub fn is_checkpoint(checkpoint: &Checkpoint) -> bool {
    let c = &multisig_constants::get().coordination;
    let ids = &checkpoint.event_ids;
    is_account_id(&checkpoint.account_id)
        && ids.len() <= c.max_seen_event_ids
        && ids
            .iter()
            .enumerate()
            .all(|(i, id)| nostr::is_public_key(id) && !ids[..i].contains(id))
        && is_relay_selection(&checkpoint.relay_urls)
        && checkpoint.synced_at <= c.max_sync_seconds
}

fn is_held(held: &Held) -> bool {
    is_account_id(&held.account_id) && nostr::is_public_key(&held.event_id)
}

/// Reads and validates the file; missing means empty, invalid fails closed.
pub fn load(dir: &Path) -> Result<File, CoreError> {
    let Some(mut file) = public_store::read::<File>(dir, public_store::COORDINATION)? else {
        return Ok(File {
            version: 1,
            ..File::default()
        });
    };
    // Relay URLs an earlier build stored are brought to the current rule before validation, as
    // the extension's preferences reader does. A custom relay with no current spelling falls back
    // to the reviewed defaults; a checkpoint whose relays have none is dropped, so the next sync
    // reads that account from the start instead of the whole file failing closed.
    file.relay_url = file.relay_url.as_deref().and_then(stored_relay_url);
    file.checkpoints.retain_mut(|checkpoint| {
        let urls: Option<Vec<String>> = checkpoint
            .relay_urls
            .iter()
            .map(|url| stored_relay_url(url))
            .collect();
        urls.map(|urls| checkpoint.relay_urls = urls).is_some()
    });
    let valid = file.version == 1
        && file.relay_url.as_deref().is_none_or(is_relay_url)
        && file.checkpoints.len() <= multisig_constants::get().coordination.max_checkpoints
        && file.checkpoints.iter().all(is_checkpoint)
        && file.held.len() <= MAX_HELD
        && file.held.iter().all(is_held);
    if !valid {
        return Err(CoreError::Storage {
            reason: format!("{} unreadable", public_store::COORDINATION),
        });
    }
    Ok(file)
}

/// Writes the file.
pub fn store(dir: &Path, file: &File) -> Result<(), CoreError> {
    public_store::write(dir, public_store::COORDINATION, file)
}

/// `coordinationRelayUrls`: one custom relay replaces the reviewed defaults
/// (`coordination_relay_urls`, over the file's stored relay).
pub fn relay_urls(file: &File) -> Vec<String> {
    super::coordination_relay_urls(file.relay_url.as_deref())
}

/// `getCheckpoint`: the account's checkpoint for exactly this relay selection.
pub fn checkpoint<'a>(
    file: &'a File,
    account_id: &str,
    relays: &[String],
) -> Option<&'a Checkpoint> {
    file.checkpoints
        .iter()
        .find(|checkpoint| checkpoint.account_id == account_id && checkpoint.relay_urls == relays)
}

/// Mobile's sync window for `wallet_core::nostr::sync_window` (30 days back on a first sync, else
/// three days behind the last checkpoint, which a retryable wrap holds back).
pub fn sync_lookbacks() -> SyncLookbacks {
    super::SYNC_LOOKBACKS
}

/// `recordSync`: merge seen ids when the relay set is unchanged, newest checkpoint first.
/// Returns `false` (and changes nothing) for an invalid checkpoint, as the extension does.
pub fn record_sync(file: &mut File, checkpoint: Checkpoint) -> bool {
    if !is_checkpoint(&checkpoint) {
        return false;
    }
    let c = &multisig_constants::get().coordination;
    let mut event_ids = checkpoint.event_ids.clone();
    if let Some(existing) = file
        .checkpoints
        .iter()
        .find(|stored| stored.account_id == checkpoint.account_id)
    {
        if existing.relay_urls == checkpoint.relay_urls {
            event_ids.extend(existing.event_ids.iter().cloned());
        }
    }
    let mut unique: Vec<String> = Vec::new();
    for id in event_ids {
        if !unique.contains(&id) {
            unique.push(id);
        }
    }
    unique.truncate(c.max_seen_event_ids);
    let next = Checkpoint {
        event_ids: unique,
        ..checkpoint
    };
    file.checkpoints
        .retain(|stored| stored.account_id != next.account_id);
    file.checkpoints.insert(0, next);
    file.checkpoints.truncate(c.max_checkpoints);
    true
}

/// `isSavedNostrContact`: the sender is a saved contact on `network_id`.
pub fn is_saved_nostr_contact(
    contacts: &[contacts::Entry],
    sender: &str,
    network_id: &str,
) -> bool {
    contacts.iter().any(|contact| {
        contact.network_id == network_id && contact.nostr_public_key.as_deref() == Some(sender)
    })
}

/// `isCoordinationSigner`: the sender is a saved contact on `network_id` whose Stark key's Argent
/// signer GUID is in `signer_guids`. A key the GUID cannot be computed for is not a signer.
pub fn is_saved_signer(
    contacts: &[contacts::Entry],
    sender: &str,
    network_id: &str,
    signer_guids: &[String],
) -> bool {
    contacts.iter().any(|contact| {
        contact.network_id == network_id
            && contact.nostr_public_key.as_deref() == Some(sender)
            && contact
                .public_key_x
                .as_deref()
                .and_then(|key| argent_multisig::signer_guid_hex(key).ok())
                .is_some_and(|guid| signer_guids.contains(&guid))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // An earlier build accepted any case and the default port; reading its file back must not
    // fail closed and take every multisig's coordination with it.
    #[test]
    fn a_file_with_an_earlier_relay_spelling_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        let checkpoint = |urls: &[&str]| Checkpoint {
            account_id: format!("SN_SEPOLIA:0x{}", "1".repeat(63)),
            event_ids: Vec::new(),
            relay_urls: urls.iter().map(|url| (*url).to_string()).collect(),
            synced_at: 1,
        };
        let stored = File {
            version: 1,
            relay_url: Some("WSS://Relay.Example:443".into()),
            checkpoints: vec![
                checkpoint(&["WSS://Relay.Example:443"]),
                checkpoint(&["wss://relay.example/a/../b"]),
            ],
            held: Vec::new(),
        };
        public_store::write(dir.path(), public_store::COORDINATION, &stored).unwrap();
        let loaded = load(dir.path()).unwrap();
        assert_eq!(loaded.relay_url.as_deref(), Some("wss://relay.example"));
        assert_eq!(
            loaded.checkpoints,
            vec![checkpoint(&["wss://relay.example"])],
            "a checkpoint with no current spelling is dropped, so it resyncs"
        );
    }
}
