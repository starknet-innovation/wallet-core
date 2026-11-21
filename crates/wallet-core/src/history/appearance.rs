//! Activity rows as the side panel renders them: `activity-format.ts` (`activityRowModel`) and
//! `home-tabs.ts` (`transactionRow`, `transactionGroups`, `activityEmptyCopy`), over the
//! classification in `classify.rs`. The explorer link and the token mark come from the caller's
//! catalogs, and the pre-confirmed hint (which names the wallet) from its `HistoryPolicy`. The
//! sentences that carry no rule (`activityEmptyCopy`, the unavailable notice) are the wallet's
//! own.
//!
//! A row is a disclosure. Its summary shows the kind glyph, a heading that joins the verb and the
//! amount, the truncated hash (linked to the explorer) with a copy control, a status word for a
//! row that is not accepted, and the token mark. Its body lists the counterparty or a detail line,
//! the submission time, and the provisional-status hint. No fiat figure: a value computed from
//! today's price is not what the transaction was worth.

use super::activity::{ActivityEntry, TxStatus};
pub use super::classify::ActivityDirection;
use super::classify::{appearance, capitalise, compact_activity_amount, detail};
use super::explorer::explorer_transaction_url;
use super::time::{format_utc_clock, format_utc_date, utc_day_key};
use super::HistoryPolicy;
use crate::catalog::Catalogs;
use crate::js::felt::same_felt;
use crate::js::num::{js_trim, short_label};
use crate::portfolio::model::TokenConfig;
use crate::portfolio::tokens::{starknet_glyph, token_glyph, TokenGlyph};

/// `PROVISIONAL_TX_HINT`: pending has not executed; pre-confirmed has, in a block not yet final
/// (that hint names the wallet, so it is `HistoryPolicy::preconfirmed_hint`).
const PENDING_HINT: &str = "Broadcast accepted. It can't be fee-bumped, replaced, or cancelled.";
const HASH_COPY_LABEL: &str = "Copy transaction hash";

/// The status hue of a row's glyph and status word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityTone {
    /// Accepted: the neutral secondary tone, no status word.
    Settled,
    Pending,
    /// Positive hue at 0.68 opacity on the glyph.
    Preconfirmed,
    Failed,
}

/// The recorded counterparty, as a row's expansion lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityCounterparty {
    /// The fact's label: "Sender", "Recipient" or "Counterparty".
    pub role: String,
    /// The full recorded address (trimmed): the value the control copies and the accessibility
    /// value of the shown text.
    pub address: String,
    /// The text shown: the whole address, since the expansion is where it is read (extension
    /// 53acf291). Kept beside `address` so a host never derives one from the other.
    pub address_label: String,
    /// The copy control's accessible name, "Copy {role} address" in lower case.
    pub copy_label: String,
}

/// Everything one Activity row displays: the disclosure's summary, then its expansion.
#[derive(Debug, Clone, PartialEq)]
pub struct ActivityRow {
    /// An icon name (Bramble checks them against its generated `BrambleIconName` set).
    pub icon: String,
    pub tone: ActivityTone,
    pub status: TxStatus,
    /// The glyph's tooltip: the provisional hint, else "{Kind}, {status}" ("Transfer, accepted").
    /// The glyph itself is hidden from assistive tech.
    pub glyph_title: String,
    /// The verb and the amount ("Received 1.5 STRK"). An entry that records neither an amount
    /// nor a counterparty joins its detail instead ("Revoked For USDC"); else the verb alone.
    pub heading: String,
    /// The recorded hash: the value the hash row's control copies.
    pub hash: String,
    /// `accountLabel(hash)`, the truncated hash shown in the summary.
    pub hash_label: String,
    /// The hash copy control's accessible name.
    pub hash_copy_label: String,
    /// The explorer page the truncated hash links to; `None` shows it as plain text.
    pub explorer_url: Option<String>,
    /// "Pending", "Pre-confirmed", "Rejected" or "Reverted" in the status hue; `None` when
    /// accepted.
    pub status_word: Option<String>,
    /// The trailing token mark (the Starknet mark when the entry records no token).
    pub avatar: TokenGlyph,
    /// The summary's accessible name, "{heading}. {status}, {clock}" (the clock omitted when the
    /// time is unusable).
    pub accessibility_label: String,
    /// The whole row's tooltip, "{Status} · {clock}".
    pub timing: String,
    /// Expansion: the recorded counterparty, when there is one.
    pub counterparty: Option<ActivityCounterparty>,
    /// Expansion: the "Detail" fact, shown only without a counterparty and when the heading does
    /// not already carry it.
    pub detail: Option<String>,
    /// Expansion: the "Submitted" fact, "{DD Mon YYYY}, {HH:MM} UTC"; `None` when undated.
    pub submitted: Option<String>,
    /// Expansion: the "Status" fact, the provisional hint for pending and pre-confirmed rows.
    pub status_hint: Option<String>,
}

/// Rows of one UTC day under their heading.
#[derive(Debug, Clone, PartialEq)]
pub struct ActivityDayGroup {
    pub heading: String,
    pub rows: Vec<ActivityRow>,
}

/// `activityRowModel` without its fiat figure, which no row renders any more. Not FFI: the rows
/// are built from it, and the parity suite checks it against the extension's model directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityRowModel {
    pub direction: Option<ActivityDirection>,
    pub icon: &'static str,
    pub label: &'static str,
    pub title: &'static str,
    /// The appearance `type` (`transfer`, `swap`, `approval`, …).
    pub kind: &'static str,
    pub amount: Option<String>,
    /// The trimmed address and its role (`sender`, `recipient` or `counterparty`).
    pub counterparty: Option<(String, &'static str)>,
    pub detail: Option<String>,
    pub token_address: Option<String>,
    pub token_symbol: Option<String>,
}

fn status_label(status: TxStatus) -> &'static str {
    match status {
        TxStatus::Preconfirmed => "pre-confirmed",
        other => crate::activity::store::status_text(other),
    }
}

fn trimmed(value: Option<&str>) -> Option<&str> {
    value.map(js_trim).filter(|s| !s.is_empty())
}

/// `activityTrackedToken`: the one tracked token on the entry's network with its symbol and,
/// when the row recorded its token's address, at that address (`sameFelt`, so an address that is
/// not a felt matches none). A likely-spam token recorded under a tracked symbol must not be
/// priced or shown as the tracked one (Sherlock 249); a symbol two tracked tokens share, with no
/// address to tell them apart, matches none.
pub fn activity_tracked_token<'a>(
    entry: &ActivityEntry,
    tracked: &'a [TokenConfig],
) -> Option<&'a TokenConfig> {
    let symbol = trimmed(entry.token.as_deref())?.to_lowercase();
    let mut matches = tracked.iter().filter(|t| {
        t.network == entry.network
            && t.symbol.to_lowercase() == symbol
            && entry
                .token_address
                .as_deref()
                .is_none_or(|address| same_felt(&t.address, address))
    });
    match (matches.next(), matches.next()) {
        (Some(token), None) => Some(token),
        _ => None,
    }
}

/// `activityRowModel`, its token resolved by `activity_tracked_token`.
pub fn activity_row_model(entry: &ActivityEntry, tracked: &[TokenConfig]) -> ActivityRowModel {
    let look = appearance(entry);
    let symbol = trimmed(entry.token.as_deref());
    let amount = match (symbol, trimmed(entry.amount.as_deref())) {
        (Some(symbol), Some(raw)) => Some(format!("{} {symbol}", compact_activity_amount(raw))),
        _ => None,
    };
    let token = activity_tracked_token(entry, tracked);
    let role = match look.direction {
        Some(ActivityDirection::In) => "sender",
        Some(ActivityDirection::Out) => "recipient",
        _ if look.kind == "approval" => "recipient",
        _ => "counterparty",
    };
    let counterparty = trimmed(entry.counterparty.as_deref()).map(|a| (a, role));
    ActivityRowModel {
        detail: detail(entry, &look, counterparty, amount.is_some()),
        counterparty: counterparty.map(|(a, role)| (a.to_string(), role)),
        direction: look.direction,
        icon: look.icon,
        label: look.label,
        title: look.title,
        kind: look.kind,
        amount,
        token_address: token.map(|t| t.address.clone()),
        token_symbol: symbol.map(str::to_string),
    }
}

// `transactionRow` over `activityRowModel`.
fn row(
    entry: &ActivityEntry,
    tracked: &[TokenConfig],
    catalogs: Catalogs<'_>,
    policy: &HistoryPolicy,
) -> ActivityRow {
    let model = activity_row_model(entry, tracked);
    let label = status_label(entry.status);
    let hint = match entry.status {
        TxStatus::Pending => Some(PENDING_HINT.to_string()),
        TxStatus::Preconfirmed => Some(policy.preconfirmed_hint.to_string()),
        _ => None,
    };
    let at = Some(entry.submitted_at as f64);
    let clock = format_utc_clock(at);
    let timing = match &clock {
        Some(clock) => format!("{} · {clock}", capitalise(label)),
        None => capitalise(label),
    };
    let spoken = match &clock {
        Some(clock) => format!("{label}, {clock}"),
        None => label.to_string(),
    };
    let submitted = match (format_utc_date(at), &clock) {
        (Some(date), Some(clock)) => Some(format!("{date}, {clock}")),
        (date, clock) => date.or_else(|| clock.clone()),
    };
    // Legacy entries carry their figure only in the summary, which `detail` already stripped of
    // the verb; a counterparty detail is an address and stays in the expansion.
    let figure = model.amount.clone().or_else(|| {
        model
            .counterparty
            .is_none()
            .then(|| model.detail.clone())
            .flatten()
    });
    let heading = match &figure {
        Some(figure) => format!("{} {figure}", model.title),
        None => model.title.to_string(),
    };
    let detail = model
        .detail
        .clone()
        .filter(|d| model.counterparty.is_none() && figure.as_ref() != Some(d));
    let avatar = match &model.token_symbol {
        Some(symbol) => token_glyph(
            catalogs.registry,
            model
                .token_address
                .clone()
                .unwrap_or_else(|| "0x0".to_string()),
            symbol.clone(),
        ),
        None => starknet_glyph(catalogs.networks, catalogs.registry),
    };
    ActivityRow {
        icon: model.icon.into(),
        tone: match entry.status {
            TxStatus::Accepted => ActivityTone::Settled,
            TxStatus::Pending => ActivityTone::Pending,
            TxStatus::Preconfirmed => ActivityTone::Preconfirmed,
            TxStatus::Rejected | TxStatus::Reverted => ActivityTone::Failed,
        },
        status: entry.status,
        glyph_title: hint
            .clone()
            .unwrap_or_else(|| format!("{}, {label}", model.label)),
        accessibility_label: format!("{heading}. {spoken}"),
        heading,
        hash: entry.hash.clone(),
        hash_label: short_label(&entry.hash),
        hash_copy_label: HASH_COPY_LABEL.into(),
        explorer_url: explorer_transaction_url(
            catalogs.networks,
            entry.network,
            entry.hash.clone(),
        ),
        status_word: (entry.status != TxStatus::Accepted).then(|| capitalise(label)),
        avatar,
        timing,
        counterparty: model
            .counterparty
            .map(|(address, role)| ActivityCounterparty {
                role: capitalise(role),
                address_label: address.clone(),
                copy_label: format!("Copy {role} address"),
                address,
            }),
        detail,
        submitted,
        status_hint: hint,
    }
}

/// `transactionGroups`: consecutive rows grouped by UTC day under "Today", "Yesterday",
/// `DD Mon YYYY`, or "Undated". `tracked` is every tracked token (`preferences.tokens`), which
/// resolves each row's token mark; `now_ms` enables Today/Yesterday; `catalogs` give the explorer
/// and the logos; `policy` the pre-confirmed hint.
pub fn activity_groups(
    entries: Vec<ActivityEntry>,
    tracked: Vec<TokenConfig>,
    now_ms: Option<f64>,
    catalogs: Catalogs<'_>,
    policy: &HistoryPolicy,
) -> Vec<ActivityDayGroup> {
    let today = now_ms.map(utc_day_key);
    let mut groups: Vec<(Option<i64>, ActivityDayGroup)> = Vec::new();
    for entry in &entries {
        let at = Some(entry.submitted_at as f64);
        let date = format_utc_date(at);
        let key = date
            .as_ref()
            .map(|_| utc_day_key(entry.submitted_at as f64));
        let row = row(entry, &tracked, catalogs, policy);
        match groups.last_mut() {
            Some((last, group)) if *last == key => group.rows.push(row),
            _ => {
                let heading = match (key, date) {
                    (Some(k), _) if Some(k) == today => "Today".to_string(),
                    (Some(k), _) if today.is_some_and(|t| k == t - 1) => "Yesterday".to_string(),
                    (Some(_), Some(date)) => date,
                    _ => "Undated".to_string(),
                };
                groups.push((
                    key,
                    ActivityDayGroup {
                        heading,
                        rows: vec![row],
                    },
                ));
            }
        }
    }
    groups.into_iter().map(|(_, g)| g).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::activity::ActivityKind;
    use crate::NetworkId;

    // `sidepanel-activity-same-symbol.test.ts` (Bramble `a01b240c`): two tracked tokens share
    // the symbol USDC.
    const USDC_A: &str = "0xa11ce";
    const USDC_B: &str = "0xb0b";

    fn usdc(address: &str) -> TokenConfig {
        TokenConfig {
            id: format!("SN_MAIN:USDC:{address}"),
            network: NetworkId::Mainnet,
            address: address.into(),
            symbol: "USDC".into(),
            name: "USD Coin".into(),
            decimals: 6,
        }
    }

    fn send(hash: &str, token_address: Option<&str>) -> ActivityEntry {
        ActivityEntry {
            account_id: "SN_MAIN:0x1234".into(),
            amount: Some("1".into()),
            counterparty: None,
            direction: None,
            failure_notice: None,
            fee_paid: None,
            finalized_on_l1: None,
            hash: hash.into(),
            kind: ActivityKind::Transfer,
            network: NetworkId::Mainnet,
            not_found_count: None,
            origin: None,
            status: TxStatus::Accepted,
            submitted_at: 1_752_501_900_000,
            submitted_price: None,
            summary: "Send 1 USDC".into(),
            token: Some("USDC".into()),
            token_address: token_address.map(str::to_string),
        }
    }

    fn address(entry: &ActivityEntry, tracked: &[TokenConfig]) -> Option<String> {
        activity_tracked_token(entry, tracked).map(|t| t.address.clone())
    }

    // same-symbol test: "a recorded address prices its own token, but never under another
    // symbol".
    #[test]
    fn a_recorded_address_resolves_only_its_own_token() {
        let tracked = [usdc(USDC_A), usdc(USDC_B)];
        assert_eq!(
            address(&send("0x5", Some(USDC_B)), &tracked).as_deref(),
            Some(USDC_B)
        );
        // A likely-spam token recorded under the untracked symbol (Sherlock 249).
        let spam = ActivityEntry {
            token: Some("TOKEN".into()),
            ..send("0x6", Some(USDC_B))
        };
        assert_eq!(address(&spam, &tracked), None);
        // The shared symbol alone names neither token.
        assert_eq!(address(&send("0x7", None), &tracked), None);
    }

    // Not in the extension's tests: the other contract under a tracked symbol, the address in
    // another spelling, and an address that is not a felt.
    #[test]
    fn another_contract_under_a_tracked_symbol_resolves_to_nothing() {
        let tracked = [usdc(USDC_A)];
        assert_eq!(
            address(&send("0x8", None), &tracked).as_deref(),
            Some(USDC_A)
        );
        assert_eq!(address(&send("0x9", Some(USDC_B)), &tracked), None);
        let padded = send("0xa", Some("0x00000A11CE"));
        assert_eq!(address(&padded, &tracked).as_deref(), Some(USDC_A));
        assert_eq!(address(&send("0xb", Some("not-a-felt")), &tracked), None);
        assert_eq!(
            activity_row_model(&send("0x9", Some(USDC_B)), &tracked).token_address,
            None
        );
    }
}
