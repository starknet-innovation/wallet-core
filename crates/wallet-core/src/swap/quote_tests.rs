//! The quote against the extension's own cases (`avnu.test.ts`, `swap-ops.test.ts`,
//! `swap-value-check.test.ts`), one guard per case.

use super::super::fixtures::*;
use super::*;
use crate::portfolio::model::DisplayCurrency;
use crate::portfolio::prices::PricePoint;

fn new(sell: &TokenInfo, buy: &TokenInfo, amount: &str) -> Result<HttpRequest, SwapRefusal> {
    PendingQuote::new(&account(), sell, buy, amount, &POLICY).map(|(_, request)| request)
}

fn judged(body: &str) -> Result<SwapQuote, SwapRefusal> {
    pending_quote().judge(&outcome(200, body))
}

fn reply_with(key: &str, value: serde_json::Value) -> String {
    let mut reply = quote_json();
    reply[key] = value;
    serde_json::json!([reply]).to_string()
}

#[test]
fn the_request_is_the_extensions_query_on_the_pinned_api() {
    let [eth, strk] = tokens();
    let request = new(&eth, &strk, " 0.01 ").unwrap();
    assert_eq!(request.method, HttpMethod::Get);
    assert_eq!(
        request.url,
        format!(
            "https://starknet.api.avnu.fi/swap/v3/quotes?buyTokenAddress={STRK}&sellAmount=0x2386f26fc10000&sellTokenAddress={ETH}&size=1&takerAddress={ACCOUNT}"
        )
    );
    assert_eq!(request.body, None);
    assert!(request.headers.is_empty());
    assert_eq!(request.max_body_bytes, 1024 * 1024);
    assert_eq!(request.deadline_ms, 15_000);
}

#[test]
fn the_input_rules_refuse_before_any_request() {
    let [eth, strk] = tokens();
    assert_eq!(new(&eth, &eth, "1"), Err(SwapRefusal::SameToken));
    assert_eq!(new(&eth, &strk, "0"), Err(SwapRefusal::NoAmount));
    assert_eq!(
        new(&eth, &strk, "0.0000000000000000001"),
        Err(SwapRefusal::NoAmount)
    );
    assert_eq!(
        new(&eth, &strk, "1e18"),
        Err(SwapRefusal::InvalidQuoteRequest)
    );
    let unparsed = token("not-a-felt", "BAD");
    assert_eq!(
        new(&unparsed, &strk, "1"),
        Err(SwapRefusal::TokenUnavailable)
    );
    assert_eq!(
        new(&eth, &unparsed, "1"),
        Err(SwapRefusal::TokenUnavailable)
    );
    assert_eq!(
        new(&token("0x0", "ZERO"), &strk, "1"),
        Err(SwapRefusal::TokenUnavailable)
    );
    // A network with no pinned deployment has no swaps, and no request is built.
    let sepolia = Account {
        network: NetworkId::Sepolia,
        ..account()
    };
    assert_eq!(
        PendingQuote::new(&sepolia, &eth, &strk, "1", &POLICY).map(|_| ()),
        Err(SwapRefusal::Unavailable)
    );
}

#[test]
fn the_best_quote_for_the_requested_pair_is_returned() {
    let quote = quote();
    assert_eq!(quote.quote_id(), QUOTE_ID);
    assert_eq!(quote.buy_amount_text(), "12.5");
    assert_eq!(quote.sell_amount_text(), "0.01");
    assert_eq!(quote.buy_amount_in_usd, 18.8);
    assert_eq!(quote.sell_amount_in_usd, 18.79);
    assert_eq!(quote.gas_fees_in_usd, 0.025);
    // Only the first quote is read.
    let body = serde_json::json!([quote_json(), {"junk": true}]).to_string();
    assert_eq!(judged(&body).unwrap().quote_id(), QUOTE_ID);
    // Hex amounts may use either case.
    let upper = reply_with("buyAmount", "0xAD78EBC5AC6200000".into());
    assert_eq!(judged(&upper).unwrap().buy_amount_text(), "200");
}

#[test]
fn a_quote_for_another_pair_or_amount_is_refused() {
    let cases = [
        ("sellTokenAddress", STRK.to_string()),
        ("buyTokenAddress", ETH.to_string()),
        ("sellAmount", "0xffffffffffffffff".to_string()),
    ];
    for (key, value) in cases {
        assert_eq!(
            judged(&reply_with(key, value.into())),
            Err(SwapRefusal::QuoteMismatch),
            "{key}"
        );
    }
}

#[test]
fn every_quote_field_must_be_in_its_grammar() {
    let cases: [(&str, serde_json::Value); 16] = [
        ("quoteId", "".into()),
        ("quoteId", "q".repeat(121).into()),
        ("quoteId", 7.into()),
        ("sellTokenAddress", "not-a-felt".into()),
        (
            "buyTokenAddress",
            ETH.to_uppercase().replace("0X", "0x").into(),
        ),
        ("sellAmount", "12.5".into()),
        ("buyAmount", "12.5".into()),
        ("buyAmount", "0x".into()),
        ("buyAmount", "0x+1".into()),
        ("buyAmount", format!("0x{}1", "0".repeat(64)).into()),
        ("buyAmount", format!("0x{}", "1".repeat(65)).into()),
        ("sellAmountInUsd", (-1.0).into()),
        ("buyAmountInUsd", "18.8".into()),
        ("gasFeesInUsd", serde_json::Value::Null),
        ("gasFeesInUsd", (-0.5).into()),
        ("sellAmountInUsd", serde_json::Value::Null),
    ];
    for (key, value) in cases {
        assert_eq!(
            judged(&reply_with(key, value.clone())),
            Err(SwapRefusal::InvalidQuote),
            "{key} = {value}"
        );
    }
    // A number JSON spells past f64's range reads as Infinity, which is not a figure.
    let infinite = serde_json::json!([quote_json()])
        .to_string()
        .replace("0.025", "1e999");
    assert_eq!(judged(&infinite), Err(SwapRefusal::InvalidQuote));
    // The longest accepted id.
    assert!(judged(&reply_with("quoteId", "q".repeat(120).into())).is_ok());
}

#[test]
fn no_route_and_failed_exchanges_are_refused() {
    assert_eq!(judged("[]"), Err(SwapRefusal::NoRoute));
    assert_eq!(judged("{}"), Err(SwapRefusal::NoRoute));
    assert_eq!(judged("[1]"), Err(SwapRefusal::InvalidQuote));
    let body = serde_json::json!([quote_json()]).to_string();
    let failed = pending_quote().judge(&outcome(500, &body));
    assert_eq!(failed, Err(SwapRefusal::QuoteFailed));
    assert_eq!(judged("not json"), Err(SwapRefusal::QuoteFailed));
    let none = HttpOutcome {
        body: None,
        ..outcome(200, "")
    };
    assert_eq!(pending_quote().judge(&none), Err(SwapRefusal::QuoteFailed));
    // The cap is checked before anything is parsed.
    let padded = format!("{body}{}", " ".repeat(1024 * 1024));
    assert_eq!(judged(&padded), Err(SwapRefusal::QuoteFailed));
    let at_cap = format!("{body}{}", " ".repeat(1024 * 1024 - body.len()));
    assert!(judged(&at_cap).is_ok());
}

fn view(prices: &[(&str, f64)], stale: bool) -> PriceView {
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
        as_of_ms: Some(1.0),
        stale,
    }
}

/// 2 sold for 3 bought, as `swap-value-check.test.ts` sets it up.
fn delta(prices: Option<&PriceView>, sell: &str, buy: u128) -> Option<f64> {
    let [a, b] = [token(ETH, "AAA"), token(STRK, "BBB")];
    let (pending, _) = PendingQuote::new(&account(), &a, &b, sell, &POLICY).unwrap();
    let mut reply = quote_json();
    reply["sellAmount"] = hex(crate::send::calls::parse_token_amount(sell, 18)
        .unwrap()
        .low)
    .into();
    reply["buyAmount"] = hex(buy * 10u128.pow(18)).into();
    let body = serde_json::json!([reply]).to_string();
    pending
        .judge(&outcome(200, &body))
        .unwrap()
        .value_delta_percent(prices)
}

#[test]
fn the_value_check_prices_both_sides_from_the_wallets_feed() {
    let feed = view(&[(ETH, 10.0), (STRK, 5.0)], false);
    // 2 x $10 sold for 3 x $5: a 25% loss.
    assert!((delta(Some(&feed), "2", 3).unwrap() + 25.0).abs() < 1e-9);
    // A zero buy amount is a total loss.
    assert_eq!(delta(Some(&feed), "2", 0), Some(-100.0));
    // The aggregator says ~$18.8 each side; the feed says 0.01 ETH is $40 and 12.5 STRK $10.
    let market = view(&[(ETH, 4_000.0), (STRK, 0.8)], false);
    assert!((quote().value_delta_percent(Some(&market)).unwrap() + 75.0).abs() < 1e-9);
}

#[test]
fn the_value_check_falls_back_rather_than_guessing() {
    assert_eq!(delta(None, "2", 3), None);
    assert_eq!(delta(Some(&view(&[(ETH, 10.0)], false)), "2", 3), None);
    assert_eq!(delta(Some(&view(&[(STRK, 5.0)], false)), "2", 3), None);
    let zero = view(&[(ETH, 10.0), (STRK, 0.0)], false);
    assert_eq!(delta(Some(&zero), "2", 3), None);
    let infinite = view(&[(ETH, f64::INFINITY), (STRK, 5.0)], false);
    assert_eq!(delta(Some(&infinite), "2", 3), None);
    // A stale cache can show a loss as break-even.
    let stale = view(&[(ETH, 10.0), (STRK, 5.0)], true);
    assert_eq!(delta(Some(&stale), "2", 3), None);
}
