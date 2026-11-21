//! The Activity feed's filter, amount sort and per-counterparty summary
//! (`wallet_core::history::{filter, party}`, the extension's `portfolio/activity-filter.ts`): the
//! UniFFI shapes and the exports. The host keeps the filter the person chose; the core says which
//! values the controls offer, which chosen values went stale, which rows a filter keeps, and what
//! the summary above them reads.

use wallet_core::history::filter as port;
pub use wallet_core::history::filter::{
    ActivityContact, ActivityFilter, ActivityFilterOptions, ActivitySort, ActivityTokenOption,
};
use wallet_core::history::party;
pub use wallet_core::history::party::{
    ActivityPartySummary, ActivityPartySummaryParts, ActivityTokenTotal,
};

use super::activity::{ActivityEntry, TxStatus};
use crate::portfolio::model::TokenConfig;

/// The feed's order: newest first as read, or grouped by token with the largest amount first.
#[uniffi::remote(Enum)]
pub enum ActivitySort {
    Date,
    Amount,
}

/// The feed's filter; `None` on an axis is "All", and an empty query searches nothing.
#[uniffi::remote(Record)]
pub struct ActivityFilter {
    /// A row's verb, one of `ActivityFilterOptions::actions`.
    pub action: Option<String>,
    /// Matches a contact's name or any spelling of the counterparty, case-insensitively.
    pub query: String,
    pub sort: ActivitySort,
    pub status: Option<TxStatus>,
    /// A token key, one of `ActivityFilterOptions::tokens`' `key`.
    pub token: Option<String>,
}

/// One name the search resolves: an address book contact or a labelled account of the person's,
/// on the feed's network.
#[uniffi::remote(Record)]
pub struct ActivityContact {
    pub address: String,
    pub label: String,
}

/// One token the token control offers: `key` is the filter value, `label` what is shown.
#[uniffi::remote(Record)]
pub struct ActivityTokenOption {
    pub key: String,
    pub label: String,
}

/// The values present in a feed, so no control offers one that matches nothing.
#[uniffi::remote(Record)]
pub struct ActivityFilterOptions {
    pub actions: Vec<String>,
    pub statuses: Vec<TxStatus>,
    pub tokens: Vec<ActivityTokenOption>,
}

/// One token's settled totals with the counterparty, in that token's units.
#[uniffi::remote(Record)]
pub struct ActivityTokenTotal {
    pub received: String,
    pub sent: String,
    pub symbol: String,
}

/// The rows counted, their first and last submission times, the counterparty's name (or its
/// address), and the totals per token.
#[uniffi::remote(Record)]
pub struct ActivityPartySummary {
    pub count: u64,
    pub first_at: Option<u64>,
    pub label: String,
    pub last_at: Option<u64>,
    pub totals: Vec<ActivityTokenTotal>,
}

/// The summary's two texts: the name (written as plain text), and the joined detail line.
#[uniffi::remote(Record)]
pub struct ActivityPartySummaryParts {
    pub body: String,
    pub label: String,
}

/// `activityFilterOptions` over one account's feed; `tracked` is every tracked token.
#[uniffi::export]
pub fn activity_filter_options(
    entries: Vec<ActivityEntry>,
    tracked: Vec<TokenConfig>,
) -> ActivityFilterOptions {
    port::activity_filter_options(&entries, &tracked)
}

/// The rows `filter` keeps, in the feed's order or by amount. `activity_groups` renders them; the
/// extension shows the amount sort as one list rather than by day.
#[uniffi::export]
pub fn filter_activity(
    entries: Vec<ActivityEntry>,
    filter: ActivityFilter,
    tracked: Vec<TokenConfig>,
    contacts: Vec<ActivityContact>,
) -> Vec<ActivityEntry> {
    port::filter_activity(entries, &filter, &tracked, &contacts)
}

/// `reconcileActivityFilter`: the filter with every axis whose value `options` (from the current
/// feed) no longer offers reset to all; call it before rendering the controls and the rows.
#[uniffi::export]
pub fn reconcile_activity_filter(
    filter: ActivityFilter,
    options: ActivityFilterOptions,
) -> ActivityFilter {
    port::reconcile_activity_filter(filter, &options)
}

/// `activityPartySummary` over the rows `filter_activity` returned for `filter`: `None` unless
/// they name exactly one counterparty. As in the extension's feed (`activity-ui.ts`), there is no
/// summary while the search is blank.
#[uniffi::export]
pub fn activity_party_summary(
    rows: Vec<ActivityEntry>,
    filter: ActivityFilter,
    tracked: Vec<TokenConfig>,
    contacts: Vec<ActivityContact>,
) -> Option<ActivityPartySummary> {
    if crate::js::num::js_trim(&filter.query).is_empty() {
        return None;
    }
    party::activity_party_summary(&rows, &tracked, &contacts)
}

/// `activityPartySummaryParts`: "{n} transaction(s) · Sent … · Received … · {date or range}".
#[uniffi::export]
pub fn activity_party_summary_parts(summary: ActivityPartySummary) -> ActivityPartySummaryParts {
    party::activity_party_summary_parts(&summary)
}
