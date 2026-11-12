//! Activity: the Starkscan lane, the RPC history scan, the local log's cadence, and the row
//! presentation, each a sans-IO port of the extension's TypeScript. The log itself is
//! `crate::activity`; catalogs (`crate::catalog`) and the wallet's choices (`HistoryPolicy`) are
//! arguments.

pub mod activity;
pub mod activity_book;
pub mod activity_rules;
pub mod appearance;
pub mod classify;
pub mod events;
pub mod explorer;
pub mod starkscan;
pub mod time;

/// A wallet's history choices (Bramble's: `bramble-core`'s `HISTORY_POLICY`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryPolicy {
    /// The Starkscan API base (`https://…/v1`) transfers are read from (`STARKSCAN_API_BASE`).
    pub explorer_api_base: &'static str,
    /// The name of the token an untracked contract's row carries (`matchingToken`).
    pub untracked_token_name: &'static str,
    /// A pre-confirmed row's hint (`PROVISIONAL_TX_HINT`), which names the wallet.
    pub preconfirmed_hint: &'static str,
}
