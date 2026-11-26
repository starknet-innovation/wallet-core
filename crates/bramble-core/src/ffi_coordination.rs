//! UniFFI surface for Nostr coordination (`docs/nostr-multisig.md` §3.4): relay selection, and one
//! "Check for updates" as a sans-IO plan the host drives with its WebSockets and RPC. Gift-wrap
//! plaintext and raw events stay in the core; only counts and intent ids come back.
//!
//! A sync imports every trusted package it opens, as the extension's `syncIntents` does
//! (`importSharedIntent` in `multisig-coordination-ops.ts`): each import is checked against a
//! fresh `MultisigStateRead` of the multisig, which the plan hands the host one at a time.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard};

use wallet_core::nostr::sync_window::nostr_sync_since;

use crate::coordination;
use crate::ffi_coordination_sync::Importing;
use crate::multisig_rpc::MultisigStateRead;
use crate::nostr_relay::{self, RelayStep, SyncJudge};
use crate::{multisig_constants, multisig_store, CoreError, WalletCore};

/// Relay selection and the timeouts the host enforces (`RELAY_TIMEOUT_MS` for connect and sync).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RelayPolicy {
    pub relay_urls: Vec<String>,
    pub connect_timeout_ms: u32,
    pub request_timeout_ms: u32,
    pub is_custom: bool,
}

/// What one "Check for updates" did, as the extension's sync answers. `received` counts the wraps
/// this sync is finished with (imported, nothing new, or refused for good); a wrap refused for a
/// passing reason is opened again by the next sync. `intent_ids` are the intents it imported or
/// added signatures to, once each, and `imported` their number.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CoordinationSyncOutcome {
    pub received: u32,
    pub imported: u32,
    pub intent_ids: Vec<String>,
}

/// What the host does next for a sync whose relays are done.
#[derive(uniffi::Enum)]
pub enum CoordinationSyncStep {
    /// Drive `read`, a fresh read of the multisig, to its end (a failed request included), then
    /// hand it back through `coordination_sync_import`.
    Read { read: Arc<MultisigStateRead> },
    /// Terminal: the checkpoint is recorded.
    Done { outcome: CoordinationSyncOutcome },
}

/// One coordination sync: the host opens a socket per relay, sends `open_frame`, forwards every
/// frame to `accept_frame` and does what its `RelayStep` says (a `NextPage` is read on the same
/// socket), and reports timeouts, errors and closes through `relay_ended`. Then
/// `coordination_sync_open` opens what the relays returned and steps through the imports.
#[derive(uniffi::Object)]
pub struct NostrSyncPlan {
    pub(crate) judge: SyncJudge,
    pub(crate) record_id: String,
    pub(crate) now: u64,
    pub(crate) seen: HashSet<String>,
    stage: Mutex<Stage>,
}

pub(crate) enum Stage {
    Relays,
    Importing(Box<Importing>),
    Over,
}

impl NostrSyncPlan {
    // A poisoned lock means a panic mid-step: the sync is over.
    pub(crate) fn stage(&self) -> MutexGuard<'_, Stage> {
        self.stage.lock().unwrap_or_else(|poisoned| {
            let mut guard = poisoned.into_inner();
            *guard = Stage::Over;
            guard
        })
    }
}

#[uniffi::export]
impl NostrSyncPlan {
    /// The relays to open, in order.
    pub fn relay_urls(&self) -> Vec<String> {
        self.judge.relay_urls()
    }

    /// The subscription frame to send once relay `relay` is connected.
    pub fn open_frame(&self, relay: u32) -> String {
        let _ = relay;
        self.judge.open_frame()
    }

    /// Judges one text frame from relay `relay`.
    pub fn accept_frame(&self, relay: u32, frame: String) -> RelayStep {
        self.judge.accept_frame(relay as usize, &frame)
    }

    /// Relay `relay` timed out, errored, or closed.
    pub fn relay_ended(&self, relay: u32) {
        self.judge.relay_ended(relay as usize);
    }
}

pub(crate) fn not_found() -> CoreError {
    CoreError::InvalidInput {
        reason: "shared account not found".into(),
    }
}

#[uniffi::export]
impl WalletCore {
    /// The relays coordination uses and the timeouts the host enforces.
    pub fn relay_policy(&self) -> Result<RelayPolicy, CoreError> {
        self.require_unlocked()?;
        let file = coordination::load(self.data_dir())?;
        let timeout = multisig_constants::get().relays.timeout_ms;
        Ok(RelayPolicy {
            relay_urls: coordination::relay_urls(&file),
            connect_timeout_ms: timeout,
            request_timeout_ms: timeout,
            is_custom: file.relay_url.is_some(),
        })
    }

    /// Saves a custom relay (it replaces the defaults) or, when empty, restores the defaults.
    pub fn set_nostr_relay_url(&self, raw: String) -> Result<Option<String>, CoreError> {
        self.require_unlocked()?;
        let url = nostr_relay::normalize_relay_url(&raw)?;
        let mut file = coordination::load(self.data_dir())?;
        file.relay_url = url.clone();
        coordination::store(self.data_dir(), &file)?;
        Ok(url)
    }

    /// Starts a coordination sync for one shared account at the host clock `now` (Unix seconds).
    pub fn coordination_sync_begin(
        &self,
        multisig_id: String,
        now: u64,
    ) -> Result<Arc<NostrSyncPlan>, CoreError> {
        let own = self.nostr_public_key()?;
        let file = multisig_store::load(self.data_dir())?;
        let record = file
            .records
            .iter()
            .find(|record| record.id == multisig_id)
            .ok_or_else(not_found)?;
        let coordination_file = coordination::load(self.data_dir())?;
        let relays = coordination::relay_urls(&coordination_file);
        let checkpoint = coordination::checkpoint(&coordination_file, &record.id, &relays);
        // A clock past `i64::MAX` saturates; the judge then refuses the unsafe `since`.
        let since = nostr_sync_since(
            i64::try_from(now).unwrap_or(i64::MAX),
            checkpoint.and_then(|c| i64::try_from(c.synced_at).ok()),
            coordination::sync_lookbacks(),
        );
        let seen: HashSet<String> = checkpoint
            .map(|c| c.event_ids.iter().cloned().collect())
            .unwrap_or_default();
        Ok(Arc::new(NostrSyncPlan {
            judge: SyncJudge::new(&own, since, relays, seen.clone())?,
            record_id: record.id.clone(),
            now,
            seen,
            stage: Mutex::new(Stage::Relays),
        }))
    }
}
