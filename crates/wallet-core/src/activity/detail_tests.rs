//! The sponsored and gasless rows (`feePayer`, `userFee`): their grammar, that neither takes a
//! receipt fee, and the fee and summary text they show. The receipt and price rules are
//! `activity_parity.rs`'s, over the extension's fixture.

use super::*;
use crate::activity::feed::typed;
use crate::history::activity::{entry_from_json, entry_to_json, FeePayer, UserFee};
use crate::history::activity_rules::user_fee;
use crate::host::testing::Memory;
use crate::js::json;
use crate::send::judge::Observation;
use serde_json::{json, Value};

const SCOPE: &str = "SN_MAIN:0x5dccf9cfdcabb9e4ef45814bda1b056bb23063d0e9b64e5a845e9dfb3d23f10";
const USDC: &str = "0x33068f6539f8e6e6b131e6b2b814e6c34a5224bc66947c47dab9dfee93b35fb";

fn fee() -> Value {
    json!({"amount": "1000", "token": "USDC", "tokenAddress": USDC})
}

fn row(extra: Value) -> Value {
    let mut row = json!({
        "accountId": SCOPE,
        "hash": "0x7a1ce5",
        "kind": "transfer",
        "networkId": "SN_MAIN",
        "origin": "wallet",
        "status": "pending",
        "submittedAt": 1,
        "summary": "Send 1 USDC (network fee 0.001 USDC)",
    });
    for (key, value) in extra.as_object().unwrap() {
        row[key] = value.clone();
    }
    row
}

fn entry(extra: Value) -> Entry {
    serde_json::from_value(row(extra)).unwrap()
}

fn typed_row(extra: Value) -> Option<ActivityEntry> {
    entry_from_json(&json::parse(&row(extra).to_string()).unwrap())
}

#[test]
fn a_user_fee_is_the_extensions_grammar_exactly() {
    let signed = user_fee(&fee()).unwrap();
    assert_eq!(
        signed,
        UserFee {
            amount: "1000".into(),
            token: "USDC".into(),
            token_address: USDC.into()
        }
    );
    let edited = |key: &str, value: Value| {
        let mut fee = fee();
        fee[key] = value;
        user_fee(&fee)
    };
    assert!(edited("amount", json!("9".repeat(78))).is_some());
    assert!(edited("token", json!("USDC.e")).is_some());
    assert!(edited("token", json!("A".repeat(32))).is_some());
    for (key, value) in [
        ("amount", json!("01000")),
        ("amount", json!("0")),
        ("amount", json!("")),
        ("amount", json!("10a")),
        ("amount", json!("9".repeat(79))),
        ("amount", json!(1000)),
        ("token", json!("US DC")),
        ("token", json!("")),
        ("token", json!("A".repeat(33))),
        ("tokenAddress", json!("0x0")),
        (
            "tokenAddress",
            json!("0x033068f6539f8e6e6b131e6b2b814e6c34a5224bc66947c47dab9dfee93b35fb"),
        ),
        ("tokenAddress", json!("USDC")),
        ("decimals", json!(6)),
    ] {
        assert_eq!(edited(key, value.clone()), None, "{key}: {value}");
    }
}

#[test]
fn a_row_carries_only_a_sponsor_and_only_a_valid_user_fee() {
    let gasless = typed_row(json!({"userFee": fee()})).unwrap();
    assert_eq!(gasless.user_fee, user_fee(&fee()));
    assert_eq!(entry_to_json(&gasless)["userFee"], fee());
    let sponsored = typed_row(json!({"feePayer": "sponsor"})).unwrap();
    assert_eq!(sponsored.fee_payer, Some(FeePayer::Sponsor));
    assert_eq!(entry_to_json(&sponsored)["feePayer"], json!("sponsor"));
    assert!(typed_row(json!({"feePayer": "user"})).is_none());
    assert!(
        typed_row(json!({"userFee": {"amount": "0", "token": "USDC", "tokenAddress": USDC}}))
            .is_none()
    );
    // The store refuses the same rows, so neither survives a write and a read.
    assert!(typed(&entry(json!({"userFee": fee(), "feePayer": "sponsor"}))).is_some());
    for refused in [
        json!({"feePayer": "user"}),
        json!({"userFee": {"amount": "1"}}),
    ] {
        let storage = Memory::default();
        let mut log = ActivityLog::new(&storage);
        log.record(entry(refused.clone()));
        assert!(log.list(SCOPE).is_empty(), "{refused}");
    }
}

#[test]
fn neither_a_sponsored_nor_a_gasless_row_takes_the_receipts_fee() {
    let hash = crate::felt::parse("0x7a1ce5").unwrap();
    let receipt = TransactionFee {
        amount: "0x10".into(),
        unit: FeeUnit::Fri,
    };
    for (extra, wants) in [
        (json!({}), true),
        (json!({"feePayer": "sponsor"}), false),
        (json!({"userFee": fee()}), false),
    ] {
        let storage = Memory::default();
        let mut log = ActivityLog::new(&storage);
        log.record(entry(extra.clone()));
        log.observe(
            SCOPE,
            &hash,
            Observation::Found {
                status: TxStatus::Accepted,
                accepted_finality: false,
                finalized_on_l1: false,
            },
        );
        assert_eq!(log.wants_fee(SCOPE, &hash), wants, "{extra}");
        assert_eq!(log.record_fee(SCOPE, &hash, &receipt), wants, "{extra}");
    }
}

#[test]
fn the_fee_fact_names_the_sponsor_or_the_signed_token_fee() {
    let usdc = |address: &str| (address == USDC).then_some(6);
    let paid = json!({"feePaid": {"amount": "0x10", "unit": "FRI"}});
    let sponsored = typed_row(json!({"feePayer": "sponsor", "feePaid": paid["feePaid"]})).unwrap();
    assert_eq!(fee_text(&sponsored, usdc).as_deref(), Some(SPONSORED));
    let gasless = typed_row(json!({"userFee": fee(), "feePaid": paid["feePaid"]})).unwrap();
    assert_eq!(fee_text(&gasless, usdc).as_deref(), Some("0.001 USDC"));
    // A token the wallet has no decimals for shows no fee, never the receipt's in its place.
    assert_eq!(fee_text(&gasless, |_| None), None);
    let own = typed_row(paid).unwrap();
    assert!(fee_text(&own, |_| None).is_some());
}

#[test]
fn a_gasless_summary_names_its_signed_fee_and_stays_clamped() {
    let amount = U256::from_u128(1000);
    assert_eq!(
        gasless_fee_summary("Send 1 USDC to 0x5ef2…9f01", &amount, 6, "USDC"),
        "Send 1 USDC to 0x5ef2…9f01 (network fee 0.001 USDC)"
    );
    let long = gasless_fee_summary(&"a".repeat(200), &amount, 6, "USDC");
    assert_eq!(long.encode_utf16().count(), 200);
    assert!(long.ends_with('…'));
}
