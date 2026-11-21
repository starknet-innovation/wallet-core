//! ERC-20 allowances, the read side: the records a wallet keeps of the grants it submitted
//! (`record`), which of them it keeps when full (`retention`), the log over `host::Storage`
//! (`store`), how grants made elsewhere are found and read on chain (`discovery`, `scan`,
//! `tracked`), and how the two are merged for the allowance manager. Each is a sans-IO port of
//! the extension's `packages/features/transactions/src/allowances/` and the listing rules in
//! `apps/extension/src/background/portfolio/allowance-ops.ts`.
//!
//! The revoke (`allowance-revoke.ts`'s call and `allowanceTarget`) is not here: it submits a
//! call, which waits on the plan's call batches (P9). Decoding the calls a wallet submits into
//! log writes (`allowances.ts`) waits with it. The record already carries the fields the revoke
//! settles on (`revoking`, `revoke_hash`), and `store` already settles them.

pub mod discovery;
pub mod record;
pub mod retention;
pub mod scan;
pub mod store;
pub mod tracked;

#[cfg(test)]
mod discovery_tests;
#[cfg(test)]
mod retention_tests;
#[cfg(test)]
mod scan_node;
#[cfg(test)]
mod scan_tests;
#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod tracked_tests;

/// A wallet's allowance numbers (Bramble's: `bramble-core`'s `ALLOWANCE_POLICY`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AllowancePolicy {
    /// Event pages read per token for the owner-filtered (Cairo 1 layout) approvals.
    pub owner_event_pages: usize,
    /// Event pages read per block window, and for the rest of the log, for legacy approvals.
    pub log_event_pages: usize,
    /// Events asked for per `starknet_getEvents` page.
    pub event_chunk_size: u64,
    /// Distinct spenders per token whose live allowance is read.
    pub max_spenders_per_token: usize,
    /// Blocks in one window of the legacy read, walked back from the head (at least 1).
    pub legacy_block_window: u64,
    /// Windows the legacy read walks before reading the rest of the log from block 0.
    pub legacy_windows: usize,
    /// Tracked pairs whose live allowance is read in one listing.
    pub max_tracked_pairs: usize,
    /// Records kept for one account on one network.
    pub entries_per_scope: usize,
    /// Records kept across the whole log.
    pub entries_total: usize,
    /// The storage key the log is kept under.
    pub store_key: &'static str,
}

/// A policy for this crate's unit tests, at the extension's numbers; `bramble-core` sets
/// Bramble's.
#[cfg(test)]
pub(crate) const TEST_POLICY: AllowancePolicy = AllowancePolicy {
    owner_event_pages: 5,
    log_event_pages: 3,
    event_chunk_size: 100,
    max_spenders_per_token: 50,
    legacy_block_window: 200_000,
    legacy_windows: 3,
    max_tracked_pairs: 60,
    entries_per_scope: 50,
    entries_total: 200,
    store_key: "allowances.json",
};

#[cfg(test)]
mod tests {
    // As `portfolio_and_history_never_touch_key_material`: allowance code reads public chain
    // state and its own log, never a key.
    #[test]
    fn allowances_never_touch_key_material() {
        let sources = [
            include_str!("discovery.rs"),
            include_str!("record.rs"),
            include_str!("retention.rs"),
            include_str!("scan.rs"),
            include_str!("store.rs"),
            include_str!("tracked.rs"),
        ];
        for source in sources {
            for forbidden in [
                "crate::kms",
                "Zeroizing",
                "SecretFelt",
                "mnemonic",
                "private_key",
                "viewing_key",
            ] {
                assert!(
                    !source.contains(forbidden),
                    "allowance code must not reference {forbidden}"
                );
            }
        }
    }
}
