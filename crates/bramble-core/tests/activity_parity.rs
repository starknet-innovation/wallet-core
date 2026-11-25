//! The fee and the value at submission an expanded Activity row shows (P2) against what the
//! extension's own code produced (`fixtures/extension-activity-detail.jsonl`, `bun run
//! mobile:parity`): the fee read from a receipt reply, which rows take one after a status read,
//! which stored rows validate, what the send path prices, and the text the row shows.

mod portfolio_support;

use portfolio_support::Memory;
use starknet_wallet_core::activity::detail::{fee_text, submitted_price, submitted_value_text};
use starknet_wallet_core::activity::store::Entry;
use starknet_wallet_core::activity::ActivityLog;
use starknet_wallet_core::felt;
use starknet_wallet_core::history::activity::{
    entry_from_json, entry_to_json, FeeUnit, SubmittedPrice, TransactionFee,
};
use starknet_wallet_core::js::json::{parse, JsValue};
use starknet_wallet_core::portfolio::model::DisplayCurrency;
use starknet_wallet_core::portfolio::prices::{PricePoint, PriceView};
use starknet_wallet_core::send::judge::{self, Observation, TxStatus};

fn cases(block: &str) -> Vec<JsValue> {
    let found: Vec<JsValue> = include_str!("fixtures/extension-activity-detail.jsonl")
        .lines()
        .map(|line| parse(line).expect("a JSON line"))
        .filter(|case| case.get("block").and_then(JsValue::as_str) == Some(block))
        .collect();
    assert!(!found.is_empty(), "no {block} cases");
    found
}

fn name(case: &JsValue) -> &str {
    case.get("name").and_then(JsValue::as_str).unwrap()
}

fn json(value: &JsValue) -> serde_json::Value {
    serde_json::from_str(&value.to_json()).unwrap()
}

fn is_null(value: Option<&JsValue>) -> bool {
    matches!(value, None | Some(JsValue::Null))
}

fn expected_fee(value: Option<&JsValue>) -> Option<TransactionFee> {
    if is_null(value) {
        return None;
    }
    let value = value.unwrap();
    Some(TransactionFee {
        amount: value.get("amount")?.as_str()?.into(),
        unit: match value.get("unit")?.as_str()? {
            "FRI" => FeeUnit::Fri,
            _ => FeeUnit::Wei,
        },
    })
}

fn receipt_fee(reply: &JsValue) -> Option<TransactionFee> {
    let mut body = json(reply);
    body["jsonrpc"] = "2.0".into();
    body["id"] = 1.into();
    judge::receipt_fee(judge::answer(&body.to_string(), 1).as_ref())
}

fn status(text: &str) -> TxStatus {
    starknet_wallet_core::activity::store::parse_status(text).unwrap()
}

#[test]
fn receipt_fees_match_read_receipt_fee() {
    for case in cases("receiptFee") {
        let reply = case.get("reply").unwrap();
        assert_eq!(
            receipt_fee(reply),
            expected_fee(case.get("feePaid")),
            "{}",
            name(&case)
        );
    }
}

#[test]
fn a_fee_is_read_only_where_reconciliation_reads_one() {
    for case in cases("reconcile") {
        let storage = Memory::default();
        let mut log = ActivityLog::new(&storage);
        let row: Entry = serde_json::from_value(json(case.get("row").unwrap())).unwrap();
        let (scope, hash) = (row.account_id.clone(), felt::parse(&row.hash).unwrap());
        assert!(log.record(row), "{}", name(&case));
        let observed = status(case.get("observed").and_then(JsValue::as_str).unwrap());
        let found = Observation::Found {
            status: observed,
            accepted_finality: observed == TxStatus::Accepted,
            finalized_on_l1: false,
        };
        assert_eq!(log.observe(&scope, &hash, found), Some(observed));
        let read = case.get("receiptRead") == Some(&JsValue::Bool(true));
        assert_eq!(log.wants_fee(&scope, &hash), read, "{}", name(&case));
        if read {
            if let Some(fee) = receipt_fee(case.get("reply").unwrap()) {
                assert!(log.record_fee(&scope, &hash, &fee));
            }
        }
        let after = log.scope(&scope).remove(0);
        assert_eq!(
            after.fee_paid,
            expected_fee(case.get("feePaid")),
            "{}",
            name(&case)
        );
        let text = case.get("status").and_then(JsValue::as_str).unwrap();
        assert_eq!(after.status, status(text), "{}", name(&case));
    }
}

#[test]
fn stored_rows_validate_as_is_activity_entry_does() {
    for case in cases("record") {
        let entry = case.get("entry").unwrap();
        let valid = case.get("valid") == Some(&JsValue::Bool(true));
        let parsed = entry_from_json(entry);
        assert_eq!(parsed.is_some(), valid, "{}", name(&case));
        if let Some(parsed) = parsed {
            // What the core writes back reads as the same row.
            let again = parse(&entry_to_json(&parsed).to_string()).unwrap();
            assert_eq!(entry_from_json(&again), Some(parsed), "{}", name(&case));
        }
    }
}

fn currency(text: &str) -> DisplayCurrency {
    DisplayCurrency::from_code(text).unwrap()
}

#[test]
fn a_send_is_priced_as_with_submitted_price_prices_it() {
    for case in cases("capture") {
        let answer = case.get("answer").unwrap();
        let view = PriceView {
            currency: currency(answer.get("currency").and_then(JsValue::as_str).unwrap()),
            prices: answer
                .get("prices")
                .and_then(JsValue::as_object)
                .unwrap()
                .iter()
                .map(|(address, value)| PricePoint {
                    address: address.clone(),
                    value: value.as_f64().unwrap(),
                })
                .collect(),
            changes: Vec::new(),
            as_of_ms: answer.get("asOf").and_then(JsValue::as_f64),
            stale: answer.get("stale") == Some(&JsValue::Bool(true)),
        };
        let mut row = json(case.get("entry").unwrap());
        row["status"] = "pending".into();
        row["submittedAt"] = 0.into();
        let row: Entry = serde_json::from_value(row).unwrap();
        let expected = case
            .get("submittedPrice")
            .filter(|v| !is_null(Some(v)))
            .map(|v| SubmittedPrice {
                currency: currency(v.get("currency").and_then(JsValue::as_str).unwrap()),
                priced_at: v.get("pricedAt").and_then(JsValue::as_f64).unwrap() as u64,
                unit_price: v.get("unitPrice").and_then(JsValue::as_f64).unwrap(),
            });
        assert_eq!(submitted_price(&row, &view), expected, "{}", name(&case));
        if let Some(request) = case.get("request").filter(|v| !is_null(Some(v))) {
            // The price is read from the cache only, for the row's token on its network.
            assert_eq!(request.get("cached"), Some(&JsValue::Bool(true)));
            let addresses = request
                .get("addresses")
                .and_then(JsValue::as_array)
                .unwrap();
            assert_eq!(addresses[0].as_str(), row.token_address.as_deref());
        }
    }
}

#[test]
fn the_expanded_row_shows_the_extension_text() {
    for case in cases("fact") {
        let Some(entry) = entry_from_json(case.get("entry").unwrap()) else {
            // A row no store would hold (an empty amount) shows nothing either way.
            assert!(
                is_null(case.get("value")) && is_null(case.get("fee")),
                "{}",
                name(&case)
            );
            continue;
        };
        let text = |key: &str| case.get(key).and_then(JsValue::as_str).map(str::to_string);
        assert_eq!(fee_text(&entry, |_| None), text("fee"), "{}", name(&case));
        assert_eq!(
            submitted_value_text(&entry),
            text("value"),
            "{}",
            name(&case)
        );
    }
}
