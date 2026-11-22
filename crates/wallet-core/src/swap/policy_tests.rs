//! The build policy against the extension's own cases (`swap-build-policy.test.ts`,
//! `swap-route-binding.test.ts`, `swap-ops.test.ts`, `avnu-route.test.ts`): each holds the
//! reviewed swap constant and changes one thing, so each refusal proves its guard alone.

use super::super::fixtures::*;
use super::*;
use BuildPolicyError as E;

fn intent() -> Intent {
    Intent {
        sell_token: felt(ETH),
        sell_amount: U256::from_u128(SELL_AMOUNT),
        buy_token: felt(STRK),
        taker: felt(ACCOUNT),
        router: felt(ROUTER),
        min_buy_amount: reviewed_min_buy_amount(U256::from_u128(BUY_AMOUNT), 0.005, 1).unwrap(),
        max_calls: 4,
        max_fee_bps: 100,
    }
}

fn judge(calls: &[Call]) -> Result<AvnuRoute, BuildPolicyError> {
    build_policy(calls, &intent())
}

fn with(change: impl FnOnce(&mut Route)) -> Call {
    let mut route = Route::honest();
    change(&mut route);
    route.call()
}

#[test]
fn the_canonical_approve_then_route_build_passes() {
    let route = judge(&[approve_router(), route_call()]).unwrap();
    assert_eq!(route.min_buy_amount, U256::from_u128(MIN_BUY_AMOUNT));
    assert_eq!(route.beneficiary, felt(ACCOUNT));
}

#[test]
fn a_call_on_a_contract_that_is_not_the_router_is_refused() {
    let drain = call(ATTACKER, "drain", &["0x1"]);
    assert_eq!(
        judge(&[approve_router(), route_call(), drain]),
        Err(E::NotRouter)
    );
}

#[test]
fn an_aggregator_chosen_spender_is_refused_even_when_every_call_agrees() {
    let approve = call(
        ETH,
        "approve",
        &[AGGREGATOR_CHOSEN, &hex(SELL_AMOUNT), "0x0"],
    );
    let operator = call(
        AGGREGATOR_CHOSEN,
        "set_approval_for_all",
        &[ATTACKER, "0x1"],
    );
    assert_eq!(judge(&[approve, operator]), Err(E::UnexpectedSpender));
    // A tracked non-sell token, or this account, named as spender.
    for spender in [STRK, ACCOUNT] {
        let approve = call(ETH, "approve", &[spender, &hex(SELL_AMOUNT), "0x0"]);
        let other = call(spender, "burn", &["0x1", "0x0"]);
        assert_eq!(judge(&[approve, other]), Err(E::UnexpectedSpender));
    }
}

#[test]
fn only_the_pinned_swap_entrypoint_is_admitted_on_the_router() {
    for entrypoint in [
        "set_approval_for_all",
        "upgrade",
        "collect_fees",
        "swap_exact_token_to",
    ] {
        let mut other = route_call();
        other.entrypoint = entrypoint.into();
        assert_eq!(judge(&[approve_router(), other]), Err(E::NotSwapEntrypoint));
    }
    // A two-felt transfer the verifier cannot decode is still not a swap.
    let transfer = call(ROUTER, "transfer", &[ATTACKER, &hex(SELL_AMOUNT)]);
    assert_eq!(
        judge(&[approve_router(), transfer]),
        Err(E::NotSwapEntrypoint)
    );
}

#[test]
fn a_build_over_the_call_cap_is_refused() {
    let calls = [
        approve_router(),
        route_call(),
        route_call(),
        route_call(),
        route_call(),
    ];
    assert_eq!(judge(&calls), Err(E::TooManyCalls));
}

#[test]
fn approvals_must_be_one_exact_approve_of_the_sell_token() {
    assert_eq!(
        judge(&[approve_router(), approve_router(), route_call()]),
        Err(E::MultipleApprovals)
    );
    for entrypoint in ["increase_allowance", "increaseAllowance"] {
        let mut additive = approve_router();
        additive.entrypoint = entrypoint.into();
        assert_eq!(judge(&[additive, route_call()]), Err(E::AdditiveAllowance));
    }
    let other_token = call(
        STRK,
        "approve",
        &["0x1", "0xffffffffffffffffffffffff", "0x0"],
    );
    assert_eq!(judge(&[other_token]), Err(E::UnexpectedApprovalToken));
    assert_eq!(judge(&[route_call()]), Err(E::MissingApproval));
    assert_eq!(judge(&[approve_router()]), Err(E::NoSwapCall));
}

#[test]
fn the_approved_amount_is_exactly_the_quoted_one() {
    let larger = call(
        ETH,
        "approve",
        &["0x1", "0xffffffffffffffffffffffff", "0x0"],
    );
    let wide_limb = call(ETH, "approve", &[ROUTER, &format!("0x1{:032x}", 0), "0x0"]);
    let high_limb = call(ETH, "approve", &[ROUTER, &hex(SELL_AMOUNT), "0x1"]);
    let short = call(ETH, "approve", &[ROUTER, &hex(SELL_AMOUNT)]);
    let long = call(ETH, "approve", &[ROUTER, &hex(SELL_AMOUNT), "0x0", "0x0"]);
    for approve in [larger, wide_limb, high_limb, short, long] {
        assert_eq!(judge(&[approve, route_call()]), Err(E::UnexpectedAllowance));
    }
}

#[test]
fn a_direct_transfer_anywhere_in_the_build_is_refused() {
    for contract in [ETH, ROUTER] {
        let transfer = call(contract, "transfer", &[ATTACKER, &hex(SELL_AMOUNT), "0x0"]);
        assert_eq!(
            judge(&[approve_router(), route_call(), transfer]),
            Err(E::DirectTransfer)
        );
    }
    let transfer_from = call(ETH, "transfer_from", &[ACCOUNT, ATTACKER, "0x1", "0x0"]);
    assert_eq!(
        judge(&[approve_router(), route_call(), transfer_from]),
        Err(E::DirectTransfer)
    );
    // Only a transfer the decoder reads is one; a malformed one falls to the route allowlist.
    let malformed = call(ROUTER, "transfer", &[ATTACKER, &hex(SELL_AMOUNT)]);
    assert_eq!(
        judge(&[approve_router(), route_call(), malformed]),
        Err(E::NotSwapEntrypoint)
    );
}

#[test]
fn a_second_swap_call_is_refused() {
    assert_eq!(
        judge(&[approve_router(), route_call(), route_call()]),
        Err(E::MultipleSwapCalls)
    );
}

#[test]
fn each_route_field_is_bound_to_the_reviewed_swap() {
    let cases = [
        (
            with(|r| r.beneficiary = felt(ATTACKER)),
            E::BeneficiaryMismatch,
        ),
        (with(|r| r.min_buy_amount = U256::ZERO), E::BelowMinimum),
        (with(|r| r.buy_token = felt(ATTACKER)), E::BuyTokenMismatch),
        (
            with(|r| {
                r.sell_token = felt(STRK);
                r.sell_amount = U256::from_u128(10u128.pow(24));
            }),
            E::SellTokenMismatch,
        ),
        (
            with(|r| r.sell_amount = U256::from_u128(SELL_AMOUNT + 1)),
            E::SellAmountMismatch,
        ),
        // A uint256 is its two limbs joined, never its low half.
        (with(|r| r.sell_amount.high = 1), E::SellAmountMismatch),
        (
            with(|r| {
                r.fee_bps = Felt::from(10_000u16);
                r.fee_recipient = felt(ATTACKER);
            }),
            E::FeeAboveCap,
        ),
    ];
    for (route, error) in cases {
        assert_eq!(judge(&[approve_router(), route]), Err(error));
    }
}

#[test]
fn the_minimum_output_floor_is_exact_to_one_base_unit() {
    let floor = BUY_AMOUNT * 9_949 / 10_000;
    let at = with(|r| r.min_buy_amount = U256::from_u128(floor));
    assert!(judge(&[approve_router(), at]).is_ok());
    let below = with(|r| r.min_buy_amount = U256::from_u128(floor - 1));
    assert_eq!(judge(&[approve_router(), below]), Err(E::BelowMinimum));
    // More than the floor is always admitted.
    let generous = with(|r| r.min_buy_amount = U256::from_u128(BUY_AMOUNT));
    assert!(judge(&[approve_router(), generous]).is_ok());
}

#[test]
fn the_integrator_fee_is_capped_at_the_policy_ceiling() {
    let fee = |bps: Felt| judge(&[approve_router(), with(|r| r.fee_bps = bps)]);
    assert!(fee(Felt::from(100u8)).is_ok());
    assert_eq!(fee(Felt::from(101u8)), Err(E::FeeAboveCap));
    // Wider than the ABI's u128 is above any cap.
    assert_eq!(fee(Felt::from(u128::MAX) + Felt::ONE), Err(E::FeeAboveCap));
}

#[test]
fn calldata_the_wallet_cannot_read_exactly_is_refused() {
    let honest = route_call().calldata;
    let mut two = Route::honest();
    two.routes = vec![direct_route(&[]), direct_route(&[])];
    let mut short_span = two.calldata();
    short_span.truncate(short_span.len() - 3);
    let mut trailing = honest.clone();
    trailing.push(Felt::ONE);
    let mut wide = honest.clone();
    wide[1] = Felt::from(u128::MAX) + Felt::ONE;
    for calldata in [vec![], honest[..8].to_vec(), trailing, short_span, wide] {
        let mut bad = route_call();
        bad.calldata = calldata;
        assert_eq!(judge(&[approve_router(), bad]), Err(E::UndecodableRoute));
    }
}

#[test]
fn a_branch_route_is_walked() {
    let mut branch = vec![felt(ETH), felt(STRK), Felt::from(0x6272616e6368u64)];
    branch.extend_from_slice(&direct_route(&[Felt::from(0x1f4u16), Felt::ZERO])[2..]);
    branch.push(Felt::ONE);
    branch.extend_from_slice(&direct_route(&[Felt::from(7u8)])[2..]);
    let route = with(|r| r.routes = vec![branch]);
    assert!(judge(&[approve_router(), route]).is_ok());
}

#[test]
fn the_floor_is_the_quote_less_the_slippage_and_a_basis_point() {
    let quoted = U256::from_u128(BUY_AMOUNT);
    let floor = |slippage| reviewed_min_buy_amount(quoted, slippage, 1);
    assert_eq!(
        floor(0.005),
        Some(U256::from_u128(BUY_AMOUNT * 9_949 / 10_000))
    );
    assert!(floor(0.005).unwrap() < U256::from_u128(MIN_BUY_AMOUNT));
    // A fractional basis point rounds toward more slippage.
    assert_eq!(
        floor(0.00501),
        Some(U256::from_u128(BUY_AMOUNT * 9_948 / 10_000))
    );
    for slippage in [0.0, -0.01, f64::NAN, 1.0, f64::INFINITY] {
        assert_eq!(floor(slippage), None);
    }
    assert_eq!(reviewed_min_buy_amount(U256::ZERO, 0.005, 1), None);
    // A tolerance that reaches the whole amount leaves no floor.
    assert_eq!(reviewed_min_buy_amount(quoted, 0.005, 9_950), None);
    assert!(reviewed_min_buy_amount(quoted, 0.005, 9_949).is_some());
}

#[test]
fn the_floor_is_exact_for_the_largest_quote() {
    let max = U256 {
        high: u128::MAX,
        low: u128::MAX,
    };
    // floor((2^256 - 1) * 9_949 / 10_000), computed independently over the decimal digits.
    let expected = "115201549582205882826910772985143599523218307743845797162856350329472772678771";
    assert_eq!(
        reviewed_min_buy_amount(max, 0.005, 1).unwrap().to_decimal(),
        expected
    );
}
