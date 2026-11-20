//! Activity rows (`wallet_core::history::activity`, the extension's `ActivityEntry`,
//! `isActivityEntry`, `onChainActivityEntry` and `mergeActivity`): the UniFFI shapes.

pub use wallet_core::history::activity::{
    ActivityEntry, ActivityKind, ActivityOrigin, FailureNotice, FeeUnit, SubmittedPrice,
    TransactionFee, TransferDirection, TxStatus,
};

use crate::networks::NetworkId;
use crate::portfolio::model::DisplayCurrency;

/// Provenance (`ActivityEntry.origin`): `Wallet` only on rows this wallet recorded for a
/// transaction it submitted.
#[uniffi::remote(Enum)]
pub enum ActivityOrigin {
    Wallet,
}

/// The fee a receipt reported (`feePaid`): `amount` a canonical hex felt in `unit`.
#[uniffi::remote(Record)]
pub struct TransactionFee {
    pub amount: String,
    pub unit: FeeUnit,
}

/// One unit of the row's token in `currency` when this wallet submitted it (`submittedPrice`).
#[uniffi::remote(Record)]
pub struct SubmittedPrice {
    pub currency: DisplayCurrency,
    pub priced_at: u64,
    pub unit_price: f64,
}

/// Reconciliation bookkeeping for a failure the user has moved past.
#[uniffi::remote(Enum)]
pub enum FailureNotice {
    Delivered,
    Pending,
}

/// Whether value came into or went out of the account (`ActivityEntry.direction`).
#[uniffi::remote(Enum)]
pub enum TransferDirection {
    In,
    Out,
}

/// The extension's `ActivityEntry`. Built only by the core (on-chain rows) or validated by it
/// (`activity::store::record`).
#[uniffi::remote(Record)]
pub struct ActivityEntry {
    pub account_id: String,
    pub amount: Option<String>,
    pub counterparty: Option<String>,
    /// Recorded where known: from the decoded transfer on discovered rows, `Out` on a send this
    /// wallet made. The feed reads it before the summary, whose token symbol a third party
    /// chooses (Sherlock 196). Absent on legacy rows and rows that are not one value movement.
    #[uniffi(default = None)]
    pub direction: Option<TransferDirection>,
    pub failure_notice: Option<FailureNotice>,
    /// The fee the receipt reported, read once a row this wallet submitted settled with a
    /// receipt. Absent on discovered and older rows: never an estimate.
    #[uniffi(default = None)]
    pub fee_paid: Option<TransactionFee>,
    pub finalized_on_l1: Option<bool>,
    pub hash: String,
    pub kind: ActivityKind,
    pub network: NetworkId,
    pub not_found_count: Option<u64>,
    #[uniffi(default = None)]
    pub origin: Option<ActivityOrigin>,
    pub status: TxStatus,
    pub submitted_at: u64,
    /// The unit price captured when this wallet submitted a public send; never back-filled.
    #[uniffi(default = None)]
    pub submitted_price: Option<SubmittedPrice>,
    pub summary: String,
    pub token: Option<String>,
    /// The canonical address of the token `token` names (issue 643); never without `token`.
    #[uniffi(default = None)]
    pub token_address: Option<String>,
}

/// `activityFeeText`: the expanded row's "Network fee", the receipt's fee in its own unit; `None`
/// when the row holds none.
#[uniffi::export]
pub fn activity_fee_text(entry: ActivityEntry) -> Option<String> {
    wallet_core::activity::detail::fee_text(&entry)
}

/// The expanded row's "Value at submission": the amount at the price captured when it was sent,
/// in that price's currency; `None` without one. Never today's price.
#[uniffi::export]
pub fn activity_value_text(entry: ActivityEntry) -> Option<String> {
    wallet_core::activity::detail::submitted_value_text(&entry)
}
