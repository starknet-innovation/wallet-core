//! The extension's pre-sign call summaries (`decodeCall`, `recipientForFlags` and the selector
//! vocabulary in `packages/features/transactions/src/routing/verify-{decode,selectors}.ts`):
//! what one call does, in the words the review shows, or the opaque `Call <entrypoint> on …`
//! line when the selector or the calldata does not fit a known shape. Nothing here guesses.
//!
//! Calldata arrives as parsed felts: the callers validate it first (a multisig call through
//! `multisig_invoke::is_calls`), so the extension's `BigInt` leniency over strings has nothing
//! left to accept. What the wallet knows (its trusted tokens and watched NFT collections on this
//! network) comes in as [`Known`].

use starknet_types_core::felt::Felt;

use super::calls::{format_token_amount, transfer_summary};
use super::u256::U256;
use crate::catalog::TokenInfo;
use crate::felt;
use crate::js::felt::address_label;
use crate::js::felt::same_felt;

/// AVNU's `multi_route_swap` entrypoint and the felt its beneficiary sits at.
pub const AVNU_MULTI_ROUTE_SWAP: &str = "multi_route_swap";
pub const AVNU_BENEFICIARY_FELT_INDEX: usize = 8;

/// The spelling that sets an allowance; the other grants add to the one standing.
pub const APPROVE_SET: &str = "approve";
/// `APPROVE_ENTRYPOINTS` (`verify-selectors.ts`): the selectors that grant an ERC-20 allowance.
/// The one list: the swap's build policy and the allowance log read it from here.
pub const APPROVE: [&str; 3] = [APPROVE_SET, "increaseAllowance", "increase_allowance"];
/// The duals that lower an allowance (`isDecreaseAllowanceEntrypoint`): never a grant, but the
/// unlimited-approval rule subtracts them from its running total.
pub const DECREASE_ALLOWANCE: [&str; 2] = ["decreaseAllowance", "decrease_allowance"];
const OPERATOR_APPROVAL: [&str; 2] = ["setApprovalForAll", "set_approval_for_all"];
const PLAIN_TRANSFER_FROM: [&str; 2] = ["transferFrom", "transfer_from"];
const SAFE_TRANSFER_FROM: [&str; 2] = ["safeTransferFrom", "safe_transfer_from"];
const BATCH_TRANSFER_FROM: [&str; 2] = ["safeBatchTransferFrom", "safe_batch_transfer_from"];

/// What the wallet knows on the call's network: its trusted tokens (the extension's
/// `trustedTokens`, so spam and dapp-added tokens are left out by the caller) and the NFT
/// collections it watches.
#[derive(Debug, Clone, Copy, Default)]
pub struct Known<'a> {
    pub tokens: &'a [TokenInfo],
    pub nft_contracts: &'a [String],
}

impl Known<'_> {
    /// `tokenFor`: the trusted token at `contract`.
    pub(crate) fn token(&self, contract: &Felt) -> Option<&TokenInfo> {
        let contract = felt::canonical(contract);
        self.tokens
            .iter()
            .find(|token| same_felt(&token.address, &contract))
    }

    /// `isWatchedNftContract`.
    pub(crate) fn watches_nft(&self, contract: &Felt) -> bool {
        let contract = felt::canonical(contract);
        self.nft_contracts
            .iter()
            .any(|watched| same_felt(watched, &contract))
    }
}

/// `DecodedCallKind`: what a call does, in the terms the review explains. Each grant has its own
/// kind, so no risk rule infers authority from a summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallKind {
    /// Grants or raises an ERC-20 allowance (or an approve on a contract the wallet cannot place).
    Approve,
    /// Grants transfer rights over one item of a watched collection.
    NftApproval,
    /// Grants (or revokes) transfer rights over a whole collection.
    OperatorApproval,
    /// An AVNU route whose payee and floor were decoded.
    Swap,
    /// Moves value.
    Transfer,
    /// A call the wallet cannot describe.
    Unknown,
}

/// `DecodedCall`, without the contract and entrypoint the caller already holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedCall {
    pub kind: CallKind,
    pub summary: String,
}

/// `recipientFeltIndex`: which felt carries the address a call pays, from the selector alone.
pub fn recipient_felt_index(entrypoint: &str) -> Option<usize> {
    match entrypoint {
        "transfer" => Some(0),
        AVNU_MULTI_ROUTE_SWAP => Some(AVNU_BENEFICIARY_FELT_INDEX),
        _ if PLAIN_TRANSFER_FROM.contains(&entrypoint)
            || SAFE_TRANSFER_FROM.contains(&entrypoint)
            || BATCH_TRANSFER_FROM.contains(&entrypoint) =>
        {
            Some(1)
        }
        _ => None,
    }
}

/// `recipientForFlags`: the address a call pays, when its selector names one and the felt is
/// there, whether or not the call could be summarised.
pub fn recipient_for_flags(entrypoint: &str, calldata: &[Felt]) -> Option<Felt> {
    calldata.get(recipient_felt_index(entrypoint)?).copied()
}

/// `addressLabel(value)`.
fn short(value: &Felt) -> String {
    address_label(&felt::canonical(value))
}

/// `uint256Felts`: two felts each below 2^128.
pub(crate) fn uint256_at(calldata: &[Felt], low: usize) -> Option<U256> {
    let limb = |index: usize| u128::try_from(*calldata.get(index)?).ok();
    Some(U256 {
        low: limb(low)?,
        high: limb(low + 1)?,
    })
}

/// `amountLabel`: decimals and symbol for a known token, base units otherwise.
fn amount_label(amount: &U256, token: Option<&TokenInfo>) -> String {
    match token {
        Some(token) => format!(
            "{} {}",
            format_token_amount(amount, token.decimals),
            token.symbol
        ),
        None => format!("{} units", amount.to_decimal()),
    }
}

/// Whether `calldata[index]` is the length of everything after it.
fn tail_length_at(calldata: &[Felt], index: usize) -> bool {
    calldata.len() > index && calldata[index] == Felt::from(calldata.len() - index - 1)
}

fn transfer_from(contract: &Felt, calldata: &[Felt], known: &Known) -> Option<String> {
    if calldata.len() != 4 {
        return None;
    }
    let amount = uint256_at(calldata, 2)?;
    let to = short(&calldata[1]);
    Some(match known.token(contract) {
        Some(token) => format!(
            "Send {} from {} to {to}",
            amount_label(&amount, Some(token)),
            short(&calldata[0])
        ),
        None => format!("Send NFT #{} to {to}", amount.to_decimal()),
    })
}

fn safe_transfer(calldata: &[Felt]) -> Option<String> {
    let recipient = calldata.get(1)?;
    let id = uint256_at(calldata, 2)?.to_decimal();
    let value = if tail_length_at(calldata, 6) {
        uint256_at(calldata, 4)
    } else {
        None
    };
    let erc721 = tail_length_at(calldata, 4);
    let party = short(recipient);
    match (value, erc721) {
        (Some(_), true) => Some(format!("Safe transfer of NFT #{id} to {party}")),
        (Some(value), false) => Some(format!(
            "Send {} of NFT #{id} to {party}",
            value.to_decimal()
        )),
        (None, true) => Some(format!("Send NFT #{id} to {party}")),
        (None, false) => None,
    }
}

fn batch_transfer(contract: &Felt, calldata: &[Felt]) -> Option<String> {
    let recipient = calldata.get(1)?;
    let ids = *calldata.get(2)?;
    let count = usize::try_from(ids).ok().filter(|count| {
        count
            .checked_mul(4)
            .and_then(|n| n.checked_add(5))
            .is_some_and(|needed| needed <= calldata.len())
    })?;
    let values_index = 3 + 2 * count;
    let data_index = values_index + 1 + 2 * count;
    if calldata[values_index] != ids || !tail_length_at(calldata, data_index) {
        return None;
    }
    Some(format!(
        "Send {count} token id{} of collection {} to {}",
        if count == 1 { "" } else { "s" },
        short(contract),
        short(recipient)
    ))
}

fn operator_approval(contract: &Felt, calldata: &[Felt]) -> Option<String> {
    let [operator, approved] = calldata else {
        return None;
    };
    let (party, collection) = (short(operator), short(contract));
    Some(if *approved == Felt::ZERO {
        format!("Revoke {party}'s permission to transfer every item in collection {collection}")
    } else {
        format!("Allow {party} to transfer EVERY item you own in collection {collection}")
    })
}

fn transfer_or_approve(
    contract: &Felt,
    entrypoint: &str,
    calldata: &[Felt],
    known: &Known,
) -> Option<String> {
    if calldata.len() != 3 {
        return None;
    }
    let amount = uint256_at(calldata, 1)?;
    let token = known.token(contract);
    let party = short(&calldata[0]);
    Some(if entrypoint == "transfer" {
        match token {
            Some(token) => transfer_summary(&amount, token.decimals, &token.symbol, &calldata[0]),
            None => format!("Send {} to {party}", amount_label(&amount, None)),
        }
    } else if known.watches_nft(contract) {
        format!(
            "Allow {party} to transfer NFT #{} of collection {}",
            amount.to_decimal(),
            short(contract)
        )
    } else if token.is_none() {
        format!(
            "Authorize {party} for amount or NFT #{} on unknown contract {}",
            amount.to_decimal(),
            short(contract)
        )
    } else {
        let extra = if entrypoint == APPROVE_SET {
            ""
        } else {
            " more"
        };
        format!(
            "Allow {party} to spend {}{extra}",
            amount_label(&amount, token)
        )
    })
}

/// `routeAmountLabel`: a known token's amount, or base units of the token's address.
fn route_amount(amount: &U256, token: &Felt, known: &Known) -> String {
    match known.token(token) {
        Some(found) => amount_label(amount, Some(found)),
        None => format!("{} units of {}", amount.to_decimal(), short(token)),
    }
}

fn avnu_swap(calldata: &[Felt], known: &Known) -> Option<String> {
    let route = super::avnu_route::decode(calldata)?;
    Some(format!(
        "Swap {} for at least {}, paid to {}",
        route_amount(&route.sell_amount, &route.sell_token, known),
        route_amount(&route.min_buy_amount, &route.buy_token, known),
        short(&route.beneficiary)
    ))
}

/// `decodeCall`: the kind and review line for one call. A selector it does not know, or calldata
/// that does not fit the shape, is `Unknown` with the opaque line.
pub fn decode_call(
    contract: &Felt,
    entrypoint: &str,
    calldata: &[Felt],
    known: &Known,
) -> DecodedCall {
    let decoded = match entrypoint {
        "transfer" => transfer_or_approve(contract, entrypoint, calldata, known)
            .map(|summary| (CallKind::Transfer, summary)),
        AVNU_MULTI_ROUTE_SWAP => {
            avnu_swap(calldata, known).map(|summary| (CallKind::Swap, summary))
        }
        _ if APPROVE.contains(&entrypoint) => {
            let kind = if known.watches_nft(contract) {
                CallKind::NftApproval
            } else {
                CallKind::Approve
            };
            transfer_or_approve(contract, entrypoint, calldata, known)
                .map(|summary| (kind, summary))
        }
        _ if OPERATOR_APPROVAL.contains(&entrypoint) => operator_approval(contract, calldata)
            .map(|summary| (CallKind::OperatorApproval, summary)),
        _ if PLAIN_TRANSFER_FROM.contains(&entrypoint) => {
            transfer_from(contract, calldata, known).map(|summary| (CallKind::Transfer, summary))
        }
        _ if SAFE_TRANSFER_FROM.contains(&entrypoint) => {
            safe_transfer(calldata).map(|summary| (CallKind::Transfer, summary))
        }
        _ if BATCH_TRANSFER_FROM.contains(&entrypoint) => {
            batch_transfer(contract, calldata).map(|summary| (CallKind::Transfer, summary))
        }
        _ => None,
    };
    let (kind, summary) = decoded.unwrap_or_else(|| {
        (
            CallKind::Unknown,
            format!("Call {entrypoint} on {}", short(contract)),
        )
    });
    DecodedCall { kind, summary }
}

/// `decodeCall(...).summary`: the review line for one call.
pub fn call_summary(contract: &Felt, entrypoint: &str, calldata: &[Felt], known: &Known) -> String {
    decode_call(contract, entrypoint, calldata, known).summary
}

#[cfg(test)]
#[path = "call_decode_tests.rs"]
mod tests;
