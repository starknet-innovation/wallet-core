//! The Activity feed's filter and amount sort (`wallet_core::history::filter`, the extension's
//! `portfolio/activity-filter.ts`): the UniFFI shapes and the exports. The host keeps the filter
//! the person chose; the core says which values the controls offer and which rows a filter keeps.

use wallet_core::history::filter as port;
pub use wallet_core::history::filter::{
    ActivityContact, ActivityFilter, ActivityFilterOptions, ActivitySort, ActivityTokenOption,
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
