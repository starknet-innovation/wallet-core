//! The per-counterparty summary above the Activity feed, ported from `wallet-ui`'s
//! `portfolio/activity-filter.ts`: `activityPartySummary` and `activityPartySummaryParts`, over
//! the facts `activitySummaryRow` reads off each rendered row, here read off each `ActivityEntry`
//! through the same row model and token key as `history::filter`. The extension shows it for the
//! rows a non-empty search leaves visible (`activity-ui.ts`); the caller decides when to ask.

use super::activity::{ActivityEntry, TxStatus};
use super::appearance::{activity_row_model, ActivityDirection};
use super::filter::{contact_for, token_key, ActivityContact};
use super::time::format_utc_date;
use crate::js::big::{parse_bigint, Big};
use crate::js::felt::canonical_felt;
use crate::js::num::js_trim;
use crate::portfolio::model::TokenConfig;
use crate::rpc::format_digits;

/// One token's settled totals with the counterparty, in that token's units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityTokenTotal {
    pub received: String,
    pub sent: String,
    pub symbol: String,
}

/// `ActivityPartySummary`: the rows counted, their first and last submission times, the
/// counterparty's name (or its address), and the totals per token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityPartySummary {
    pub count: u64,
    pub first_at: Option<u64>,
    pub label: String,
    pub last_at: Option<u64>,
    pub totals: Vec<ActivityTokenTotal>,
}

/// The summary as the two texts the feed shows: the name, and the joined detail line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityPartySummaryParts {
    pub body: String,
    pub label: String,
}

// `ActivitySummaryRow`: what `activityRowAttributes` writes and `activitySummaryRow` reads back.
struct SummaryRow {
    amount: String,
    counterparty: String,
    decimals: Option<u8>,
    direction: Option<ActivityDirection>,
    label: String,
    status: TxStatus,
    submitted_at: u64,
    token: String,
    token_key: String,
}

fn summary_row(
    entry: &ActivityEntry,
    tracked: &[TokenConfig],
    contacts: &[ActivityContact],
) -> SummaryRow {
    let model = activity_row_model(entry, tracked);
    let trimmed = |value: &Option<String>| js_trim(value.as_deref().unwrap_or("")).to_string();
    SummaryRow {
        amount: trimmed(&entry.amount),
        counterparty: trimmed(&entry.counterparty).to_lowercase(),
        // `pricing.tokens.find(token => token.address === model.tokenAddress)`.
        decimals: model
            .token_address
            .as_ref()
            .and_then(|address| tracked.iter().find(|t| t.address == *address))
            .map(|t| t.decimals),
        direction: model.direction,
        label: contact_for(entry.counterparty.as_deref(), contacts)
            .map(|c| c.label.clone())
            .unwrap_or_default(),
        status: entry.status,
        submitted_at: entry.submitted_at,
        token: trimmed(&entry.token),
        token_key: token_key(entry, model.token_address),
    }
}

// `parseAmountSafe`: `/^\d+(?:\.\d+)?$/` with at most `decimals` fraction digits, in base units,
// positive; anything else is skipped rather than trusted.
fn parse_amount_safe(amount: &str, decimals: u8) -> Option<Big> {
    let trimmed = js_trim(amount);
    let (whole, fraction) = trimmed.split_once('.').unwrap_or((trimmed, ""));
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(whole) || (trimmed.contains('.') && !digits(fraction)) {
        return None;
    }
    let width = usize::from(decimals);
    if fraction.len() > width {
        return None;
    }
    let raw = parse_bigint(&format!("{whole}{fraction:0<width$}"))?.magnitude;
    (!raw.is_zero()).then_some(raw)
}

// `activityPartySummary` over the rows' facts.
fn summarise(rows: &[SummaryRow]) -> Option<ActivityPartySummary> {
    // One counterparty written with or without leading zeros is one counterparty.
    let mut parties: Vec<String> = rows
        .iter()
        .filter(|r| !r.counterparty.is_empty())
        .map(|r| canonical_felt(&r.counterparty).unwrap_or_else(|| r.counterparty.to_lowercase()))
        .collect();
    parties.sort();
    parties.dedup();
    if parties.len() != 1 {
        return None;
    }
    let first = rows.iter().find(|r| !r.counterparty.is_empty());
    let label = rows
        .iter()
        .find(|r| !r.label.is_empty())
        .map(|r| r.label.clone())
        .or_else(|| first.map(|r| r.counterparty.clone()))
        .unwrap_or_default();
    // Every row is counted; only a settled movement with a parseable amount adds to a total.
    let mut totals: Vec<(&str, Big, Big)> = Vec::new();
    for row in rows {
        let Some(decimals) = row.decimals else {
            continue;
        };
        let inbound = match row.direction {
            Some(ActivityDirection::In) => true,
            Some(ActivityDirection::Out) => false,
            _ => continue,
        };
        let settled = matches!(row.status, TxStatus::Accepted | TxStatus::Preconfirmed);
        if !settled || row.token_key.is_empty() {
            continue;
        }
        let Some(raw) = parse_amount_safe(&row.amount, decimals) else {
            continue;
        };
        let index = match totals.iter().position(|(key, ..)| *key == row.token_key) {
            Some(index) => index,
            None => {
                totals.push((&row.token_key, Big::zero(), Big::zero()));
                totals.len() - 1
            }
        };
        let (_, received, sent) = &mut totals[index];
        let sum = if inbound { received } else { sent };
        *sum = sum.plus(&raw);
    }
    let format = |key: &str, raw: &Big| {
        let decimals = rows
            .iter()
            .find_map(|r| r.decimals.filter(|_| r.token_key == key))
            .unwrap_or(0);
        format_digits(&raw.to_decimal(), decimals, decimals)
    };
    Some(ActivityPartySummary {
        count: rows.len() as u64,
        first_at: rows.iter().map(|r| r.submitted_at).min(),
        label,
        last_at: rows.iter().map(|r| r.submitted_at).max(),
        totals: totals
            .iter()
            .map(|(key, received, sent)| ActivityTokenTotal {
                received: format(key, received),
                sent: format(key, sent),
                symbol: rows
                    .iter()
                    .find(|r| r.token_key == *key)
                    .map_or_else(|| key.to_string(), |r| r.token.clone()),
            })
            .collect(),
    })
}

/// `activityPartySummary` over the rows `filter_activity` kept: `None` unless they name exactly
/// one counterparty.
pub fn activity_party_summary(
    rows: &[ActivityEntry],
    tracked: &[TokenConfig],
    contacts: &[ActivityContact],
) -> Option<ActivityPartySummary> {
    let facts: Vec<SummaryRow> = rows
        .iter()
        .map(|entry| summary_row(entry, tracked, contacts))
        .collect();
    summarise(&facts)
}

/// `activityPartySummaryParts`: "{n} transaction(s) · Sent … · Received … · {date or range}",
/// a zero total left out; the label verbatim (the host writes it as text).
pub fn activity_party_summary_parts(summary: &ActivityPartySummary) -> ActivityPartySummaryParts {
    let plural = if summary.count == 1 { "" } else { "s" };
    let mut lines = vec![format!("{} transaction{plural}", summary.count)];
    for total in &summary.totals {
        if total.sent != "0" {
            lines.push(format!("Sent {} {}", total.sent, total.symbol));
        }
        if total.received != "0" {
            lines.push(format!("Received {} {}", total.received, total.symbol));
        }
    }
    let date = |at: Option<u64>| at.and_then(|ms| format_utc_date(Some(ms as f64)));
    if let (Some(from), Some(to)) = (date(summary.first_at), date(summary.last_at)) {
        lines.push(if from == to {
            from
        } else {
            format!("{from} to {to}")
        });
    }
    ActivityPartySummaryParts {
        body: lines.join(" · "),
        label: summary.label.clone(),
    }
}

#[cfg(test)]
#[path = "party_tests.rs"]
mod tests;
