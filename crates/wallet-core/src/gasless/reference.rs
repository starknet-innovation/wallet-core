//! The independent reference a gasless fee is bounded by: what the wallet's STRK fee estimate is
//! worth in the fee token's base units, at the wallet's own fiat prices for both, never the
//! paymaster's (`convertStrkFeeToToken` in `gasless-ceiling.ts`, priced as the extension's
//! `reviewGaslessOffers` and `executeReviewedGasless` price it, through
//! `strk20-fee-reference.ts`). It rounds down; `GaslessPolicy::fee_ceiling` adds the margin.
//! Every unknown is `None`, which hides the token from an offer and refuses a submission as
//! `Unverifiable`.

use super::GaslessPolicy;
use crate::portfolio::prices::PriceView;
use crate::send::u256::U256;
use crate::{felt, Felt};

/// `MAX_TOKEN_DECIMALS`: wider than any ERC-20 declares.
pub const MAX_TOKEN_DECIMALS: u8 = 36;

/// `Number.MAX_SAFE_INTEGER`.
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

/// `convertStrkFeeToToken`: `strk_fee * strk_price * 10^token_decimals / (token_price *
/// 10^strk_decimals)`, rounded down, with both prices scaled integers of one currency. `None`
/// for a zero fee or price, or decimals past [`MAX_TOKEN_DECIMALS`]. Also `None` when the
/// numerator passes 2^256, where the extension's `bigint` would go on: that needs a fee of
/// millions of STRK at any real price, so failing closed refuses nothing a wallet estimates.
pub fn convert_strk_fee_to_token(
    strk_fee: U256,
    strk_price: u64,
    token_price: u64,
    strk_decimals: u8,
    token_decimals: u8,
) -> Option<U256> {
    if strk_fee == U256::ZERO
        || strk_price == 0
        || token_price == 0
        || strk_decimals > MAX_TOKEN_DECIMALS
        || token_decimals > MAX_TOKEN_DECIMALS
    {
        return None;
    }
    let mut value = strk_fee.checked_mul_small(strk_price)?;
    for _ in 0..token_decimals {
        value = value.checked_mul_small(10)?;
    }
    // floor(n / (p * 10^d)) = floor(floor(n / 10^d) / p) for positive integers.
    for _ in 0..strk_decimals {
        value = value.checked_div_rem_small(10)?.0;
    }
    Some(value.checked_div_rem_small(token_price)?.0)
}

/// `scaledPrice`: `Math.round(value * scale)`, a positive safe integer, or `None`. Rust's
/// `round` is JavaScript's for every positive value (half away from zero is half up there).
pub fn scaled_price(value: f64, scale: u64) -> Option<u64> {
    if !value.is_finite() || value <= 0.0 {
        return None;
    }
    let scaled = (value * scale as f64).round();
    (1.0..=MAX_SAFE_INTEGER)
        .contains(&scaled)
        .then_some(scaled as u64)
}

/// `createStrk20FeeReference`'s answer for `token`: `strk_fee` FRI converted at `prices`, a
/// price answer in the policy's reference currency (pinned, so a display choice cannot move a
/// security bound) that the feed vouches for as current. A stale or undated answer, a missing or
/// non-positive price for either token, or a zero fee is `None`. Prices are matched by felt, so
/// a padded address is not "unpriced". `strk` and `token` are `(address, decimals)` from the
/// wallet's own registry, never the paymaster's list.
pub fn fee_reference(
    policy: &GaslessPolicy,
    prices: &PriceView,
    strk: (Felt, u8),
    token: (Felt, u8),
    strk_fee: U256,
) -> Option<U256> {
    if prices.currency != policy.reference_currency || prices.stale || prices.as_of_ms.is_none() {
        return None;
    }
    let price = |address: &Felt| {
        let point = prices
            .prices
            .iter()
            .find(|point| felt::parse(&point.address).as_ref() == Some(address))?;
        scaled_price(point.value, policy.price_scale)
    };
    convert_strk_fee_to_token(strk_fee, price(&strk.0)?, price(&token.0)?, strk.1, token.1)
}

#[cfg(test)]
#[path = "reference_tests.rs"]
mod tests;
