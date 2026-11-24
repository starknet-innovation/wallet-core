//! Activity rows, ported from `transactions/activity/activity.ts` (`ActivityEntry`,
//! `isActivityEntry`) and `history/tx-history.ts` (`onChainActivityEntry`, `mergeActivity`).
//! Summaries are load-bearing in the extension (dedup matches on them), so they are built here,
//! exactly, and never by the host.

use std::collections::HashSet;

use serde_json::{json, Map, Value};

use super::activity_rules::{
    activity_token_address, direction_text, fee_json, is_recorded_token_address, parse_direction,
    price_json, submitted_price, transaction_fee, user_fee, user_fee_json,
};
pub use super::activity_rules::{FeePayer, UserFee};
use super::explorer::is_account_id_for_network;
use crate::activity::store::{kind_text, parse_kind, parse_status, status_text};
use crate::js::felt::{
    address_label, canonical_felt, is_stark_field_element, same_felt, same_felt_value,
    uint256_felts,
};
use crate::js::json::JsValue;
use crate::js::num::{is_safe_integer, utf16_len, utf16_slice};
use crate::portfolio::model::{network_code, network_from_code, DisplayCurrency, TokenConfig};
pub use crate::send::fees::{FeeUnit, TransactionFee};
pub use crate::send::judge::TxStatus;
use crate::NetworkId;

/// The longest summary a row stores, in UTF-16 units (`MAX_ACTIVITY_SUMMARY_LENGTH`).
pub(crate) const MAX_SUMMARY_LENGTH: usize = 200;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// What an activity row's transaction did: the extension's `ActivityEntry.kind`. The one kind
/// for local and on-chain rows alike.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityKind {
    Declare,
    Deploy,
    Invoke,
    Transfer,
}

/// Reconciliation bookkeeping for a failure the user has moved past.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureNotice {
    Delivered,
    Pending,
}

/// Whether value came into or went out of the account (`ActivityEntry.direction`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferDirection {
    In,
    Out,
}

/// Provenance (`ActivityEntry.origin`): `Wallet` only on rows this wallet recorded for a
/// transaction it submitted, never on discovered rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityOrigin {
    Wallet,
}

/// What one unit of the row's token was worth in `currency` when this wallet submitted it
/// (`SubmittedPrice`): a fresh cached price captured at `priced_at`, never a later one.
#[derive(Debug, Clone, PartialEq)]
pub struct SubmittedPrice {
    pub currency: DisplayCurrency,
    pub priced_at: u64,
    pub unit_price: f64,
}

/// The extension's `ActivityEntry`. Built only by the core (on-chain rows) or validated by it
/// (`ActivityLog::record`).
#[derive(Debug, Clone, PartialEq)]
pub struct ActivityEntry {
    pub account_id: String,
    pub amount: Option<String>,
    pub counterparty: Option<String>,
    /// Recorded where known: from the decoded transfer on discovered rows, `Out` on a send this
    /// wallet made. The feed reads it before the summary, whose token symbol a third party
    /// chooses (Sherlock 196). Absent on legacy rows and rows that are not one value movement.
    pub direction: Option<TransferDirection>,
    pub failure_notice: Option<FailureNotice>,
    /// The fee the receipt reported, read once a row this wallet submitted reached an outcome
    /// with a receipt (`feePaid`). Never an estimate; absent on discovered and older rows.
    pub fee_paid: Option<TransactionFee>,
    /// `Sponsor` on a row whose network fee someone else paid (`feePayer`).
    pub fee_payer: Option<FeePayer>,
    pub finalized_on_l1: Option<bool>,
    pub hash: String,
    pub kind: ActivityKind,
    pub network: NetworkId,
    pub not_found_count: Option<u64>,
    pub origin: Option<ActivityOrigin>,
    pub status: TxStatus,
    pub submitted_at: u64,
    /// The unit price known when this wallet submitted a public send; never back-filled.
    pub submitted_price: Option<SubmittedPrice>,
    pub summary: String,
    pub token: Option<String>,
    /// The canonical address of the token `token` names (issue 643); never without `token`.
    pub token_address: Option<String>,
    /// The token fee the person signed on a gasless transaction (`userFee`).
    pub user_fee: Option<UserFee>,
}

fn optional_detail(value: Option<&String>) -> bool {
    value.is_none_or(|v| (1..=200).contains(&utf16_len(v)))
}

/// `isActivityEntry`.
pub fn is_activity_entry(entry: &ActivityEntry) -> bool {
    is_account_id_for_network(&entry.account_id, entry.network)
        && is_stark_field_element(&entry.hash)
        && entry.submitted_at <= MAX_SAFE_INTEGER
        && (1..=MAX_SUMMARY_LENGTH).contains(&utf16_len(&entry.summary))
        && optional_detail(entry.amount.as_ref())
        && optional_detail(entry.counterparty.as_ref())
        && optional_detail(entry.token.as_ref())
        && is_recorded_token_address(entry.token_address.as_deref(), entry.token.as_deref())
        && entry.not_found_count.is_none_or(|n| n <= MAX_SAFE_INTEGER)
}

pub fn entry_to_json(entry: &ActivityEntry) -> Value {
    let mut map = Map::new();
    map.insert("accountId".into(), json!(entry.account_id));
    if let Some(v) = &entry.amount {
        map.insert("amount".into(), json!(v));
    }
    if let Some(v) = &entry.counterparty {
        map.insert("counterparty".into(), json!(v));
    }
    if let Some(v) = entry.direction {
        map.insert("direction".into(), json!(direction_text(v)));
    }
    if let Some(v) = entry.failure_notice {
        map.insert(
            "failureNotice".into(),
            json!(if v == FailureNotice::Pending {
                "pending"
            } else {
                "delivered"
            }),
        );
    }
    if let Some(fee) = &entry.fee_paid {
        map.insert("feePaid".into(), fee_json(fee));
    }
    if entry.fee_payer.is_some() {
        map.insert("feePayer".into(), json!("sponsor"));
    }
    if let Some(v) = entry.finalized_on_l1 {
        map.insert("finalizedOnL1".into(), json!(v));
    }
    map.insert("hash".into(), json!(entry.hash));
    map.insert("kind".into(), json!(kind_text(entry.kind)));
    map.insert("networkId".into(), json!(network_code(entry.network)));
    if let Some(v) = entry.not_found_count {
        map.insert("notFoundCount".into(), json!(v));
    }
    if entry.origin.is_some() {
        map.insert("origin".into(), json!("wallet"));
    }
    map.insert("status".into(), json!(status_text(entry.status)));
    map.insert("submittedAt".into(), json!(entry.submitted_at));
    if let Some(price) = &entry.submitted_price {
        map.insert("submittedPrice".into(), price_json(price));
    }
    map.insert("summary".into(), json!(entry.summary));
    if let Some(v) = &entry.token {
        map.insert("token".into(), json!(v));
    }
    if let Some(v) = &entry.token_address {
        map.insert("tokenAddress".into(), json!(v));
    }
    if let Some(fee) = &entry.user_fee {
        map.insert("userFee".into(), user_fee_json(fee));
    }
    Value::Object(map)
}

fn safe_count(value: &JsValue) -> Option<u64> {
    let n = value.as_f64()?;
    (n.fract() == 0.0 && (0.0..=MAX_SAFE_INTEGER as f64).contains(&n)).then_some(n as u64)
}

/// Reads a stored or fixture entry; `None` unless it passes `isActivityEntry`.
pub fn entry_from_json(value: &JsValue) -> Option<ActivityEntry> {
    const KEYS: [&str; 20] = [
        "accountId",
        "amount",
        "counterparty",
        "direction",
        "failureNotice",
        "feePaid",
        "feePayer",
        "finalizedOnL1",
        "hash",
        "kind",
        "networkId",
        "notFoundCount",
        "origin",
        "status",
        "submittedAt",
        "submittedPrice",
        "summary",
        "token",
        "tokenAddress",
        "userFee",
    ];
    if !value
        .as_object()?
        .iter()
        .all(|(k, _)| KEYS.contains(&k.as_str()))
    {
        return None;
    }
    let text = |key: &str| value.get(key).and_then(JsValue::as_str).map(str::to_string);
    let optional_text = |key: &str| match value.get(key) {
        None => Some(None),
        Some(JsValue::String(s)) => Some(Some(s.clone())),
        Some(_) => None,
    };
    // The nested fields are judged as JSON, by the store's own validators.
    let nested = |key: &str| {
        value
            .get(key)
            .and_then(|v| serde_json::from_str(&v.to_json()).ok())
    };
    let fee_paid = match value.get("feePaid") {
        None => None,
        Some(_) => Some(transaction_fee(&nested("feePaid")?)?),
    };
    // serde_json may round a double's last bit; the price is read again as JavaScript reads it.
    let submitted = match value.get("submittedPrice") {
        None => None,
        Some(price) => Some(SubmittedPrice {
            unit_price: price.get("unitPrice")?.as_f64()?,
            ..submitted_price(&nested("submittedPrice")?)?
        }),
    };
    let entry = ActivityEntry {
        account_id: text("accountId")?,
        amount: optional_text("amount")?,
        counterparty: optional_text("counterparty")?,
        direction: match optional_text("direction")? {
            None => None,
            Some(text) => Some(parse_direction(&text)?),
        },
        failure_notice: match value.get("failureNotice") {
            None => None,
            Some(JsValue::String(s)) if s == "pending" => Some(FailureNotice::Pending),
            Some(JsValue::String(s)) if s == "delivered" => Some(FailureNotice::Delivered),
            Some(_) => return None,
        },
        fee_paid,
        fee_payer: match optional_text("feePayer")?.as_deref() {
            None => None,
            Some("sponsor") => Some(FeePayer::Sponsor),
            Some(_) => return None,
        },
        finalized_on_l1: match value.get("finalizedOnL1") {
            None => None,
            Some(JsValue::Bool(b)) => Some(*b),
            Some(_) => return None,
        },
        hash: text("hash")?,
        kind: parse_kind(&text("kind")?)?,
        network: network_from_code(&text("networkId")?)?,
        not_found_count: match value.get("notFoundCount") {
            None => None,
            Some(v) => Some(safe_count(v)?),
        },
        origin: match optional_text("origin")?.as_deref() {
            None => None,
            Some("wallet") => Some(ActivityOrigin::Wallet),
            Some(_) => return None,
        },
        status: parse_status(&text("status")?)?,
        submitted_at: safe_count(value.get("submittedAt")?)?,
        submitted_price: submitted,
        summary: text("summary")?,
        token: optional_text("token")?,
        token_address: optional_text("tokenAddress")?,
        user_fee: match value.get("userFee") {
            None => None,
            Some(_) => Some(user_fee(&nested("userFee")?)?),
        },
    };
    is_activity_entry(&entry).then_some(entry)
}

/// One ERC-20 transfer relative to the account (`DecodedTransfer`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedTransfer {
    pub amount_raw: String,
    pub block_number: u64,
    pub counterparty: String,
    pub incoming: bool,
    pub hash: String,
}

/// `onChainActivityEntry`: an accepted transfer row dated by its block.
pub fn on_chain_activity_entry(
    account_id: &str,
    decoded: &DecodedTransfer,
    network: NetworkId,
    timestamp_ms: u64,
    token: &TokenConfig,
) -> ActivityEntry {
    let amount = crate::rpc::format_amount(&decoded.amount_raw, token.decimals, token.decimals)
        .unwrap_or_else(|_| decoded.amount_raw.clone());
    let short = address_label(&decoded.counterparty);
    let summary = if decoded.incoming {
        format!("Received {amount} {} from {short}", token.symbol)
    } else {
        format!("Sent {amount} {} to {short}", token.symbol)
    };
    ActivityEntry {
        account_id: account_id.to_string(),
        amount: Some(amount),
        counterparty: Some(decoded.counterparty.clone()),
        direction: Some(if decoded.incoming {
            TransferDirection::In
        } else {
            TransferDirection::Out
        }),
        failure_notice: None,
        fee_payer: None,
        user_fee: None,
        fee_paid: None,
        finalized_on_l1: None,
        hash: decoded.hash.clone(),
        kind: ActivityKind::Transfer,
        network,
        not_found_count: None,
        origin: None,
        status: TxStatus::Accepted,
        submitted_at: timestamp_ms,
        submitted_price: None,
        summary: utf16_slice(&summary, 0, MAX_SUMMARY_LENGTH),
        token: Some(token.symbol.clone()),
        token_address: activity_token_address(&token.address),
    }
}

fn tx_key(entry: &ActivityEntry) -> String {
    format!(
        "{}:{}",
        network_code(entry.network),
        canonical_felt(&entry.hash).unwrap_or_else(|| entry.hash.clone())
    )
}

/// The per-leg key chain rows de-duplicate on (`legKey` without the network prefix).
pub fn leg_suffix(entry: &ActivityEntry) -> String {
    format!(
        "\u{0}{}\u{0}{}\u{0}{}\u{0}{}",
        entry.token.as_deref().unwrap_or(""),
        entry.token_address.as_deref().unwrap_or(""),
        entry.counterparty.as_deref().unwrap_or(""),
        entry.amount.as_deref().unwrap_or("")
    )
}

fn leg_key(entry: &ActivityEntry) -> String {
    format!("{}{}", tx_key(entry), leg_suffix(entry))
}

/// `mergeActivity`: every row is keyed per leg, local rows included, so a stored multi-leg
/// transaction keeps all its legs (Sherlock 204). A transaction the wallet recorded owns every
/// chain leg with its hash, and a chain leg never changes a local row's status: a reverted
/// transaction still emits its fee `Transfer`, so only the status re-poll settles a row (#653).
/// Newest first.
pub fn merge_activity(local: &[ActivityEntry], on_chain: &[ActivityEntry]) -> Vec<ActivityEntry> {
    let mut keys: HashSet<String> = HashSet::new();
    let mut local_txs: HashSet<String> = HashSet::new();
    let mut merged: Vec<ActivityEntry> = Vec::new();
    for entry in local {
        if keys.insert(leg_key(entry)) {
            merged.push(entry.clone());
            local_txs.insert(tx_key(entry));
        }
    }
    for entry in on_chain {
        if !local_txs.contains(&tx_key(entry)) && keys.insert(leg_key(entry)) {
            merged.push(entry.clone());
        }
    }
    merged.sort_by_key(|e| std::cmp::Reverse(e.submitted_at));
    merged
}

/// `TRANSFER_EVENT_KEY = hash.getSelectorFromName("Transfer")`; checked by `kms::selector`
/// in the tests.
pub const TRANSFER_EVENT_KEY: &str =
    "0x99cd8bde557814842a3121e8ddfd433a539b8c9f14bf31ebf108d12e6196e9";

/// `decodeTransferEvent(event, account)`.
pub fn decode_transfer_event(event: &JsValue, account: &str) -> Option<DecodedTransfer> {
    let keys = event.get("keys")?.as_array()?;
    let data = event.get("data")?.as_array()?;
    if !same_felt_value(keys.first(), TRANSFER_EVENT_KEY) {
        return None;
    }
    let indexed = keys.len() >= 3;
    let (raw_from, raw_to) = if indexed {
        (keys.get(1), keys.get(2))
    } else {
        (data.first(), data.get(1))
    };
    let (low, high) = if indexed {
        (data.first(), data.get(1))
    } else {
        (data.get(2), data.get(3))
    };
    let (amount_low, amount_high) = uint256_felts(low, high)?;
    // Audit #163: zero in either direction is planted (`transfer_from` of 0 needs no allowance).
    if amount_low == 0 && amount_high == 0 {
        return None;
    }
    let from = canonical_felt(raw_from?.as_str()?)?;
    let to = canonical_felt(raw_to?.as_str()?)?;
    if !is_stark_field_element(&from) || !is_stark_field_element(&to) {
        return None;
    }
    let hash = canonical_felt(event.get("transaction_hash")?.as_str()?)?;
    let block = event
        .get("block_number")?
        .as_f64()
        .filter(|n| is_safe_integer(*n) && *n >= 0.0)?;
    let incoming = if same_felt(&from, account) {
        false
    } else if same_felt(&to, account) {
        true
    } else {
        return None;
    };
    Some(DecodedTransfer {
        amount_raw: crate::rpc::u256_to_decimal(amount_low, amount_high),
        block_number: block as u64,
        counterparty: if incoming { from } else { to },
        incoming,
        hash,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfer_key_is_starknet_keccak() {
        assert_eq!(
            crate::felt::canonical(&crate::selector("Transfer")),
            TRANSFER_EVENT_KEY
        );
    }
}
