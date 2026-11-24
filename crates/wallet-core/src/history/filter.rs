//! The Activity feed's filter and amount sort, ported from `wallet-ui`'s
//! `portfolio/activity-filter.ts`: `ActivityFilter`, `activityFilterOptions`, `activityTokenKey`,
//! `activitySearchIndex`, and `activityRowMatches` over the facts `activityRowAttributes` writes,
//! then `activityAmountComparator`. The extension writes those facts onto each rendered row and
//! filters the rows in place; here they are read off each `ActivityEntry`, so the filter and the
//! row model share one classification (`activity_row_model`). `tracked` is every tracked token
//! (the extension's `pricing.tokens`); `contacts` are the names a search resolves, which the
//! wallet assembles from its address book and its labelled accounts.

use std::cmp::Ordering;
use std::collections::HashMap;

use super::activity::{ActivityEntry, TxStatus};
use super::appearance::activity_row_model;
use crate::js::felt::address_label;
use crate::js::felt::{canonical_address, canonical_felt};
use crate::js::num::{js_trim, string_to_number, utf16_cmp};
use crate::portfolio::model::TokenConfig;

/// The feed's order: newest first as given, or `activityAmountComparator`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActivitySort {
    #[default]
    Date,
    Amount,
}

/// `ActivityFilter`: every axis is conjunctive, and `None` is the extension's `all`. `action` is
/// a row's visible verb (an `ActivityFilterOptions::actions` value), `token` a token key (an
/// `ActivityTokenOption::key`). An empty query searches nothing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ActivityFilter {
    pub action: Option<String>,
    pub query: String,
    pub sort: ActivitySort,
    pub status: Option<TxStatus>,
    pub token: Option<String>,
}

/// `ActivityContact`: one name a search resolves, on the feed's network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityContact {
    pub address: String,
    pub label: String,
}

/// One token the token control offers: `key` is the filter value, `label` what the reader sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityTokenOption {
    pub key: String,
    pub label: String,
}

/// `ActivityFilterOptions`: the values present in a feed, so no control offers one that matches
/// nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityFilterOptions {
    pub actions: Vec<String>,
    pub statuses: Vec<TxStatus>,
    pub tokens: Vec<ActivityTokenOption>,
}

// The status control's lifecycle order.
const STATUS_ORDER: [TxStatus; 5] = [
    TxStatus::Accepted,
    TxStatus::Pending,
    TxStatus::Preconfirmed,
    TxStatus::Rejected,
    TxStatus::Reverted,
];

fn trimmed_token(entry: &ActivityEntry) -> &str {
    js_trim(entry.token.as_deref().unwrap_or(""))
}

// `activityTokenKey`, given the row model's tracked token: the canonical address, so two tokens
// that share a symbol are never one; a `symbol:` key for a legacy row no single tracked token
// answers; `""` when the row names no token.
pub(super) fn token_key(entry: &ActivityEntry, tracked_address: Option<String>) -> String {
    let symbol = trimmed_token(entry);
    if symbol.is_empty() {
        return String::new();
    }
    entry
        .token_address
        .clone()
        .or(tracked_address)
        .filter(|address| !address.is_empty())
        .and_then(|address| canonical_felt(&address))
        .unwrap_or_else(|| format!("symbol:{}", symbol.to_lowercase()))
}

// `contactFor`: by field element, never by substring.
pub(super) fn contact_for<'a>(
    counterparty: Option<&str>,
    contacts: &'a [ActivityContact],
) -> Option<&'a ActivityContact> {
    let canonical = canonical_felt(counterparty.filter(|c| !c.is_empty())?)?;
    contacts
        .iter()
        .find(|contact| canonical_felt(&contact.address).as_deref() == Some(canonical.as_str()))
}

// `activitySearchIndex`: the contact's name and the counterparty in its recorded, canonical and
// padded spellings, lower-cased.
fn search_index(counterparty: Option<&str>, label: Option<&str>) -> String {
    let mut parts: Vec<String> = label
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .into_iter()
        .collect();
    if let Some(counterparty) = counterparty.filter(|c| !c.is_empty()) {
        parts.push(counterparty.to_string());
        if let Some(canonical) = canonical_felt(counterparty).filter(|c| c != counterparty) {
            parts.push(canonical);
        }
        if let Some(padded) = canonical_address(counterparty) {
            if !parts.iter().any(|part| part.to_lowercase() == padded) {
                parts.push(padded);
            }
        }
    }
    parts.join(" ").to_lowercase()
}

/// `activityFilterOptions`: actions in text order, statuses in lifecycle order, tokens by label
/// then key; a label two address-keyed tokens share names each by its short address.
pub fn activity_filter_options(
    entries: &[ActivityEntry],
    tracked: &[TokenConfig],
) -> ActivityFilterOptions {
    let mut actions: Vec<String> = Vec::new();
    let mut tokens: Vec<(String, String)> = Vec::new();
    for entry in entries {
        let model = activity_row_model(entry, tracked);
        actions.push(model.title.to_string());
        let key = token_key(entry, model.token_address);
        let label = trimmed_token(entry);
        if !key.is_empty() && !label.is_empty() && !tokens.iter().any(|(k, _)| *k == key) {
            tokens.push((key, label.to_string()));
        }
    }
    actions.sort_by(|l, r| utf16_cmp(l, r));
    actions.dedup();
    tokens.sort_by(|(lk, ll), (rk, rl)| utf16_cmp(ll, rl).then_with(|| utf16_cmp(lk, rk)));
    let shared = |label: &str| tokens.iter().filter(|(_, l)| l == label).count() >= 2;
    let tokens = tokens
        .iter()
        .map(|(key, label)| ActivityTokenOption {
            key: key.clone(),
            label: if !shared(label) || key.starts_with("symbol:") {
                label.clone()
            } else {
                format!("{label} · {}", address_label(key))
            },
        })
        .collect();
    ActivityFilterOptions {
        actions,
        statuses: STATUS_ORDER
            .into_iter()
            .filter(|status| entries.iter().any(|e| e.status == *status))
            .collect(),
        tokens,
    }
}

fn numeric_amount(amount: Option<&str>) -> Option<f64> {
    let trimmed = js_trim(amount?);
    let value = string_to_number(trimmed);
    (!trimmed.is_empty() && value.is_finite()).then_some(value)
}

// A row paired with its token key and that key's group symbol.
type Keyed = (String, String, ActivityEntry);

// `activityAmountComparator` over keyed rows: grouped by token (by symbol, then key; a token-less
// row last), each token's rows by amount descending, then newest first; a row with no numeric
// amount last within its token. The extension groups by each row's own symbol, which is no order
// when one address is recorded under two symbols (`USDC`, `USDC.e`): Rust's sort panics on that,
// so each key groups under one symbol, its first row's, as the token control labels it.
fn by_amount(
    (left_key, left_symbol, left): &Keyed,
    (right_key, right_symbol, right): &Keyed,
) -> Ordering {
    if left_key != right_key {
        if left_key.is_empty() {
            return Ordering::Greater;
        }
        if right_key.is_empty() {
            return Ordering::Less;
        }
        return utf16_cmp(left_symbol, right_symbol).then_with(|| utf16_cmp(left_key, right_key));
    }
    let newest_first = right.submitted_at.cmp(&left.submitted_at);
    match (
        numeric_amount(left.amount.as_deref()),
        numeric_amount(right.amount.as_deref()),
    ) {
        (None, None) => newest_first,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(l), Some(r)) => r
            .partial_cmp(&l)
            .unwrap_or(Ordering::Equal)
            .then(newest_first),
    }
}

/// `reconcileActivityFilter`: an axis whose value the feed no longer offers is reset to all, so a
/// filter never hides every row behind a value its control cannot show; the query and the sort
/// pass through.
pub fn reconcile_activity_filter(
    filter: ActivityFilter,
    options: &ActivityFilterOptions,
) -> ActivityFilter {
    ActivityFilter {
        action: filter.action.filter(|a| options.actions.contains(a)),
        status: filter.status.filter(|s| options.statuses.contains(s)),
        token: filter
            .token
            .filter(|t| options.tokens.iter().any(|option| option.key == *t)),
        ..filter
    }
}

/// The rows `filter` keeps (`activityRowMatches`), in the feed's order or, for the amount sort,
/// by `activityAmountComparator`.
pub fn filter_activity(
    entries: Vec<ActivityEntry>,
    filter: &ActivityFilter,
    tracked: &[TokenConfig],
    contacts: &[ActivityContact],
) -> Vec<ActivityEntry> {
    let query = js_trim(&filter.query).to_lowercase();
    let rows: Vec<(String, ActivityEntry)> = entries
        .into_iter()
        .filter_map(|entry| {
            let model = activity_row_model(&entry, tracked);
            let key = token_key(&entry, model.token_address);
            let counterparty = entry.counterparty.as_deref();
            let label = contact_for(counterparty, contacts).map(|c| c.label.as_str());
            let keep = filter.action.as_deref().is_none_or(|a| a == model.title)
                && filter.status.is_none_or(|s| s == entry.status)
                && filter.token.as_ref().is_none_or(|t| *t == key)
                && (query.is_empty() || search_index(counterparty, label).contains(&query));
            keep.then_some((key, entry))
        })
        .collect();
    if filter.sort != ActivitySort::Amount {
        return rows.into_iter().map(|(_, entry)| entry).collect();
    }
    let mut symbols: HashMap<String, String> = HashMap::new();
    for (key, entry) in &rows {
        symbols
            .entry(key.clone())
            .or_insert_with(|| trimmed_token(entry).to_lowercase());
    }
    let mut keyed: Vec<Keyed> = rows
        .into_iter()
        .map(|(key, entry)| {
            let symbol = symbols.get(&key).cloned().unwrap_or_default();
            (key, symbol, entry)
        })
        .collect();
    keyed.sort_by(by_amount);
    keyed.into_iter().map(|(_, _, entry)| entry).collect()
}

#[cfg(test)]
#[path = "filter_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "filter_reconcile_tests.rs"]
mod reconcile_tests;

#[cfg(test)]
#[path = "filter_sort_tests.rs"]
mod sort_tests;
