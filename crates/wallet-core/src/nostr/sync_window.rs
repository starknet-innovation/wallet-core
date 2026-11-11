//! The time span a co-signer sync scans, and how long one wrap may hold it open, as the
//! extension's `multisig-sync-window.ts`. A wrap that can never become valid is receipted; one
//! that can still clear holds the window back, but never beyond the first-sync lookback, so a
//! sender cannot pin the scan with wraps that are retried forever. The lookbacks are the wallet's
//! choice, so they are an argument ([`SyncLookbacks`]). Times are Unix seconds.

/// A wallet's sync lookbacks (Bramble's: 30 days, then 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncLookbacks {
    /// `NOSTR_SYNC_LOOKBACK_SECONDS`: the first-sync lookback, which is also the retry cut-off.
    pub lookback_seconds: i64,
    /// `NOSTR_INCREMENTAL_LOOKBACK_SECONDS`: the overlap kept behind the last checkpoint.
    pub incremental_lookback_seconds: i64,
}

/// `retryableSyncedAt`: where the next sync may start, now or just before the oldest retryable
/// wrap.
pub fn retryable_synced_at(
    now_seconds: i64,
    oldest_retryable_at: Option<i64>,
    lookbacks: SyncLookbacks,
) -> i64 {
    match oldest_retryable_at {
        None => now_seconds,
        Some(oldest) => {
            now_seconds.min(oldest.saturating_add(lookbacks.incremental_lookback_seconds))
        }
    }
}

/// `nostrSyncSince`: the `since` bound for a relay query, from the last recorded sync. A recorded
/// sync in the future is ignored.
pub fn nostr_sync_since(now_seconds: i64, synced_at: Option<i64>, lookbacks: SyncLookbacks) -> i64 {
    let since = match synced_at {
        Some(synced_at) if synced_at <= now_seconds => {
            synced_at.saturating_sub(lookbacks.incremental_lookback_seconds)
        }
        _ => now_seconds.saturating_sub(lookbacks.lookback_seconds),
    };
    since.max(0)
}

/// `isWrapPastRetry`: a wrap that is still retryable has aged past the first-sync lookback and is
/// receipted as final. `now_seconds` is a JavaScript number: the extension passes the clock in
/// unfloored seconds (`now() / 1000`), so a wrap exactly one lookback old is past it half a second
/// later.
pub fn is_wrap_past_retry(created_at: u64, now_seconds: f64, lookbacks: SyncLookbacks) -> bool {
    // Both sides are below 2^53 in practice, where the conversions are exact.
    (created_at as f64) < now_seconds - lookbacks.lookback_seconds as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOOKBACKS: SyncLookbacks = SyncLookbacks {
        lookback_seconds: 30 * 86_400,
        incremental_lookback_seconds: 3 * 86_400,
    };

    #[test]
    fn the_window_never_goes_below_zero_or_past_now() {
        assert_eq!(nostr_sync_since(1_000, None, LOOKBACKS), 0);
        assert_eq!(nostr_sync_since(i64::MIN, Some(i64::MIN), LOOKBACKS), 0);
        assert_eq!(retryable_synced_at(10, Some(i64::MAX), LOOKBACKS), 10);
        assert!(!is_wrap_past_retry(0, 1_000.0, LOOKBACKS));
    }
}
