//! What an expanded Activity row adds (mc-wallet issues 463 and 480, the extension's
//! `activity-detail.ts`): the fee the receipt reported, read once a row this wallet submitted
//! reaches an outcome with a receipt (`tx-reconcile.ts` `wantsFee`, `activity.ts` `recordedFee`),
//! and the value at the unit price captured when the row was submitted
//! (`activity-valuation.ts`), never at a later price. Each fact comes from the stored row alone;
//! one the row lacks is left out, never estimated.

use super::store::{status_text, ActivityLog, Entry};
use crate::felt;
use crate::history::activity::{ActivityEntry, FeeUnit, SubmittedPrice, TransactionFee};
use crate::history::activity_rules::fee_json;
use crate::history::activity_rules::MAX_ACTIVITY_TIMESTAMP_MS;
use crate::js::big::parse_bigint;
use crate::js::num::{js_trim, string_to_number};
use crate::portfolio::fiat::format_fiat;
use crate::portfolio::model::DisplayCurrency;
use crate::portfolio::prices::PriceView;
use crate::send::calls::format_token_amount;
use crate::send::fees::format_fri;
use crate::send::judge::TxStatus;
use crate::send::u256::U256;
use crate::Felt;

// `wantsFee`: only this wallet's rows (a receipt's fee is its sender's), only once (a recorded
// fee is final), and only after an outcome the chain executed, which has a receipt.
fn wants_fee(entry: &Entry) -> bool {
    entry.origin.as_deref() == Some("wallet")
        && entry.fee_paid.is_none()
        && [TxStatus::Accepted, TxStatus::Reverted]
            .iter()
            .any(|status| entry.status == status_text(*status))
}

impl ActivityLog<'_> {
    /// Whether a row for `hash` in the scope still wants the fee its receipt reports.
    pub fn wants_fee(&mut self, account_id: &str, hash: &Felt) -> bool {
        self.load_log()
            .scopes
            .get(account_id)
            .is_some_and(|entries| {
                entries
                    .iter()
                    .any(|e| felt::parse(&e.hash).as_ref() == Some(hash) && wants_fee(e))
            })
    }

    /// Records the receipt's fee on every row for `hash` that wants one; returns whether any
    /// row took it and the file was written.
    pub fn record_fee(&mut self, account_id: &str, hash: &Felt, fee: &TransactionFee) -> bool {
        let mut log = self.load_log();
        let Some(entries) = log.scopes.get_mut(account_id) else {
            return false;
        };
        let mut changed = false;
        for entry in entries
            .iter_mut()
            .filter(|e| felt::parse(&e.hash).as_ref() == Some(hash) && wants_fee(e))
        {
            entry.fee_paid = Some(fee_json(fee));
            changed = true;
        }
        changed && self.save_log(&log)
    }
}

/// `withSubmittedPrice`: the price to record with a row as it is written, from the wallet's
/// cached-only price read (`PriceBook::plan(.., cached_only = true)`) for the row's token.
/// Only a public send this wallet made, of a positive amount, is priced; a stale or undated
/// answer is the last price seen rather than the price at submission, so it records nothing.
pub fn submitted_price(entry: &Entry, view: &PriceView) -> Option<SubmittedPrice> {
    let amount = string_to_number(entry.amount.as_deref()?);
    let priceable = entry.origin.as_deref() == Some("wallet")
        && entry.kind == "transfer"
        && entry.direction.as_deref() == Some("out")
        && entry.submitted_price.is_none()
        && amount.is_finite()
        && amount > 0.0;
    let unit_price = view.price(entry.token_address.as_deref()?)?;
    let priced_at = view.as_of_ms?;
    (priceable
        && !view.stale
        && unit_price.is_finite()
        && unit_price > 0.0
        && priced_at.fract() == 0.0
        && (0.0..=MAX_ACTIVITY_TIMESTAMP_MS as f64).contains(&priced_at))
    .then_some(SubmittedPrice {
        currency: view.currency,
        priced_at: priced_at as u64,
        unit_price,
    })
}

/// `activityFeeText` for the rows this core records: the receipt's fee in its own unit, STRK to
/// six places (`formatFri`), ETH in full. `None` when the row holds no fee.
pub fn fee_text(entry: &ActivityEntry) -> Option<String> {
    let fee = entry.fee_paid.as_ref()?;
    let raw = U256::from_decimal(&parse_bigint(&fee.amount)?.magnitude.to_decimal())?;
    Some(match fee.unit {
        FeeUnit::Fri => format_fri(&raw),
        FeeUnit::Wei => format!("{} ETH", format_token_amount(&raw, 18)),
    })
}

/// The amount at the captured unit price, and the currency it was priced in: the figure behind
/// `submittedValueText` and the CSV's `submittedValue`. `None` without a captured price or a
/// plain positive amount.
pub fn submitted_value(entry: &ActivityEntry) -> Option<(f64, DisplayCurrency)> {
    let price = entry.submitted_price.as_ref()?;
    // `Number(entry.amount?.trim() || NaN)`: an empty amount is NaN, not zero.
    let amount = js_trim(entry.amount.as_deref()?);
    let amount = if amount.is_empty() {
        f64::NAN
    } else {
        string_to_number(amount)
    };
    (amount.is_finite() && amount > 0.0).then_some((amount * price.unit_price, price.currency))
}

/// `submittedValueText`: `submitted_value` through `formatFiat`.
pub fn submitted_value_text(entry: &ActivityEntry) -> Option<String> {
    submitted_value(entry).map(|(value, currency)| format_fiat(value, currency))
}
