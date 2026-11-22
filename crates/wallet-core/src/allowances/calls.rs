//! The calls that change an ERC-20 allowance, and what the log files for them once a batch is
//! submitted (`allowances.ts` and the background's `allowance-recording.ts`): `approve` sets the
//! standing allowance, either spelling of `increase_allowance` adds to it, either spelling of
//! `decrease_allowance` subtracts from it. Every path that submits calls the person did not build
//! here (a dapp's invoke, a swap, a multisig execution, a revoke) files them through
//! `record_submitted`, so a grant the log never saw cannot go unlisted and unrevokable.
//!
//! An ERC-721 `approve(to, token_id)` is the same three felts as an ERC-20 `approve(spender,
//! amount)`, so the call alone cannot tell them apart; `approved_asset_kind` says what the
//! wallet's own lists can, and each consumer decides on that answer rather than assuming ERC-20.

use std::collections::HashMap;

use starknet_types_core::felt::Felt;

use super::record::{AllowanceInput, AllowanceRecord};
use super::retention::Ranking;
use super::store::{AllowanceDelta, AllowanceLog, DeltaDirection};
use crate::felt;
use crate::history::explorer::activity_account_id;
use crate::send::call_decode::{uint256_at, Known};
use crate::send::calls::Call;
use crate::send::u256::U256;
use crate::NetworkId;

/// The scope that owns what a batch granted (`AllowanceOwner`): for a multisig execution the
/// multisig, not the signer that broadcast it. The account id names the network too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowanceOwner {
    pub account_id: String,
    pub network: NetworkId,
}

impl AllowanceOwner {
    /// `allowanceOwner`: the account at `address` on `network`; `None` for a zero address.
    pub fn of(network: NetworkId, address: &Felt) -> Option<Self> {
        Some(Self {
            account_id: activity_account_id(network, felt::canonical(address)).ok()?,
            network,
        })
    }
}

/// How a call moves the standing allowance (`AllowanceChangeKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Set,
    Increase,
    Decrease,
}

/// One decoded allowance call (`DecodedAllowanceCall`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedAllowanceCall {
    pub kind: ChangeKind,
    pub spender: Felt,
    pub amount: U256,
}

/// What the wallet's lists say about an approved contract (`ApprovedAssetKind`). `Unknown` is
/// unknown, not "probably ERC-20": callers fail closed on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    /// An NFT collection this account watches.
    Collection,
    /// A token this account trusts.
    Fungible,
    Unknown,
}

/// One change for the log (`AllowanceChange`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowanceChange {
    pub kind: ChangeKind,
    pub input: AllowanceInput,
}

// `changeKind`, matched exactly (no case folding): the entrypoint is untrusted text, and an
// unknown one decodes as nothing. The grant spellings are `call_decode`'s `APPROVE`; a test holds
// the two lists together.
fn change_kind(entrypoint: &str) -> Option<ChangeKind> {
    match entrypoint {
        "approve" => Some(ChangeKind::Set),
        "increaseAllowance" | "increase_allowance" => Some(ChangeKind::Increase),
        "decreaseAllowance" | "decrease_allowance" => Some(ChangeKind::Decrease),
        _ => None,
    }
}

/// `decodeAllowanceCall`: the spender, uint256 amount and kind of an allowance call, or `None`
/// for any other entrypoint or calldata without a spender and two limbs below 2^128.
pub fn decode_allowance_call(call: &Call) -> Option<DecodedAllowanceCall> {
    let kind = change_kind(&call.entrypoint)?;
    Some(DecodedAllowanceCall {
        kind,
        spender: *call.calldata.first()?,
        amount: uint256_at(&call.calldata, 1)?,
    })
}

/// `approvedAssetKind`: a watched collection first, then a trusted token, compared by value.
pub fn approved_asset_kind(known: &Known, contract: &Felt) -> AssetKind {
    if known.watches_nft(contract) {
        AssetKind::Collection
    } else if known.token(contract).is_some() {
        AssetKind::Fungible
    } else {
        AssetKind::Unknown
    }
}

/// `allowanceChangesFromCalls`: every allowance change in `calls`, owned by `owner`. A zero
/// increase or decrease moves nothing and is dropped (a zero `approve` is kept: it is the revoke);
/// so is any call on a watched collection, whose second argument is an item id.
pub fn allowance_changes(
    calls: &[Call],
    owner: &AllowanceOwner,
    known: &Known,
) -> Vec<AllowanceChange> {
    calls
        .iter()
        .filter_map(|call| {
            let decoded = decode_allowance_call(call)?;
            if decoded.kind != ChangeKind::Set && decoded.amount == U256::ZERO {
                return None;
            }
            if approved_asset_kind(known, &call.contract_address) == AssetKind::Collection {
                return None;
            }
            Some(AllowanceChange {
                kind: decoded.kind,
                input: AllowanceInput {
                    account_id: owner.account_id.clone(),
                    network: owner.network,
                    token: felt::canonical(&call.contract_address),
                    spender: felt::canonical(&decoded.spender),
                    amount: decoded.amount,
                    lower_bound: false,
                },
            })
        })
        .collect()
}

/// `recordApproveAllowances`: files every allowance change in a submitted batch under `owner`,
/// with the hash of the transaction that carried it (so a revoke among them can settle on its
/// outcome). Best effort by design: the transaction is already on its way, so a storage failure
/// leaves that change unfiled and the rest are still tried.
pub fn record_submitted(
    log: &mut AllowanceLog,
    calls: &[Call],
    owner: &AllowanceOwner,
    known: &Known,
    transaction_hash: &str,
    ranking: &Ranking,
) {
    record_changes(
        log,
        &allowance_changes(calls, owner, known),
        transaction_hash,
        ranking,
    );
}

/// `record_submitted` for changes already read: what a plan's pending record carries for a batch
/// (`send::plan::PendingKind::Invoke`), filed under the hash the plan signed.
pub fn record_changes(
    log: &mut AllowanceLog,
    changes: &[AllowanceChange],
    transaction_hash: &str,
    ranking: &Ranking,
) {
    for AllowanceChange { kind, input } in changes {
        let direction = match kind {
            ChangeKind::Set => {
                let _ = log.record(input, Some(transaction_hash), ranking);
                continue;
            }
            ChangeKind::Increase => DeltaDirection::Add,
            ChangeKind::Decrease => DeltaDirection::Subtract,
        };
        let delta = AllowanceDelta {
            direction,
            input: input.clone(),
            transaction_hash: Some(transaction_hash.to_string()),
        };
        let _ = log.apply_delta(&delta, ranking);
    }
}

/// `standingAllowanceTotals`: `owner`'s logged amounts by (token, spender), for the reviewer's
/// unlimited-approval rule. Only trusted tokens are seeded, so an item id is never added to an
/// amount; a lower bound is seeded at its bound. Every record is one this wallet wrote.
pub fn standing_allowance_totals(
    records: &[AllowanceRecord],
    owner: &AllowanceOwner,
    known: &Known,
) -> HashMap<(Felt, Felt), U256> {
    records
        .iter()
        .filter(|r| r.account_id == owner.account_id && r.network == owner.network)
        .filter_map(|r| {
            let token = felt::parse(&r.token)?;
            let spender = felt::parse(&r.spender)?;
            (approved_asset_kind(known, &token) == AssetKind::Fungible)
                .then_some(((token, spender), r.amount))
        })
        .collect()
}
