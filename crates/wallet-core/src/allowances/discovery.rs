//! Allowances the wallet did not grant itself, and the manager's list (`allowance-discovery.ts`
//! and the listing rules of `background/portfolio/allowance-ops.ts`): decoding an `Approval`
//! event, parsing an `allowance` reply, merging what was found on chain with the log, and what
//! the listing may claim about each row.

use std::collections::{HashMap, HashSet};

use super::record::{allowance_id, AllowanceRecord};
use crate::history::explorer::is_account_id_for_network;
use crate::js::felt::{canonical_felt, is_non_zero_felt, same_felt_value, uint256_felts};
use crate::js::json::JsValue;
use crate::send::u256::U256;
use crate::NetworkId;

/// `APPROVAL_EVENT_KEY = hash.getSelectorFromName("Approval")`; Keccak-checked in the tests.
pub const APPROVAL_EVENT_KEY: &str =
    "0x134692b230b9e1ffa39098904722134159652b09c5bc41d88d6698779d228ff";

/// Where a listed allowance came from (`AllowanceSource`): this wallet's own grant, one a
/// watched multisig granted (read-only here), or one found on chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllowanceSource {
    Wallet,
    Multisig,
    External,
}

/// What a listing could not vouch for (`ALLOWANCE_NOTICES`), so the manager never reports "none
/// found" for a list it knows is partial.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllowanceNotice {
    /// On-chain discovery stopped early or could not run (Sherlock 181).
    ScanIncomplete,
    /// A watched multisig could not be read (Sherlock 190).
    MultisigUnread,
    /// The log is at a retention cap, so smaller approvals may have been dropped (Sherlock 183).
    RetentionFull,
}

/// `listingNotices`, in the order the manager states them.
pub fn listing_notices(
    scan_complete: bool,
    multisig_unread: bool,
    retention_full: bool,
) -> Vec<AllowanceNotice> {
    [
        (!scan_complete, AllowanceNotice::ScanIncomplete),
        (multisig_unread, AllowanceNotice::MultisigUnread),
        (retention_full, AllowanceNotice::RetentionFull),
    ]
    .into_iter()
    .filter_map(|(on, notice)| on.then_some(notice))
    .collect()
}

/// One live allowance read from chain (`ExternalAllowance`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalAllowance {
    pub token: String,
    pub spender: String,
    pub amount: U256,
}

/// The live amount of one tracked record, by that record's id (`AllowanceReading`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowanceReading {
    pub id: String,
    pub amount: U256,
}

/// A row of the manager's list (`AllowanceView`): a record without its revoke hash, its source,
/// and `unconfirmed_token` when the wallet cannot say the contract is fungible (an ERC-721
/// `approve(to, id)` is the same three felts), so neither the amount nor the ERC-20 revoke may
/// be offered for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowanceView {
    pub id: String,
    pub account_id: String,
    pub network: NetworkId,
    pub token: String,
    pub spender: String,
    pub amount: U256,
    pub lower_bound: bool,
    pub revoking: bool,
    pub source: AllowanceSource,
    pub unconfirmed_token: bool,
}

impl AllowanceView {
    // A revoke's hash is bookkeeping for settling the record, never shown.
    fn of(record: &AllowanceRecord, source: AllowanceSource) -> Self {
        Self {
            id: record.id.clone(),
            account_id: record.account_id.clone(),
            network: record.network,
            token: record.token.clone(),
            spender: record.spender.clone(),
            amount: record.amount,
            lower_bound: record.lower_bound,
            revoking: record.revoking,
            source,
            unconfirmed_token: false,
        }
    }
}

/// `decodeApprovalEvent`: the owner and spender of an `Approval` event, both canonical and
/// non-zero, from the keys (Cairo 1 layout, three keys or more) or the data (legacy layout).
pub fn decode_approval_event(keys: &[JsValue], data: &[JsValue]) -> Option<(String, String)> {
    if !same_felt_value(keys.first(), APPROVAL_EVENT_KEY) {
        return None;
    }
    let (owner, spender) = if keys.len() >= 3 {
        (keys.get(1), keys.get(2))
    } else {
        (data.first(), data.get(1))
    };
    let canonical = |value: Option<&JsValue>| {
        canonical_felt(value?.as_str()?).filter(|felt| is_non_zero_felt(felt))
    };
    Some((canonical(owner)?, canonical(spender)?))
}

/// `parseAllowanceAmount`: an `allowance` reply's `[low, high]` uint256.
pub fn parse_allowance_amount(result: &[JsValue]) -> Option<U256> {
    let (low, high) = uint256_felts(result.first(), result.get(1))?;
    Some(U256 { high, low })
}

// `withOnChainAmount`: the on-chain reading replaces the tracked figure when it is at least as
// large (never understating a spender's reach), and resolves a lower bound. A grant with a
// revoke pending takes its reading as it stands, and keeps the mark unless the reading is zero.
fn with_on_chain_amount(view: AllowanceView, on_chain: Option<U256>) -> AllowanceView {
    match on_chain {
        Some(amount) if amount >= view.amount || view.revoking => AllowanceView {
            amount,
            lower_bound: false,
            revoking: view.revoking && amount != U256::ZERO,
            ..view
        },
        _ => view,
    }
}

// `externalView`: one discovered allowance as a row for the active account, or `None` when it
// is already revoked (zero) or fails the field rules.
fn external_view(
    entry: &ExternalAllowance,
    account_id: &str,
    network: NetworkId,
) -> Option<AllowanceView> {
    if entry.amount == U256::ZERO || !is_account_id_for_network(account_id, network) {
        return None;
    }
    let token = canonical_felt(&entry.token).filter(|felt| is_non_zero_felt(felt))?;
    let spender = canonical_felt(&entry.spender).filter(|felt| is_non_zero_felt(felt))?;
    Some(AllowanceView {
        id: format!("{account_id}:{token}:{spender}"),
        account_id: account_id.to_string(),
        network,
        token,
        spender,
        amount: entry.amount,
        lower_bound: false,
        revoking: false,
        source: AllowanceSource::External,
        unconfirmed_token: false,
    })
}

/// `mergeAllowanceViews` for one network: the active account's records (`in_wallet`), then a
/// watched multisig's (`multisig`), each at its on-chain amount when that is at least as large,
/// then the discovered allowances no record already lists, zero ones omitted. A per-record
/// reading (any owner) wins over the event scan's figure (the active account's only).
pub fn merge_allowance_views(
    account_id: &str,
    network: NetworkId,
    in_wallet: &[AllowanceRecord],
    multisig: &[AllowanceRecord],
    external: &[ExternalAllowance],
    readings: &[AllowanceReading],
) -> Vec<AllowanceView> {
    let mut on_chain: HashMap<String, U256> = HashMap::new();
    for entry in external {
        if let Some(id) = allowance_id(account_id, &entry.token, &entry.spender) {
            on_chain.insert(id, entry.amount);
        }
    }
    for reading in readings {
        on_chain.insert(reading.id.clone(), reading.amount);
    }
    let mut views: Vec<AllowanceView> = in_wallet
        .iter()
        .map(|record| AllowanceView::of(record, AllowanceSource::Wallet))
        .chain(
            multisig
                .iter()
                .map(|record| AllowanceView::of(record, AllowanceSource::Multisig)),
        )
        .map(|view| {
            let amount = on_chain.get(&view.id).copied();
            with_on_chain_amount(view, amount)
        })
        .collect();
    let mut seen: HashSet<String> = views.iter().map(|view| view.id.clone()).collect();
    for entry in external {
        if let Some(view) = external_view(entry, account_id, network) {
            if seen.insert(view.id.clone()) {
                views.push(view);
            }
        }
    }
    views
}

/// `outstandingAllowances`: drops a row the chain resolved to an exact zero, and marks
/// `unconfirmed_token` on a row whose contract is neither one the wallet tracks as a token
/// (`is_fungible`) nor one the chain answered an `allowance` read for (`readings`).
pub fn outstanding_allowances(
    views: Vec<AllowanceView>,
    is_fungible: impl Fn(&str) -> bool,
    readings: &[AllowanceReading],
) -> Vec<AllowanceView> {
    let confirmed: HashSet<&str> = readings.iter().map(|r| r.id.as_str()).collect();
    views
        .into_iter()
        .filter(|view| view.amount != U256::ZERO || view.lower_bound)
        .map(|view| {
            let unconfirmed = !is_fungible(&view.token) && !confirmed.contains(view.id.as_str());
            AllowanceView {
                unconfirmed_token: unconfirmed,
                ..view
            }
        })
        .collect()
}
