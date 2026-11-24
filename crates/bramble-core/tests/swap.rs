//! The AVNU swap through mobile's exports, end to end against canned AVNU replies and a canned
//! node: the quote and its value check, the build against the router `SWAP_POLICY` pins for
//! Sepolia, the plan with its review and fee, the approval filed in the allowance log before the
//! row, and the refusals. The mechanism is wallet-core's `swap` (its unit tests carry the
//! extension's hostile builds, one per guard).
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

mod send_support;

use std::path::Path;
use std::sync::Arc;

use bramble_core::{
    CallKind, DisplayCurrency, FeeAffordability, HttpMethod, HttpOutcome, NetworkId, PricePoint,
    PriceView, SendStep, StepReply, SwapError, TokenPreferences, TokenSwap, WalletCore,
};
use send_support::*;
use serde_json::{json, Value};

const ETH: &str = "0x49d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7";
const STRK_ID: &str = "0x4718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d";
/// AVNU's Sepolia exchange, as `SWAP_POLICY` pins it.
const ROUTER: &str = "0x2c56e8b00dbe2a71e57472685378fc8988bba947e9a99b26a00fade2b4fe7c2";
const ATTACKER: &str = "0x6666666666666666666666666666666666666666666666666666666666666";
const EXCHANGE: &str = "0x1114c7103e12c2b2ecbd3a2472ba9c48ddcbf702b1c242dd570057e26212111";
const QUOTE_ID: &str = "dae4e180-e2cb-49f4-bd91-6f3de3a0c3ca";
/// 0.01 ETH for 12.5 STRK; an honest 0.5% build promises 12.4375.
const SELL: u128 = 10_000_000_000_000_000;
const BUY: u128 = 12_500_000_000_000_000_000;
const MIN_BUY: u128 = 12_437_500_000_000_000_000;
const SEPOLIA_CHAIN: &str = "0x534e5f5345504f4c4941";

fn account() -> String {
    format!("SN_SEPOLIA:{ACCOUNT_0}")
}

fn prefs() -> Arc<TokenPreferences> {
    TokenPreferences::new(None)
}

/// The tracked token's id and stored address on `network`.
fn token(network: NetworkId, symbol: &str) -> (String, String) {
    let tracked = prefs().tracked(network);
    let token = &tracked
        .iter()
        .find(|tracked| tracked.token.symbol == symbol)
        .unwrap()
        .token;
    (token.id.clone(), token.address.clone())
}

fn hex(value: u128) -> String {
    format!("{value:#x}")
}

fn ok(body: &str) -> HttpOutcome {
    HttpOutcome {
        status: 200,
        retry_after: None,
        body: Some(body.as_bytes().to_vec()),
        timed_out: false,
    }
}

fn quote_reply() -> String {
    json!([{"buyAmount": hex(BUY), "buyAmountInUsd": 18.8, "buyTokenAddress": STRK_ID,
        "gasFeesInUsd": 0.025, "quoteId": QUOTE_ID, "sellAmount": hex(SELL),
        "sellAmountInUsd": 18.79, "sellTokenAddress": ETH}])
    .to_string()
}

/// `avnuRouteCalldata` for a direct ETH→STRK route paying `beneficiary` at least `min_buy`.
fn route(beneficiary: &str, min_buy: u128) -> Vec<String> {
    let mut calldata = vec![ETH.into(), hex(SELL), "0x0".into(), STRK_ID.into()];
    calldata.extend([hex(BUY), "0x0".into(), hex(min_buy), "0x0".into()]);
    calldata.extend([beneficiary.into(), "0x0".into(), "0x0".into(), "0x1".into()]);
    calldata
        .extend([ETH, STRK_ID, EXCHANGE, "0x2540be400", "0x2", "0x1f4", "0x0"].map(String::from));
    calldata
}

fn call(contract: &str, entrypoint: &str, calldata: Vec<String>) -> Value {
    json!({"contractAddress": contract, "entrypoint": entrypoint, "calldata": calldata})
}

fn approve(spender: &str) -> Value {
    call(
        ETH,
        "approve",
        vec![spender.into(), hex(SELL), "0x0".into()],
    )
}

fn build_reply(chain: &str, calls: &[Value]) -> String {
    json!({"chainId": chain, "calls": calls}).to_string()
}

fn honest_build() -> String {
    build_reply(
        SEPOLIA_CHAIN,
        &[
            approve(ROUTER),
            call(ROUTER, "multi_route_swap", route(ACCOUNT_0, MIN_BUY)),
        ],
    )
}

fn start(core: &Arc<WalletCore>) -> Arc<TokenSwap> {
    let (eth, strk) = (
        token(NetworkId::Sepolia, "ETH").0,
        token(NetworkId::Sepolia, "STRK").0,
    );
    core.clone()
        .prepare_swap(account(), eth, strk, " 0.01 ".into(), prefs())
        .unwrap()
}

/// A swap quoted and built from `build`, as far as its review.
fn built(core: &Arc<WalletCore>, build: &str) -> Result<Arc<TokenSwap>, SwapError> {
    let swap = start(core);
    swap.accept_quote(ok(&quote_reply()), None).unwrap();
    swap.build_request(None).unwrap();
    swap.accept_build(ok(build)).map(|_| swap)
}

fn refused(reason: &str) -> SwapError {
    SwapError::Refused {
        reason: reason.into(),
    }
}

fn allowance_log(dir: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(dir.join("allowances.v1.json")).unwrap_or("[]".into());
    serde_json::from_str::<Value>(&text)
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn a_swap_is_quoted_built_against_the_pinned_router_and_planned_with_its_fee() {
    let (_dir, core) = wallet();
    let swap = start(&core);
    let quote = swap.quote_request();
    assert_eq!(quote.method, HttpMethod::Get);
    assert_eq!(
        quote.url,
        format!(
            "https://sepolia.api.avnu.fi/swap/v3/quotes?buyTokenAddress={}&sellAmount={}\
             &sellTokenAddress={}&size=1&takerAddress={ACCOUNT_0}",
            token(NetworkId::Sepolia, "STRK").1,
            hex(SELL),
            token(NetworkId::Sepolia, "ETH").1,
        )
    );
    // The wallet's own feed prices the quote: 12.5 STRK at 1.5 against 0.01 ETH at 2,000.
    let view = PriceView {
        currency: DisplayCurrency::Usd,
        prices: [("ETH", 2_000.0), ("STRK", 1.5)]
            .map(|(symbol, value)| PricePoint {
                address: token(NetworkId::Sepolia, symbol).1,
                value,
            })
            .to_vec(),
        changes: Vec::new(),
        as_of_ms: None,
        stale: false,
    };
    let quoted = swap.accept_quote(ok(&quote_reply()), Some(view)).unwrap();
    assert_eq!(
        (quoted.sell_amount.as_str(), quoted.buy_amount.as_str()),
        ("0.01", "12.5")
    );
    assert_eq!(quoted.buy_amount_in_usd, 18.8);
    let delta = quoted.value_delta_percent.unwrap();
    assert!((delta + 6.25).abs() < 1e-9, "{delta}");
    // A slippage past the policy is refused and keeps the quote.
    assert_eq!(
        swap.build_request(Some(0.2)).err(),
        Some(refused("invalid swap preparation request"))
    );
    let build = swap.build_request(None).unwrap();
    assert_eq!(build.url, "https://sepolia.api.avnu.fi/swap/v3/build");
    assert_eq!(
        build.body.as_deref(),
        Some(&*format!(
            r#"{{"includeApprove":true,"quoteId":"{QUOTE_ID}","slippage":0.005,"takerAddress":"{ACCOUNT_0}"}}"#
        ))
    );
    let review = swap.accept_build(ok(&honest_build())).unwrap();
    assert_eq!(review.min_buy_amount, "12.4375");
    assert_eq!(
        review.summaries[1],
        "Swap 0.01 ETH for at least 12.4375 STRK, paid to 0x2083…98d9"
    );
    assert!(review.flags.is_empty(), "{:?}", review.flags);

    let plan = swap.plan(Some("100000000000000000000".into())).unwrap();
    let mut node = sepolia(true);
    let first = plan.advance(StepReply::Start).unwrap();
    assert_eq!(
        run_until(&plan, &core, &mut node, first, never),
        SendStep::Review
    );
    let card = plan.current_review().unwrap();
    assert!(card.can_confirm);
    assert_eq!(card.affordability, FeeAffordability::Sufficient);
    let calls = plan.reviewed_calls();
    assert_eq!(
        calls
            .iter()
            .map(|c| (c.contract_address.as_str(), c.entrypoint.as_str()))
            .collect::<Vec<_>>(),
        [(ETH, "approve"), (ROUTER, "multi_route_swap")]
    );
    assert_eq!(calls[0].kind, CallKind::Approve);
    assert!(plan.risk_flags().is_empty(), "{:?}", plan.risk_flags());
    // One plan per preparation.
    assert_eq!(swap.plan(None).err(), Some(SwapError::NotFound));
}

// The approval is filed before the write leaves, and without the row: here the activity file
// cannot be written at all. A normal run files both, the grant first.
#[test]
fn a_swaps_approval_is_filed_before_its_row_and_without_it() {
    for blocked in [false, true] {
        let (dir, core) = wallet();
        if blocked {
            std::fs::create_dir(dir.path().join("activity.v1.json.tmp")).unwrap();
        }
        let plan = built(&core, &honest_build()).unwrap().plan(None).unwrap();
        let mut node = sepolia(true);
        let first = plan.advance(StepReply::Start).unwrap();
        assert_eq!(
            run_until(&plan, &core, &mut node, first, never),
            SendStep::Review
        );
        let step = plan.advance(StepReply::Confirm).unwrap();
        let write = run_until(&plan, &core, &mut node, step, |_, is_write| is_write);
        assert!(rpc_parts(&write).3, "{write:?}");
        let filed = allowance_log(dir.path());
        assert_eq!(filed.len(), 1, "filed before the write left");
        assert_eq!(
            (
                &filed[0]["token"],
                &filed[0]["spender"],
                &filed[0]["amount"]
            ),
            (&json!(ETH), &json!(ROUTER), &json!(SELL.to_string()))
        );
        let rows = core.activity(account()).unwrap();
        assert_eq!(rows.len(), usize::from(!blocked));
        if let Some(row) = rows.first() {
            assert_eq!(row.summary, "Swap 0.01 ETH for ~12.5 STRK via avnu");
        }
    }
}

#[test]
fn a_build_off_the_pin_or_under_the_floor_is_refused() {
    let (_dir, core) = wallet();
    let swap_call = |contract: &str, beneficiary: &str, min_buy: u128| {
        call(contract, "multi_route_swap", route(beneficiary, min_buy))
    };
    let cases = [
        (
            build_reply(
                SEPOLIA_CHAIN,
                &[approve(ATTACKER), swap_call(ROUTER, ACCOUNT_0, MIN_BUY)],
            ),
            "swap build approved an unexpected spender",
        ),
        (
            build_reply(
                SEPOLIA_CHAIN,
                &[approve(ROUTER), swap_call(ATTACKER, ACCOUNT_0, MIN_BUY)],
            ),
            "swap build called a contract that is not the avnu router",
        ),
        (
            build_reply(
                SEPOLIA_CHAIN,
                &[approve(ROUTER), swap_call(ROUTER, ACCOUNT_0, MIN_BUY / 2)],
            ),
            "swap build accepted less than the minimum you reviewed",
        ),
        (
            build_reply(
                SEPOLIA_CHAIN,
                &[approve(ROUTER), swap_call(ROUTER, ACCOUNT_1, MIN_BUY)],
            ),
            "swap build routed the proceeds to another account",
        ),
        // A build for another network.
        (
            build_reply(
                "0x534e5f4d41494e",
                &[approve(ROUTER), swap_call(ROUTER, ACCOUNT_0, MIN_BUY)],
            ),
            "invalid swap build response",
        ),
    ];
    for (build, reason) in cases {
        assert_eq!(
            built(&core, &build).err(),
            Some(refused(reason)),
            "{reason}"
        );
    }
}

// An expired quote (AVNU refuses its build), a step out of turn, and a refusal all end the swap:
// a new price is a new `TokenSwap`.
#[test]
fn a_stale_quote_or_a_step_out_of_turn_ends_the_swap() {
    let (_dir, core) = wallet();
    let swap = start(&core);
    assert_eq!(swap.build_request(None).err(), Some(SwapError::NotFound));
    assert_eq!(swap.plan(None).err(), Some(SwapError::NotFound));
    swap.accept_quote(ok(&quote_reply()), None).unwrap();
    assert_eq!(
        swap.accept_quote(ok(&quote_reply()), None).err(),
        Some(SwapError::NotFound)
    );
    swap.build_request(None).unwrap();
    let expired = HttpOutcome {
        status: 400,
        ..ok(r#"{"messages":["Quote not found"]}"#)
    };
    assert_eq!(
        swap.accept_build(expired).err(),
        Some(refused("swap build failed"))
    );
    assert_eq!(
        swap.accept_build(ok(&honest_build())).err(),
        Some(SwapError::NotFound)
    );
    assert_eq!(swap.plan(None).err(), Some(SwapError::NotFound));
    // A quote for another pair or amount.
    let other = start(&core);
    let wrong = quote_reply().replace(&hex(SELL), &hex(SELL * 2));
    assert_eq!(
        other.accept_quote(ok(&wrong), None).err(),
        Some(refused("swap quote does not match the requested pair"))
    );
    assert_eq!(
        other.accept_quote(ok(&quote_reply()), None).err(),
        Some(SwapError::NotFound)
    );
}

#[test]
fn a_swap_takes_only_tokens_tracked_on_the_accounts_network() {
    let (_dir, core) = wallet();
    let prepare = |sell: String, buy: String, amount: &str| {
        core.clone()
            .prepare_swap(account(), sell, buy, amount.into(), prefs())
            .err()
    };
    let (eth, strk) = (
        token(NetworkId::Sepolia, "ETH").0,
        token(NetworkId::Sepolia, "STRK").0,
    );
    let mainnet_eth = token(NetworkId::Mainnet, "ETH").0;
    assert_ne!(mainnet_eth, eth);
    let unavailable = Some(refused("token unavailable on this network"));
    assert_eq!(prepare(mainnet_eth, strk.clone(), "1"), unavailable);
    assert_eq!(prepare(ETH.into(), strk.clone(), "1"), unavailable);
    assert_eq!(
        prepare(eth.clone(), eth.clone(), "1"),
        Some(refused("pick two different tokens"))
    );
    assert_eq!(
        prepare(eth.clone(), strk.clone(), "0"),
        Some(refused("enter an amount to swap"))
    );
    let unknown = core
        .clone()
        .prepare_swap("SN_SEPOLIA:0x1".into(), eth, strk, "1".into(), prefs())
        .err();
    assert_eq!(unknown, Some(SwapError::UnknownAccount));
}

// A plan holds the session it was prepared in: a lock and a fresh unlock void the swap.
#[test]
fn a_lock_voids_a_prepared_swap() {
    let (dir, core) = wallet();
    let swap = built(&core, &honest_build()).unwrap();
    core.lock();
    core.unlock(PASSWORD.into()).unwrap();
    let plan = swap.plan(None).unwrap();
    let mut node = sepolia(true);
    let first = plan.advance(StepReply::Start).unwrap();
    assert_eq!(
        run_until(&plan, &core, &mut node, first, never),
        SendStep::Review
    );
    let step = plan.advance(StepReply::Confirm).unwrap();
    let failure = failure(run_until(&plan, &core, &mut node, step, never));
    assert_eq!(failure.code, bramble_core::SendFailureCode::Locked);
    assert_eq!(plan.signatures_made(), 0);
    assert!(allowance_log(dir.path()).is_empty());
}
