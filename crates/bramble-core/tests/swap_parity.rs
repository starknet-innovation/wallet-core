//! AVNU swaps (P11) against what the extension's real handler did (`fixtures/extension-swap.jsonl`,
//! `bun run mobile:parity`): each quote's GET and its judgement or refusal, the value check from
//! the wallet's own feed, each build's POST and the review it prepared (lines, flags, minimum, the
//! calls that reached the signer, the activity row) or the refusal, and the reviewed floor.

mod multisig_support;

use bramble_core::swap::SWAP_POLICY;
use multisig_support::TestPrimitives;
use serde_json::Value;
use starknet_wallet_core::catalog::TokenInfo;
use starknet_wallet_core::portfolio::model::DisplayCurrency;
use starknet_wallet_core::portfolio::prices::{PricePoint, PriceView};
use starknet_wallet_core::portfolio::wire::{HttpMethod, HttpOutcome, HttpRequest};
use starknet_wallet_core::send::call_decode::Known;
use starknet_wallet_core::send::review_flags::ReviewContext;
use starknet_wallet_core::send::types::{Account, Authorization};
use starknet_wallet_core::send::u256::U256;
use starknet_wallet_core::swap::{reviewed_min_buy_amount, PendingQuote, SwapQuote};
use starknet_wallet_core::{felt, Felt, NetworkId};

/// The swap harness's unlocked account (`tests/support/swap-policy-harness.ts`).
const ACCOUNT: &str = "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcde";

fn cases(block: &str) -> Vec<Value> {
    let found: Vec<Value> = include_str!("fixtures/extension-swap.jsonl")
        .lines()
        .map(|line| serde_json::from_str(line).expect("a JSON line"))
        .filter(|case: &Value| case["block"] == block)
        .collect();
    assert!(!found.is_empty(), "no {block} cases");
    found
}

fn account() -> Account {
    Account {
        network: NetworkId::Mainnet,
        address: Felt::from_hex(ACCOUNT).unwrap(),
        public_key: Felt::from(3u8),
        authorization: Authorization::StarkPair,
        deployment: None,
    }
}

/// The harness wallet tracks Bramble's default mainnet tokens; their records are the registry's.
fn token(symbol: &str) -> TokenInfo {
    let found = bramble_core::catalogs::networks::registry()
        .tokens
        .iter()
        .find(|token| token.network_id == "SN_MAIN" && token.symbol == symbol)
        .unwrap();
    TokenInfo {
        address: found.address.clone(),
        decimals: found.decimals,
        name: found.name.clone(),
        symbol: found.symbol.clone(),
    }
}

fn tracked() -> Vec<TokenInfo> {
    ["ETH", "STRK", "USDC"].map(token).to_vec()
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap()
}

fn outcome(reply: &Value) -> HttpOutcome {
    HttpOutcome {
        status: u16::try_from(reply["status"].as_u64().unwrap()).unwrap(),
        retry_after: None,
        body: Some(text(&reply["body"]).as_bytes().to_vec()),
        timed_out: false,
    }
}

fn assert_request(request: &HttpRequest, sent: &Value, name: &str) {
    let method = match request.method {
        HttpMethod::Get => "GET",
        HttpMethod::Post => "POST",
    };
    assert_eq!(method, sent["method"], "{name}: method");
    assert_eq!(request.url, sent["url"], "{name}: url");
    assert_eq!(
        request.body.as_deref(),
        sent["body"].as_str(),
        "{name}: body"
    );
    let content_type = request
        .headers
        .iter()
        .find(|header| header.name.eq_ignore_ascii_case("content-type"))
        .map(|header| header.value.as_str());
    assert_eq!(
        content_type,
        sent["contentType"].as_str(),
        "{name}: content type"
    );
}

/// The made-up feed, keyed by each token's tracked address as the extension's is.
fn feed(value: &Value) -> Option<PriceView> {
    let prices = value["feed"]["prices"].as_object()?;
    Some(PriceView {
        currency: DisplayCurrency::Usd,
        prices: prices
            .iter()
            .map(|(symbol, price)| PricePoint {
                address: token(symbol).address,
                value: price.as_f64().unwrap(),
            })
            .collect(),
        changes: Vec::new(),
        as_of_ms: Some(0.0),
        stale: value["feed"]["stale"] == true,
    })
}

/// The quote request for `case`'s ask, or the refusal before any request.
fn ask(case: &Value) -> Result<(PendingQuote, HttpRequest), String> {
    let ask = &case["ask"];
    PendingQuote::new(
        &account(),
        &token(text(&ask["sell"])),
        &token(text(&ask["buy"])),
        text(&ask["amount"]),
        &SWAP_POLICY,
    )
    .map_err(|refusal| refusal.to_string())
}

fn quoted(case: &Value, reply: &Value) -> Result<SwapQuote, String> {
    let name = text(&case["name"]);
    let (pending, request) = ask(case)?;
    assert_request(&request, &case["requests"][0], name);
    pending
        .judge(&outcome(reply))
        .map_err(|refusal| refusal.to_string())
}

#[test]
fn quotes_match_the_handler() {
    for case in cases("quote") {
        let name = text(&case["name"]);
        let expected = &case["response"];
        let result = match case["replies"].get(0) {
            Some(reply) => quoted(&case, reply),
            None => ask(&case).map(|_| unreachable!("{name}: the core asked AVNU")),
        };
        let quote = match result {
            Err(refusal) => {
                assert_eq!(refusal, expected["error"], "{name}");
                continue;
            }
            Ok(quote) => quote,
        };
        let shown = &expected["swapQuote"];
        assert_eq!(quote.buy_amount_text(), shown["buyAmount"], "{name}");
        assert_eq!(quote.sell_amount_text(), shown["sellAmount"], "{name}");
        assert_eq!(quote.quote_id(), shown["quoteId"], "{name}");
        assert_eq!(quote.sell_amount_in_usd, shown["sellAmountInUsd"], "{name}");
        assert_eq!(quote.buy_amount_in_usd, shown["buyAmountInUsd"], "{name}");
        assert_eq!(quote.gas_fees_in_usd, shown["gasFeesInUsd"], "{name}");
        assert_eq!(
            quote.value_delta_percent(feed(&case).as_ref()),
            shown["valueDeltaPercent"].as_f64(),
            "{name}: value check"
        );
    }
}

fn felts(values: &Value) -> Vec<Felt> {
    values
        .as_array()
        .unwrap()
        .iter()
        .map(|value| felt::parse(text(value)).unwrap())
        .collect()
}

#[test]
fn builds_match_the_handler() {
    let tokens = tracked();
    let context = ReviewContext {
        known: Known {
            tokens: &tokens,
            nft_contracts: &[],
        },
        ..ReviewContext::default()
    };
    for case in cases("build") {
        let name = text(&case["name"]);
        let expected = &case["response"];
        let refused = |refusal: String| assert_eq!(refusal, expected["error"], "{name}");
        let quote = quoted(&case, &case["quote"]).unwrap();
        let (pending, request) = match quote.prepare(&account(), case["slippage"].as_f64()) {
            Err(refusal) => {
                refused(refusal.to_string());
                assert_eq!(case["requests"].as_array().unwrap().len(), 1, "{name}");
                continue;
            }
            Ok(prepared) => prepared,
        };
        assert_request(&request, &case["requests"][1], name);
        let judged = pending.judge(
            &outcome(&case["build"]),
            &context,
            &bramble_core::SEND_POLICY,
            &TestPrimitives::new(),
        );
        let preparation = match judged {
            Err(refusal) => {
                refused(refusal.to_string());
                continue;
            }
            Ok(preparation) => preparation,
        };
        let review = &expected["swapPreparation"];
        assert_eq!(
            preparation.summaries(),
            review["summaries"].as_array().unwrap().as_slice(),
            "{name}"
        );
        assert_eq!(
            preparation.min_buy_amount_text(),
            review["minBuyAmount"],
            "{name}"
        );
        let flags: Vec<Value> = preparation
            .flags()
            .iter()
            .map(|flag| serde_json::json!({ "code": flag.code(), "message": flag.message() }))
            .collect();
        assert_eq!(
            flags,
            review["flags"].as_array().unwrap().as_slice(),
            "{name}"
        );
        let invoke = preparation.into_invoke(&account()).unwrap();
        let submitted = case["submitted"][0].as_array().unwrap();
        assert_eq!(invoke.signed_calls().len(), submitted.len(), "{name}");
        for (call, sent) in invoke.signed_calls().iter().zip(submitted) {
            assert_eq!(
                Some(call.contract_address),
                felt::parse(text(&sent["contractAddress"])),
                "{name}"
            );
            assert_eq!(call.entrypoint, sent["entrypoint"], "{name}");
            assert_eq!(call.calldata, felts(&sent["calldata"]), "{name}");
        }
        let row = &case["activity"][0];
        assert_eq!(invoke.activity_summary(), row["summary"], "{name}");
        let asset = invoke.activity_asset().unwrap();
        assert_eq!(asset.amount.as_deref(), row["amount"].as_str(), "{name}");
        assert_eq!(asset.symbol, row["token"], "{name}");
        assert_eq!(
            felt::canonical(&asset.token_address),
            row["tokenAddress"],
            "{name}"
        );
    }
}

#[test]
fn floors_match_reviewed_min_buy_amount() {
    for case in cases("floor") {
        let quoted = U256::from_decimal(text(&case["quoted"])).unwrap();
        let floor = reviewed_min_buy_amount(
            quoted,
            case["slippage"].as_f64().unwrap(),
            SWAP_POLICY.slippage_rounding_tolerance_bps,
        );
        assert_eq!(
            floor.map(U256::to_decimal).as_deref(),
            case["floor"].as_str(),
            "{}",
            text(&case["name"])
        );
    }
}
