//! Each guard of the reference on its own (the extension's cases are G7h's `ceiling` block, read
//! in `bramble-core`'s `gasless_parity.rs`).

use super::*;
use crate::gasless::tests::POLICY;
use crate::portfolio::model::DisplayCurrency;
use crate::portfolio::prices::{PricePoint, PRICE_TTL_MS};

const STRK: &str = "0x4718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d";
const USDC: &str = "0x33068f6539f8e6e6b131e6b2b814e6c34a5224bc66947c47dab9dfee93b35fb";

fn felt_of(text: &str) -> Felt {
    felt::parse(text).unwrap()
}

fn fee() -> U256 {
    U256::from_u128(10_000_000_000_000_000)
}

fn view(prices: &[(&str, f64)]) -> PriceView {
    PriceView {
        currency: DisplayCurrency::Usd,
        prices: prices
            .iter()
            .map(|(address, value)| PricePoint {
                address: (*address).into(),
                value: *value,
            })
            .collect(),
        changes: Vec::new(),
        as_of_ms: Some(AS_OF as f64),
        stale: false,
    }
}

fn reference(prices: &PriceView) -> Option<U256> {
    fee_reference(
        &POLICY,
        prices,
        (felt_of(STRK), 18),
        (felt_of(USDC), 6),
        fee(),
        AS_OF + 1,
    )
}

/// When the test answer was captured, in ms.
const AS_OF: u64 = 1_790_000_000_000;

#[test]
fn a_fresh_usd_answer_converts_the_fee_rounding_down() {
    // 0.01 STRK at $0.5 in USDC at $1: 5000 base units.
    let fresh = view(&[(STRK, 0.5), (USDC, 1.0)]);
    assert_eq!(reference(&fresh), Some(U256::from_u128(5000)));
    // 0.01 STRK at $0.5 in USDC at $3: 1666.66.. rounds down.
    let dear = view(&[(STRK, 0.5), (USDC, 3.0)]);
    assert_eq!(reference(&dear), Some(U256::from_u128(1666)));
}

#[test]
fn prices_match_by_felt_not_by_spelling() {
    let padded = "0x04718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d";
    let fresh = view(&[(padded, 0.5), (USDC, 1.0)]);
    assert_eq!(reference(&fresh), Some(U256::from_u128(5000)));
}

#[test]
fn an_answer_the_feed_does_not_vouch_for_is_no_reference() {
    let fresh = view(&[(STRK, 0.5), (USDC, 1.0)]);
    let stale = PriceView {
        stale: true,
        ..fresh.clone()
    };
    let undated = PriceView {
        as_of_ms: None,
        ..fresh.clone()
    };
    // The price book's TTL bounds the answer's age against the clock, both ways.
    let old = PriceView {
        as_of_ms: Some((AS_OF + 1) as f64 - PRICE_TTL_MS),
        ..fresh.clone()
    };
    let edge = PriceView {
        as_of_ms: Some((AS_OF + 2) as f64 - PRICE_TTL_MS),
        ..fresh.clone()
    };
    let future = PriceView {
        as_of_ms: Some((AS_OF + 2) as f64),
        ..fresh.clone()
    };
    assert!(reference(&edge).is_some());
    let euros = PriceView {
        currency: DisplayCurrency::Eur,
        ..fresh.clone()
    };
    assert!(reference(&fresh).is_some());
    for refused in [stale, undated, euros, old, future] {
        assert_eq!(reference(&refused), None);
    }
}

#[test]
fn a_missing_or_unusable_price_on_either_leg_is_no_reference() {
    for prices in [
        view(&[(USDC, 1.0)]),
        view(&[(STRK, 0.5)]),
        view(&[(STRK, 0.0), (USDC, 1.0)]),
        view(&[(STRK, 0.5), (USDC, -1.0)]),
        view(&[(STRK, f64::NAN), (USDC, 1.0)]),
        view(&[(STRK, 0.5), (USDC, f64::INFINITY)]),
        // Rounds to zero at the 1e6 scale.
        view(&[(STRK, 0.000_000_4), (USDC, 1.0)]),
    ] {
        assert_eq!(reference(&prices), None, "{prices:?}");
    }
}

#[test]
fn scaled_price_is_javascripts_math_round_inside_the_safe_range() {
    assert_eq!(scaled_price(0.5, 1_000_000), Some(500_000));
    assert_eq!(scaled_price(0.000_000_5, 1_000_000), Some(1));
    assert_eq!(scaled_price(0.000_000_499, 1_000_000), None);
    // Exact halves round up, as `Math.round` does.
    assert_eq!(scaled_price(1.25, 2), Some(3));
    assert_eq!(scaled_price(0.75, 2), Some(2));
    assert_eq!(
        scaled_price(9_007_199_254_740_991.0, 1),
        Some((1 << 53) - 1)
    );
    assert_eq!(scaled_price(9_007_199_254_740_992.0, 1), None);
    assert_eq!(scaled_price(-0.5, 1_000_000), None);
}

#[test]
fn the_conversion_refuses_each_degenerate_input() {
    let one = U256::from_u128(1);
    assert_eq!(convert_strk_fee_to_token(one, 1, 1, 0, 0), Some(one));
    assert_eq!(convert_strk_fee_to_token(U256::ZERO, 1, 1, 0, 0), None);
    assert_eq!(convert_strk_fee_to_token(one, 0, 1, 0, 0), None);
    assert_eq!(convert_strk_fee_to_token(one, 1, 0, 0, 0), None);
    assert_eq!(
        convert_strk_fee_to_token(one, 1, 1, 36, 36),
        Some(one),
        "36 decimals on both legs is a real width"
    );
    assert_eq!(convert_strk_fee_to_token(one, 1, 1, 37, 0), None);
    assert_eq!(convert_strk_fee_to_token(one, 1, 1, 0, 37), None);
}

#[test]
fn the_conversion_divides_by_both_legs_of_the_denominator() {
    // 7 * 3 * 10^2 / (2 * 10^1) = 105: both the price and the decimals divide.
    let seven = U256::from_u128(7);
    assert_eq!(
        convert_strk_fee_to_token(seven, 3, 2, 1, 2),
        Some(U256::from_u128(105))
    );
    // floor(floor(n / 10^d) / p) is floor(n / (p * 10^d)): 999 / (7 * 10) = 14.
    assert_eq!(
        convert_strk_fee_to_token(U256::from_u128(999), 1, 7, 1, 0),
        Some(U256::from_u128(14))
    );
}

#[test]
fn a_numerator_past_two_to_the_256_fails_closed() {
    let huge = U256 {
        high: u128::MAX >> 1,
        low: 0,
    };
    assert_eq!(convert_strk_fee_to_token(huge, 4, 1, 0, 0), None);
    assert_eq!(convert_strk_fee_to_token(huge, 1, 1, 0, 1), None);
    assert!(convert_strk_fee_to_token(huge, 1, 1, 0, 0).is_some());
}
