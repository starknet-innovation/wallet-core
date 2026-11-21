//! `staking-positions.test.ts` and the request half of `staking-positions-ops.test.ts`, over the
//! extension's schema-derived fixtures (`staking-positions-fixture.ts`, not a live capture).

use serde_json::{json, Value};

use super::directory_tests::{catching_up, coverage, merge};
use super::positions::*;
use super::read::StakingPair;
use super::test_chain::{f, NETWORK, POLICY, STRK};
use super::{StakingNetwork, StakingPolicy, StakingRefusal};
use crate::js::json;
use crate::portfolio::wire::HttpOutcome;
use crate::{Felt, NetworkId};

fn account() -> String {
    format!("0x0{}", "1".repeat(63))
}

fn position(address: &str, validator: &str, overrides: Value) -> Value {
    merge(
        json!({
            "address": address, "delegatedRaw": "5000000000000000000",
            "exitEligibilityReasonCode": "not_applicable", "exitEligibleAtIso": null,
            "exitIntentTransactionHash": null, "pendingExitRaw": null, "poolAddress": "0x5a3",
            "share": { "denominatorRaw": "1000", "numeratorRaw": "5" }, "status": "active",
            "tokenAddress": STRK, "validatorAddress": validator,
            "validatorGenerationId": format!("sha256:{}", "a".repeat(64)),
        }),
        overrides,
    )
}

fn view(address: &str, positions: Vec<Value>, overrides: Value) -> Value {
    merge(
        json!({
            "address": address, "chainId": "SN_MAIN", "coverage": coverage(json!({})),
            "history": [], "nextHistoryCursor": null, "positions": positions,
            "positionsTruncated": false, "source": "finalized_prepared_staking_snapshot",
        }),
        overrides,
    )
}

fn parse(value: &Value) -> Option<PositionsPage> {
    let value = json::parse(&value.to_string()).unwrap();
    parse_positions(&value, NetworkId::Mainnet, &f(&account()), &POLICY)
}

fn one(overrides: Value) -> Value {
    view(
        &account(),
        vec![position(&account(), "0x0a1", overrides)],
        json!({}),
    )
}

fn pair(staker: &str, token: &str) -> StakingPair {
    StakingPair {
        staker: f(staker),
        token: f(token),
    }
}

#[test]
fn reads_a_schema_shaped_answer_as_canonical_candidates_in_order() {
    let value = view(
        &account(),
        vec![
            position(&account(), "0x0a1", json!({})),
            position(
                &account(),
                "0xb2",
                json!({ "pendingExitRaw": "1", "status": "exiting" }),
            ),
        ],
        json!({}),
    );
    assert_eq!(
        parse(&value),
        Some(PositionsPage {
            complete: true,
            positions: vec![pair("0xa1", STRK), pair("0xb2", STRK)],
        })
    );
}

#[test]
fn accepts_the_account_in_any_felt_spelling_and_ignores_unread_fields() {
    let short = format!("0x{}", "1".repeat(63));
    let upper = short.to_uppercase().replace("0X", "0x");
    let value = view(
        &upper,
        vec![position(&short, "0x0a1", json!({}))],
        json!({ "extra": 1 }),
    );
    assert_eq!(parse(&value).map(|p| p.complete), Some(true));
}

#[test]
fn an_empty_list_is_complete_only_with_usable_untruncated_coverage() {
    let empty = |overrides: Value| parse(&view(&account(), vec![], overrides));
    let whole = Some(PositionsPage {
        complete: true,
        positions: vec![],
    });
    assert_eq!(empty(json!({})), whole);
    assert_eq!(empty(json!({ "coverage": catching_up(json!({})) })), whole);
    for coverage in [
        coverage(json!({ "status": "unavailable" })),
        catching_up(json!({ "lagBlocks": 101 })),
        catching_up(json!({ "gapIntervals": [{ "fromBlock": 1, "toBlock": 2 }] })),
        catching_up(json!({ "finalizedOnly": false })),
    ] {
        assert_eq!(
            empty(json!({ "coverage": coverage })).map(|p| p.complete),
            Some(false)
        );
    }
    let truncated = json!({ "coverage": catching_up(json!({})), "positionsTruncated": true });
    assert_eq!(empty(truncated).map(|p| p.complete), Some(false));
    let listed = view(
        &account(),
        vec![position(&account(), "0x0a1", json!({}))],
        json!({ "positionsTruncated": true }),
    );
    assert_eq!(
        parse(&listed),
        Some(PositionsPage {
            complete: false,
            positions: vec![pair("0xa1", STRK)],
        })
    );
}

#[test]
fn refuses_the_whole_answer_for_anything_malformed_or_out_of_bounds() {
    let base = view(
        &account(),
        vec![position(&account(), "0x0a1", json!({}))],
        json!({}),
    );
    let with = |overrides: Value| merge(base.clone(), overrides);
    let over: Vec<Value> = (1..=MAX_STAKING_POSITIONS + 1)
        .map(|i| position(&account(), &format!("{i:#x}"), json!({})))
        .collect();
    let other = format!("0x{}", "2".repeat(63));
    let cases = [
        Value::Null,
        json!([]),
        with(json!({ "chainId": "SN_SEPOLIA" })),
        with(json!({ "source": "live" })),
        with(json!({ "address": other })),
        with(json!({ "address": "not-a-felt" })),
        with(json!({ "coverage": coverage(json!({ "status": "done" })) })),
        with(json!({ "coverage": coverage(json!({ "lagBlocks": -1 })) })),
        with(json!({ "coverage": null })),
        with(json!({ "positionsTruncated": "false" })),
        with(json!({ "positions": {} })),
        view(&account(), over, json!({})),
        view(
            &account(),
            vec![
                position(&account(), "0x0a1", json!({})),
                position(&account(), "0xA1", json!({})),
            ],
            json!({}),
        ),
        view(&account(), vec![json!("0xa1")], json!({})),
        one(json!({ "address": other })),
        one(json!({ "validatorAddress": "0x0" })),
        one(json!({ "validatorAddress": format!("0x{}", "f".repeat(64)) })),
        one(json!({ "validatorAddress": "validator" })),
        one(json!({ "tokenAddress": null })),
        one(json!({ "poolAddress": "pool" })),
        one(json!({ "delegatedRaw": null })),
        one(json!({ "delegatedRaw": "-1" })),
        one(json!({ "delegatedRaw": "1".repeat(79) })),
        one(json!({ "pendingExitRaw": "0x1" })),
        one(json!({ "status": "withdrawn" })),
        one(json!({ "status": null })),
    ];
    for value in &cases {
        assert_eq!(parse(value), None, "{value}");
    }
    for key in [
        "positionsTruncated",
        "address",
        "chainId",
        "source",
        "coverage",
    ] {
        let mut value = base.clone();
        value.as_object_mut().unwrap().remove(key);
        assert_eq!(parse(&value), None, "{key}");
    }
    for key in ["pendingExitRaw", "poolAddress", "address", "status"] {
        let mut item = position(&account(), "0x0a1", json!({}));
        item.as_object_mut().unwrap().remove(key);
        assert_eq!(
            parse(&view(&account(), vec![item], json!({}))),
            None,
            "{key}"
        );
    }
}

#[test]
fn the_same_validator_with_another_token_is_a_separate_position() {
    let value = view(
        &account(),
        vec![
            position(&account(), "0x0a1", json!({})),
            position(&account(), "0x0a1", json!({ "tokenAddress": "0xb7c" })),
        ],
        json!({}),
    );
    assert_eq!(parse(&value).map(|p| p.positions.len()), Some(2));
}

#[test]
fn accepts_exactly_the_schemas_bound() {
    let many: Vec<Value> = (1..=MAX_STAKING_POSITIONS)
        .map(|i| position(&account(), &format!("{i:#x}"), json!({})))
        .collect();
    let page = parse(&view(&account(), many, json!({})));
    assert_eq!(page.map(|p| p.positions.len()), Some(MAX_STAKING_POSITIONS));
}

const BASE: &str = "https://starkscan.test/v1";

#[test]
fn the_request_sends_only_the_accounts_address_with_no_key() {
    let account = f(&account());
    let request = positions_request(NetworkId::Mainnet, &POLICY, BASE, &account, true)
        .unwrap()
        .unwrap();
    assert_eq!(
        request.url,
        format!(
            "{BASE}/SN_MAIN/staking/address/0x{}?limit=1",
            "1".repeat(63)
        )
    );
    assert_eq!(request.headers.len(), 1);
    assert_eq!(request.headers[0].value, "application/json");
}

#[test]
fn the_request_is_off_where_the_explorer_choice_or_the_network_keeps_it_from_starkscan() {
    let account = f(&account());
    assert_eq!(
        positions_request(NetworkId::Mainnet, &POLICY, BASE, &account, false),
        Ok(None)
    );
    const NO_STARKSCAN: [StakingNetwork; 1] = [StakingNetwork {
        starkscan: false,
        ..NETWORK
    }];
    let off = StakingPolicy {
        networks: &NO_STARKSCAN,
        ..POLICY
    };
    assert_eq!(
        positions_request(NetworkId::Mainnet, &off, BASE, &account, true),
        Ok(None)
    );
    assert_eq!(
        positions_request(NetworkId::Sepolia, &POLICY, BASE, &account, true),
        Err(StakingRefusal::Unavailable)
    );
}

fn outcome(status: u16, body: &Value) -> HttpOutcome {
    HttpOutcome {
        status,
        retry_after: None,
        body: Some(body.to_string().into_bytes()),
        timed_out: false,
    }
}

#[test]
fn a_failed_malformed_oversized_wrong_chain_or_wrong_address_answer_is_unreadable() {
    let account_felt = f(&account());
    let judge =
        |outcome: &HttpOutcome| positions_page(outcome, NetworkId::Mainnet, &account_felt, &POLICY);
    let good = one(json!({}));
    assert!(judge(&outcome(200, &good)).is_ok());
    let unreadable = Err(StakingRefusal::PositionsUnreadable);
    let other = format!("0x{}", "2".repeat(63));
    for (status, body) in [
        (403, json!("Forbidden")),
        (401, json!({ "code": "unauthorized" })),
        (429, json!({ "code": "rate_limited" })),
        (
            200,
            view(&account(), vec![], json!({ "chainId": "SN_SEPOLIA" })),
        ),
        (200, view(&other, vec![], json!({}))),
        (200, one(json!({ "status": "gone" }))),
        (200, json!({ "padding": "x".repeat(2 * 1024 * 1024 + 1) })),
    ] {
        assert_eq!(judge(&outcome(status, &body)), unreadable, "{status}");
    }
    // Parsed for one account, the same answer is unreadable for another.
    assert_eq!(
        positions_page(
            &outcome(200, &good),
            NetworkId::Mainnet,
            &Felt::from(7u8),
            &POLICY
        ),
        unreadable
    );
}
