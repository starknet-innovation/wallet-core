//! The gasless offer: the fee tokens a review may show (`GaslessFeeOffer`, `isGaslessFeeOffer`),
//! the tokens the batch itself spends (`gaslessTokenSpend`) and the STRK verdict that decides
//! whether an offer is made at all (`gaslessStrkAffordability`), from `gasless-request.ts` and
//! `routing/fee-funds.ts`.

use crate::felt;
use crate::send::call_decode::{uint256_at, APPROVE};
use crate::send::calls::Call;
use crate::send::fees::FeeUnit;
use crate::send::types::FeeAffordability;
use crate::send::u256::U256;
use crate::Felt;

const MAX: U256 = U256 {
    high: u128::MAX,
    low: u128::MAX,
};

/// A fee token the person can choose. `amount` is the maximum in base units as decimal digits,
/// so a later echo compares without hex padding; the label and decimals are the wallet's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GaslessFeeOffer {
    pub amount: String,
    pub decimals: u8,
    pub label: String,
    pub token_address: String,
}

impl GaslessFeeOffer {
    /// `isGaslessFeeOffer`: an offer this wallet could have rendered, for any fee token but STRK.
    /// The label grammar is the wallet's own, so a paymaster-supplied symbol is never one.
    pub fn is_valid(&self, strk: &Felt) -> bool {
        let amount = self.amount.as_bytes();
        (1..=78).contains(&amount.len())
            && amount[0] != b'0'
            && amount.iter().all(u8::is_ascii_digit)
            && self.decimals <= 36
            && (1..=32).contains(&self.label.len())
            && self
                .label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.')
            && felt::parse_canonical_non_zero(&self.token_address).is_some_and(|t| t != *strk)
    }

    /// The offered maximum; `None` for an amount outside the grammar.
    pub(super) fn max_amount(&self) -> Option<U256> {
        U256::from_decimal(&self.amount)
    }
}

/// A wallet token a quote may offer (`GaslessHeldToken`): `balance` is raw base units, or `None`
/// when it could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GaslessHeldToken {
    pub address: Felt,
    pub balance: Option<U256>,
    pub decimals: u8,
    pub label: String,
}

/// `gaslessTokenSpend`: the base units of `token` the calls move out of the account, or `None`
/// when a call to that token is neither an approval nor a known transfer, so an offer fails
/// closed instead of under-counting. A total past 2^256 saturates (the extension's `bigint` would
/// only grow, and no balance covers either).
pub fn token_spend(calls: &[Call], token: &Felt) -> Option<U256> {
    let mut spent = U256::ZERO;
    for call in calls.iter().filter(|call| call.contract_address == *token) {
        let entrypoint = call.entrypoint.as_str();
        if APPROVE.contains(&entrypoint) {
            continue;
        }
        let offset = match entrypoint {
            "transfer" => 1,
            "transferFrom" | "transfer_from" => 2,
            _ => return None,
        };
        let amount = uint256_at(&call.calldata, offset)?;
        spent = spent.checked_add(amount).unwrap_or(MAX);
    }
    Some(spent)
}

/// `resolveTokenFeeAffordability`: whether `balance` covers `fee` plus `spent` of the same
/// token. Only `Sufficient` may be offered.
pub(super) fn token_affordability(
    fee: U256,
    balance: Option<U256>,
    spent: U256,
) -> FeeAffordability {
    let Some(balance) = balance else {
        return FeeAffordability::Unknown;
    };
    match fee.checked_add(spent) {
        Some(required) if balance >= required => FeeAffordability::Sufficient,
        _ => FeeAffordability::Insufficient,
    }
}

/// `gaslessStrkAffordability`: whether STRK covers the network fee (`fee`, its overall amount and
/// unit) plus the STRK the calls themselves spend. Only `Insufficient` opens a gasless offer: a
/// transfer of the whole balance is short even though the fee alone would fit. An unrecognised
/// call to STRK, a non-FRI fee or an unread balance is `Unknown`, and no offer is made.
pub fn strk_affordability(
    calls: &[Call],
    strk: &Felt,
    balance: Option<U256>,
    fee: Option<(U256, FeeUnit)>,
) -> FeeAffordability {
    let Some(spent) = token_spend(calls, strk) else {
        return FeeAffordability::Unknown;
    };
    match fee {
        Some((fee, FeeUnit::Fri)) => token_affordability(fee, balance, spent),
        _ => FeeAffordability::Unknown,
    }
}
