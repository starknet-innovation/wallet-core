//! The round's own rules: the order of its exchanges, the repeats, the cap, and that a failed
//! exchange hides every offer. G7h's `offers` block is read in `bramble-core`'s
//! `gasless_parity.rs`.

use super::*;
use crate::accounts::AccountFamily;
use crate::gasless::tests::{call, felt_of, v2_classes, POLICY};
use crate::gasless::typed_data::tests::{standard, NOW, RECIPIENT, USDC};
use crate::gasless::{GaslessAccount, GaslessPolicy};
use crate::NetworkId;
use serde_json::{json, Value};

const ACCOUNT: &str = "0x5dccf9cfdcabb9e4ef45814bda1b056bb23063d0e9b64e5a845e9dfb3d23f10";
const ARGENT_04: &str = "0x36078334509b514626504edc9fb252328d1a240e4e948bef8d0c08dff45927f";
const STRK: &str = "0x4718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d";
const OTHER: &str = "0x1111";

fn session(policy: &GaslessPolicy) -> GaslessSession<'_> {
    let account = GaslessAccount {
        network: NetworkId::Mainnet,
        address: felt_of(ACCOUNT),
        family: AccountFamily::Argent,
        class_hash: felt_of(ARGENT_04),
        deployed: true,
        deployment: None,
    };
    let calls = [call(USDC, "transfer", &[RECIPIENT, "0x10", "0x0"])];
    GaslessSession::open(policy, &account, &calls, &v2_classes()).unwrap()
}

fn held(address: &str) -> (GaslessHeldToken, Option<U256>) {
    let token = GaslessHeldToken {
        address: felt_of(address),
        balance: Some(U256::from_u128(5000)),
        decimals: 6,
        label: "USDC".into(),
    };
    (token, Some(U256::from_u128(1000)))
}

fn reply(result: Value) -> HttpOutcome {
    let body = json!({"id": 1, "jsonrpc": "2.0", "result": result}).to_string();
    HttpOutcome {
        status: 200,
        retry_after: None,
        body: Some(body.into_bytes()),
        timed_out: false,
    }
}

fn unavailable() -> HttpOutcome {
    HttpOutcome {
        status: 503,
        retry_after: None,
        body: Some(br#"{"error":"screening_unavailable"}"#.to_vec()),
        timed_out: false,
    }
}

fn listed(addresses: &[&str]) -> HttpOutcome {
    let rows: Vec<Value> = addresses
        .iter()
        .map(|address| json!({"decimals": 6, "token_address": address}))
        .collect();
    reply(Value::Array(rows))
}

fn built() -> HttpOutcome {
    reply(json!({
        "fee": {
            "estimated_fee_in_gas_token": "0x1",
            "estimated_fee_in_strk": "0x1",
            "gas_token_price_in_strk": "0x1",
            "suggested_max_fee_in_gas_token": "0x3e8",
            "suggested_max_fee_in_strk": "0x1",
        },
        "parameters": {},
        "type": "invoke",
        "typed_data": standard(),
    }))
}

fn method(step: &RoundStep) -> String {
    let RoundStep::Post { request, .. } = step else {
        panic!("not a post: {step:?}");
    };
    let body: Value = serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
    body["method"].as_str().unwrap().into()
}

fn offers(step: RoundStep) -> Vec<String> {
    let RoundStep::Done(offers) = step else {
        panic!("not done: {step:?}");
    };
    offers.into_iter().map(|offer| offer.amount).collect()
}

fn start<'p>(policy: &'p GaslessPolicy, tokens: &[&str]) -> (OfferRound<'p>, RoundStep) {
    let held = tokens.iter().map(|address| held(address)).collect();
    OfferRound::start(session(policy), held, felt_of(STRK), NOW)
}

#[test]
fn asks_availability_then_the_list_then_one_build_per_held_token() {
    let (mut round, step) = start(&POLICY, &[USDC]);
    assert_eq!(method(&step), "paymaster_isAvailable");
    let step = round.accept(&reply(json!(true)));
    assert_eq!(method(&step), "paymaster_getSupportedTokens");
    // STRK and a token not held get no build.
    let step = round.accept(&listed(&[STRK, OTHER, USDC]));
    assert_eq!(method(&step), "paymaster_buildTransaction");
    assert_eq!(offers(round.accept(&built())), ["1000"]);
    // The round is over; it answers the same again.
    assert_eq!(offers(round.accept(&built())), ["1000"]);
}

#[test]
fn only_true_is_available() {
    for answer in [json!(false), json!("true"), json!(1)] {
        let (mut round, _) = start(&POLICY, &[USDC]);
        assert!(offers(round.accept(&reply(answer))).is_empty());
    }
}

#[test]
fn an_unreadable_token_list_ends_the_round() {
    let (mut round, _) = start(&POLICY, &[USDC]);
    round.accept(&reply(json!(true)));
    assert!(offers(round.accept(&reply(json!({"tokens": []})))).is_empty());
}

#[test]
fn screening_unavailable_is_repeated_after_growing_pauses_then_ends_the_round() {
    let (mut round, _) = start(&POLICY, &[USDC]);
    for delay in [250, 500] {
        let step = round.accept(&unavailable());
        assert_eq!(method(&step), "paymaster_isAvailable");
        assert!(matches!(step, RoundStep::Post { delay_ms, .. } if delay_ms == delay));
    }
    assert!(offers(round.accept(&unavailable())).is_empty());
}

#[test]
fn a_failed_build_hides_the_offers_already_made() {
    let (mut round, _) = start(&POLICY, &[USDC, OTHER]);
    round.accept(&reply(json!(true)));
    round.accept(&listed(&[USDC, OTHER]));
    let step = round.accept(&built());
    assert_eq!(method(&step), "paymaster_buildTransaction");
    let refused = HttpOutcome {
        status: 403,
        retry_after: None,
        body: Some(br#"{"error":"screening_denied"}"#.to_vec()),
        timed_out: false,
    };
    assert!(offers(round.accept(&refused)).is_empty());
}

#[test]
fn a_quote_the_wallet_would_not_sign_skips_only_that_token() {
    let (mut round, _) = start(&POLICY, &[USDC]);
    round.accept(&reply(json!(true)));
    round.accept(&listed(&[USDC]));
    assert!(offers(round.accept(&reply(json!({})))).is_empty());
}

#[test]
fn a_token_without_a_reference_gets_no_build() {
    let mut unpriced = held(USDC);
    unpriced.1 = None;
    let (mut round, _) = OfferRound::start(session(&POLICY), vec![unpriced], felt_of(STRK), NOW);
    round.accept(&reply(json!(true)));
    assert!(offers(round.accept(&listed(&[USDC]))).is_empty());
}

#[test]
fn stops_building_at_the_policys_cap() {
    let one = GaslessPolicy {
        max_offers: 1,
        ..POLICY
    };
    let (mut round, _) = start(&one, &[USDC, OTHER]);
    round.accept(&reply(json!(true)));
    round.accept(&listed(&[USDC, OTHER]));
    assert_eq!(offers(round.accept(&built())), ["1000"]);
}
