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

/// `activityToCsv`: the text of the export file, which the host writes under
/// `activity_csv_file_name()` and shares. The extension exports one account's whole feed (every
/// row `ActivityBook` reads for it, not the filtered view), in the feed's order.
#[uniffi::export]
pub fn activity_csv(entries: Vec<ActivityEntry>) -> String {
    wallet_core::activity::csv::activity_to_csv(&entries, crate::networks::all())
}

/// The export's file name (`activity-ui.ts`).
#[uniffi::export]
pub fn activity_csv_file_name() -> String {
    "bramble-activity.csv".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    // The core's cases are `wallet_core::activity::csv`'s; this pins Bramble's explorer links.
    #[test]
    fn the_export_links_each_row_to_bramble_explorers() {
        let entry = |network, account_id: &str| ActivityEntry {
            account_id: account_id.into(),
            amount: None,
            counterparty: None,
            direction: None,
            failure_notice: None,
            fee_paid: None,
            finalized_on_l1: None,
            hash: "0xabc".into(),
            kind: ActivityKind::Invoke,
            network,
            not_found_count: None,
            origin: None,
            status: TxStatus::Accepted,
            submitted_at: 1_700_000_000_000,
            submitted_price: None,
            summary: "Approve spender".into(),
            token: None,
            token_address: None,
        };
        let csv = activity_csv(vec![
            entry(NetworkId::Mainnet, "SN_MAIN:0x1"),
            entry(NetworkId::Sepolia, "SN_SEPOLIA:0x2"),
        ]);
        let rows: Vec<&str> = csv.split('\n').collect();
        assert_eq!(rows[1], "\"2023-11-14 22:13:20\",\"SN_MAIN\",\"0x1\",\"invoke\",\"accepted\",\"\",\"\",\"\",\"0xabc\",\"https://starkscan.co/tx/0xabc\",\"\",\"\",\"Approve spender\"");
        assert_eq!(rows[2], "\"2023-11-14 22:13:20\",\"SN_SEPOLIA\",\"0x2\",\"invoke\",\"accepted\",\"\",\"\",\"\",\"0xabc\",\"https://sepolia.starkscan.co/tx/0xabc\",\"\",\"\",\"Approve spender\"");
        assert_eq!(activity_csv_file_name(), "bramble-activity.csv");
    }
}
