//! The history side of the one activity log (`store.rs`): typed `ActivityEntry` rows in and out,
//! `mergeActivity` of on-chain rows into a scope that already holds this wallet's submitted
//! transactions, and the `markHistorySynced` bookkeeping. Each is one read-modify-write of the
//! log, so under the caller's lock a merge can never overwrite a status the transactions side
//! just observed.

use super::store::{ActivityLog, Entry, SyncRecord, MAX_ENTRIES_PER_SCOPE};
use crate::history::activity::{entry_from_json, entry_to_json, merge_activity, ActivityEntry};
use crate::history::activity_rules::without_planted_rows;
use crate::js::json;

/// Scopes whose history-read bookkeeping is kept (`activity.ts`'s sync cap).
const MAX_SYNC_SCOPES: usize = 50;
/// A stored row as the typed record; `None` unless it passes `isActivityEntry`.
pub fn typed(entry: &Entry) -> Option<ActivityEntry> {
    let text = serde_json::to_string(entry).ok()?;
    entry_from_json(&json::parse(&text)?)
}

/// A typed row in the stored shape; `None` when the store's own validation would refuse it.
pub fn stored(entry: &ActivityEntry) -> Option<Entry> {
    serde_json::from_value(entry_to_json(entry)).ok()
}

fn mark(sync: &mut Vec<SyncRecord>, account_id: &str, now_ms: u64, unavailable: bool) {
    sync.retain(|record| record.account_id != account_id);
    sync.push(SyncRecord {
        account_id: account_id.into(),
        synced_at: now_ms,
        unavailable,
    });
    let excess = sync.len().saturating_sub(MAX_SYNC_SCOPES);
    sync.drain(..excess);
}

impl ActivityLog<'_> {
    /// One scope's rows, newest first.
    pub fn scope(&mut self, account_id: &str) -> Vec<ActivityEntry> {
        self.list(account_id).iter().filter_map(typed).collect()
    }

    /// The scope's last history read: `(synced_at, unavailable)`.
    pub fn sync_of(&mut self, account_id: &str) -> Option<(u64, bool)> {
        self.load_log()
            .sync
            .into_iter()
            .find(|record| record.account_id == account_id)
            .map(|record| (record.synced_at, record.unavailable))
    }

    /// `mergeActivity(local, onChain)` into the scope, then `markHistorySynced`; returns the
    /// merged rows newest first. Rows that fail validation are dropped, never stored.
    pub fn merge(
        &mut self,
        account_id: &str,
        on_chain: &[ActivityEntry],
        now_ms: u64,
    ) -> Vec<ActivityEntry> {
        let mut log = self.load_log();
        let local: Vec<ActivityEntry> = log
            .scopes
            .get(account_id)
            .map(|entries| entries.iter().filter_map(typed).collect())
            .unwrap_or_default();
        let local = without_planted_rows(local);
        let mut merged = merge_activity(&local, on_chain);
        merged.truncate(MAX_ENTRIES_PER_SCOPE);
        // Oldest first on disk, as `record` appends.
        let rows: Vec<Entry> = merged
            .iter()
            .rev()
            .filter(|entry| entry.account_id == account_id)
            .filter_map(stored)
            .collect();
        log.scopes.insert(account_id.to_string(), rows);
        mark(&mut log.sync, account_id, now_ms, false);
        self.save_log(&log);
        merged
    }

    /// No history lane answered: keep the rows and record the failed read (retried after 60 s).
    pub fn mark_unavailable(&mut self, account_id: &str, now_ms: u64) {
        let mut log = self.load_log();
        mark(&mut log.sync, account_id, now_ms, true);
        self.save_log(&log);
    }
}
