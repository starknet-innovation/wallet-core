//! `staking-directory.test.ts` and `staking-directory-ops.test.ts`, over the extension's
//! schema-derived fixtures (`staking-directory-fixture.ts`, not a live capture): every field the
//! wallet reads is bounded and checked, a malformed page is refused whole, incomplete coverage
//! is reported, null figures stay unknown, and only active validators with an active pool for a
//! tracked token are offered.

use serde_json::{json, Value};

use super::coverage::Coverage;
use super::directory::*;
use super::test_chain::{f, strk_config, POLICY, STRK};
use super::StakingPolicy;
use crate::js::json::{self, JsValue};
use crate::portfolio::model::TokenConfig;
use crate::NetworkId;

const BTC: &str = "0xb7c";

pub(super) fn merge(mut base: Value, overrides: Value) -> Value {
    if let (Some(base), Some(overrides)) = (base.as_object_mut(), overrides.as_object()) {
        for (key, value) in overrides {
            base.insert(key.clone(), value.clone());
        }
    }
    base
}

fn ratio(numerator: &str, denominator: &str) -> Value {
    json!({ "denominatorRaw": denominator, "numeratorRaw": numerator })
}

fn pool(token: &str, address: &str, status: &str) -> Value {
    json!({
        "address": address, "commission": ratio("1000", "10000"), "generationRaw": "1",
        "hhi": null, "metricValueReason": null, "status": status, "tokenAddress": token,
        "top1Share": null, "top5Share": null,
    })
}

fn stake(token: &str, raw: Value) -> Value {
    json!({
        "decimals": 18, "delegatedStakeRaw": raw, "metricValueReason": null,
        "networkShare": null, "pendingExitRaw": "0", "selfStakeRaw": "0", "symbol": "STRK",
        "tokenAddress": token, "totalStakeRaw": raw,
    })
}

pub(super) fn validator(address: &str, overrides: Value) -> Value {
    merge(
        json!({
            "address": address, "attestationDelay30d": null, "commission": ratio("500", "10000"),
            "delegatorCount": 12, "lastAttestationEpochRaw": "100",
            "liveness30d": ratio("995", "1000"), "liveness30dFirstEpoch": 1,
            "metricValueReason": null, "missedStreak": 0, "name": "Validator One",
            "operationalAddress": null, "pools": [pool(STRK, "0x5a3", "active")],
            "protocolSlug": null, "rewardAddress": null, "rewards": [],
            "stake": [stake(STRK, json!("1500000000000000000000"))], "status": "active",
            "validatorGenerationId": format!("sha256:{}", "a".repeat(64)),
            "website": "https://validator.example/about",
        }),
        overrides,
    )
}

pub(super) fn coverage(overrides: Value) -> Value {
    merge(
        json!({
            "activityHistoryTruncated": false, "finalizedOnly": true, "gapIntervals": [],
            "lagBlocks": 0, "lastSuccessfulRunAtIso": "2026-11-06T00:00:00Z",
            "materializedThroughBlock": 100, "outstandingMaterializationIntervals": [],
            "reasonCode": "ok", "sourceLatestFinalizedBlock": 100, "status": "prepared",
        }),
        overrides,
    )
}

/// `CATCHING_UP_COVERAGE`: Starkscan's Mainnet steady state, 28 blocks behind.
pub(super) fn catching_up(overrides: Value) -> Value {
    let base = coverage(json!({
        "activityHistoryTruncated": true, "lagBlocks": 28, "status": "catching_up",
        "reasonCode": "bounded_materialization_and_activity_window",
    }));
    merge(base, overrides)
}

pub(super) fn page(items: Vec<Value>, overrides: Value) -> Value {
    merge(
        json!({
            "chainId": "SN_MAIN", "coverage": coverage(json!({})), "items": items,
            "nextCursor": null, "source": "finalized_prepared_staking_snapshot",
        }),
        overrides,
    )
}

fn js(value: &Value) -> JsValue {
    json::parse(&value.to_string()).unwrap()
}

fn parse(value: &Value) -> Option<DirectoryPage> {
    parse_directory_page(&js(value), NetworkId::Mainnet, &POLICY)
}

fn with_validator(overrides: Value) -> Value {
    page(vec![validator("0x0a1", overrides)], json!({}))
}

pub(super) fn tokens() -> Vec<TokenConfig> {
    let btc = TokenConfig {
        id: "SN_MAIN:wbtc".into(),
        address: BTC.into(),
        symbol: "WBTC".into(),
        name: "Wrapped BTC".into(),
        decimals: 8,
        ..strk_config()
    };
    vec![strk_config(), btc]
}

#[test]
fn reads_a_schema_shaped_page_canonicalising_addresses_and_keeping_the_order() {
    let value = page(
        vec![
            validator("0x0a1", json!({})),
            validator("0xb2", json!({ "name": null })),
        ],
        json!({ "nextCursor": "opaque:cursor_1" }),
    );
    let page = parse(&value).unwrap();
    assert_eq!(page.coverage, Coverage::Usable);
    assert_eq!(page.next_cursor.as_deref(), Some("opaque:cursor_1"));
    let stakers: Vec<_> = page.validators.iter().map(|v| v.staker).collect();
    assert_eq!(stakers, [f("0xa1"), f("0xb2")]);
    let first = &page.validators[0];
    assert_eq!(first.commission_ppm, Some(50_000));
    assert_eq!(first.liveness_30d_ppm, Some(995_000));
    assert_eq!(first.name.as_deref(), Some("Validator One"));
    assert_eq!(first.status, Some(ValidatorStatus::Active));
    assert_eq!(first.website.as_deref(), Some("validator.example"));
    assert_eq!(page.validators[1].name, None);
    // A blank name is unknown, and a name is trimmed.
    let blank = parse(&with_validator(json!({ "name": "  " }))).unwrap();
    assert_eq!(blank.validators[0].name, None);
    let padded = parse(&with_validator(json!({ "name": " One " }))).unwrap();
    assert_eq!(padded.validators[0].name.as_deref(), Some("One"));
}

#[test]
fn null_figures_stay_unknown_never_zero() {
    let value = with_validator(json!({
        "commission": null, "liveness30d": null, "stake": [stake(STRK, Value::Null)],
    }));
    let validator = &parse(&value).unwrap().validators[0];
    assert_eq!(validator.commission_ppm, None);
    assert_eq!(validator.liveness_30d_ppm, None);
    assert_eq!(validator.delegated, vec![(f(STRK), None)]);
}

#[test]
fn reads_coverage_through_the_shared_rule() {
    let coverage_of = |coverage: Value| {
        parse(&page(
            vec![validator("0x0a1", json!({}))],
            json!({ "coverage": coverage }),
        ))
        .map(|p| p.coverage)
    };
    assert_eq!(coverage_of(catching_up(json!({}))), Some(Coverage::Usable));
    assert_eq!(
        coverage_of(coverage(json!({ "status": "unavailable" }))),
        Some(Coverage::Incomplete)
    );
    assert_eq!(
        coverage_of(catching_up(json!({ "lagBlocks": 101 }))),
        Some(Coverage::Incomplete)
    );
    // The lag bound is the policy's.
    let tight = StakingPolicy {
        max_catching_up_lag_blocks: 27,
        ..POLICY
    };
    let value = page(vec![], json!({ "coverage": catching_up(json!({})) }));
    let judged = parse_directory_page(&js(&value), NetworkId::Mainnet, &tight);
    assert_eq!(judged.map(|p| p.coverage), Some(Coverage::Incomplete));
}

#[test]
fn a_website_is_only_ever_an_https_hostname() {
    let site = |website: Value| {
        parse(&with_validator(json!({ "website": website })))
            .map(|p| p.validators[0].website.clone())
    };
    assert_eq!(
        site(json!("https://ok.example/path?x=1")),
        Some(Some("ok.example".into()))
    );
    assert_eq!(
        site(json!("HTTPS://Ok.Example:443#top")),
        Some(Some("ok.example".into()))
    );
    for dropped in [
        "javascript:alert(1)",
        "http://plain.example",
        "data:text/html,hi",
        "https://user:pw@creds.example",
        "https://user@creds.example",
        "not a url",
        "https://1.2.3.4/",
        "https://ok.example:99999/",
        "https://",
    ] {
        assert_eq!(site(json!(dropped)), Some(None), "{dropped}");
    }
    assert_eq!(site(Value::Null), Some(None));
    assert_eq!(site(json!("   ")), Some(None), "blank is unknown");
    // Over-long or not text at all refuses the page.
    let long = format!("https://{}.example", "a".repeat(200));
    assert_eq!(site(json!(long)), None);
    assert_eq!(site(json!(7)), None);
}

#[test]
fn refuses_the_whole_page_on_anything_malformed_or_out_of_bounds() {
    let many: Vec<Value> = (1..=51)
        .map(|i| validator(&format!("{i:#x}"), json!({})))
        .collect();
    let pools: Vec<Value> = (1..=33)
        .map(|i| pool(STRK, &format!("{i:#x}"), "active"))
        .collect();
    let a = || validator("0x0a1", json!({}));
    let cases = [
        Value::Null,
        json!([]),
        page(vec![a()], json!({ "chainId": "SN_SEPOLIA" })),
        page(vec![a()], json!({ "source": "live" })),
        page(
            vec![a()],
            json!({ "coverage": coverage(json!({ "status": "done" })) }),
        ),
        page(
            vec![a()],
            json!({ "coverage": coverage(json!({ "lagBlocks": "0" })) }),
        ),
        page(vec![a()], json!({ "coverage": null })),
        page(vec![a()], json!({ "nextCursor": "has space" })),
        page(vec![a()], json!({ "nextCursor": "c".repeat(1025) })),
        page(vec![a()], json!({ "items": {} })),
        page(many, json!({})),
        page(vec![a(), validator("0xa1", json!({}))], json!({})),
        with_validator(json!({ "address": "0x0" })),
        with_validator(json!({ "address": "a1" })),
        with_validator(json!({ "address": format!("0x{}", "f".repeat(64)) })),
        with_validator(json!({ "name": "n".repeat(65) })),
        with_validator(json!({ "name": "Evil\u{202e}Name" })),
        with_validator(json!({ "name": "Line\nbreak" })),
        with_validator(json!({ "name": 5 })),
        with_validator(json!({ "status": "paused" })),
        with_validator(json!({ "commission": ratio("0", "0") })),
        with_validator(json!({ "commission": ratio("11", "10") })),
        with_validator(json!({ "commission": ratio("-1", "10") })),
        with_validator(
            json!({ "commission": { "denominatorRaw": "10", "extra": 1, "numeratorRaw": "1" } }),
        ),
        with_validator(json!({ "commission": { "denominatorRaw": "10" } })),
        with_validator(json!({ "commission": ratio("1", "10").to_string() })),
        with_validator(json!({ "liveness30d": "99%" })),
        with_validator(json!({ "liveness30d": ratio("1", &"1".repeat(79)) })),
        with_validator(json!({ "pools": pools })),
        with_validator(json!({ "pools": [pool(STRK, "0x5a3", "exiting")] })),
        with_validator(json!({ "pools": [pool("0xnope", "0x5a3", "active")] })),
        with_validator(
            json!({ "pools": [merge(pool(STRK, "0x5a3", "active"), json!({ "address": 1 }))] }),
        ),
        with_validator(json!({ "pools": "none" })),
        with_validator(json!({ "stake": [stake(STRK, json!("1.5"))] })),
        with_validator(json!({ "stake": [stake("0x", json!("1"))] })),
        with_validator(json!({ "stake": null })),
        with_validator(
            json!({ "stake": (0..33).map(|_| stake(STRK, json!("1"))).collect::<Vec<_>>() }),
        ),
    ];
    for value in &cases {
        assert_eq!(parse(value), None, "{value}");
    }
    // A missing field the wallet reads is as malformed as a wrong one.
    for key in [
        "name",
        "website",
        "status",
        "commission",
        "liveness30d",
        "pools",
        "stake",
    ] {
        let mut value = validator("0x0a1", json!({}));
        value.as_object_mut().unwrap().remove(key);
        assert_eq!(parse(&page(vec![value], json!({}))), None, "{key}");
    }
    for key in ["chainId", "source", "coverage", "nextCursor", "items"] {
        let mut value = page(vec![a()], json!({}));
        value.as_object_mut().unwrap().remove(key);
        assert_eq!(parse(&value), None, "{key}");
    }
    // The bounds themselves are accepted.
    let fifty: Vec<Value> = (1..=50)
        .map(|i| validator(&format!("{i:#x}"), json!({})))
        .collect();
    assert!(parse(&page(fifty, json!({}))).is_some());
    let thirty_two: Vec<Value> = (1..=32)
        .map(|i| pool(STRK, &format!("{i:#x}"), "active"))
        .collect();
    assert!(parse(&with_validator(json!({ "pools": thirty_two }))).is_some());
    assert!(parse(&with_validator(json!({ "name": "n".repeat(64) }))).is_some());
}

#[test]
fn tolerates_fields_it_does_not_read() {
    assert!(parse(&with_validator(
        json!({ "futureMetric": { "anything": true } })
    ))
    .is_some());
}

#[test]
fn a_ratio_is_floored_to_parts_per_million_at_any_size() {
    let ppm = |numerator: &str, denominator: &str| {
        parse(&with_validator(
            json!({ "commission": ratio(numerator, denominator) }),
        ))
        .map(|p| p.validators[0].commission_ppm)
    };
    assert_eq!(ppm("1", "3"), Some(Some(333_333)));
    assert_eq!(ppm("2", "3"), Some(Some(666_666)));
    assert_eq!(ppm("7", "7"), Some(Some(1_000_000)));
    assert_eq!(ppm("0", "7"), Some(Some(0)));
    assert_eq!(ppm("007", "10"), Some(Some(700_000)));
    let huge = "9".repeat(78);
    let half = format!("4{}", "9".repeat(77));
    assert_eq!(ppm(&half, &huge), Some(Some(499_999)));
    assert_eq!(ppm(&huge, &huge), Some(Some(1_000_000)));
}

#[test]
fn offers_only_active_validators_with_an_active_pool_for_a_tracked_token() {
    let value = page(
        vec![
            validator("0x1", json!({})),
            validator("0x2", json!({ "status": "inactive" })),
            validator("0x3", json!({ "status": "exiting" })),
            validator("0x4", json!({ "status": null })),
            validator("0x5", json!({ "pools": [pool(STRK, "0x5a3", "retired")] })),
            validator(
                "0x6",
                json!({ "pools": [pool("0x999", "0x5a3", "active")] }),
            ),
            validator("0x7", json!({ "pools": [] })),
            validator(
                "0x8",
                json!({
                    "pools": [pool(STRK, "0x5a3", "retired"), pool(BTC, "0x5a4", "active")],
                    "stake": [],
                }),
            ),
        ],
        json!({}),
    );
    let page = parse(&value).unwrap();
    let entries = directory_entries(&page, &tokens(), NetworkId::Mainnet);
    let stakers: Vec<&str> = entries.iter().map(|e| e.staker.as_str()).collect();
    assert_eq!(stakers, ["0x1", "0x8"]);
    assert_eq!(
        entries[0].pools,
        [DirectoryPoolEntry {
            delegated: Some("1500".into()),
            symbol: "STRK".into(),
        }]
    );
    // A pool with no stake figure shows its total as unknown.
    assert_eq!(
        entries[1].pools,
        [DirectoryPoolEntry {
            delegated: None,
            symbol: "WBTC".into(),
        }]
    );
    assert!(directory_entries(&page, &[], NetworkId::Mainnet).is_empty());
    // A token tracked on another network is not this network's.
    assert!(directory_entries(&page, &tokens(), NetworkId::Sepolia).is_empty());
}

#[test]
fn formats_figures_for_display_and_keeps_unknown_as_none() {
    let value = page(
        vec![
            validator("0x1", json!({})),
            validator("0x2", json!({ "commission": null, "liveness30d": null })),
        ],
        json!({}),
    );
    let entries = directory_entries(&parse(&value).unwrap(), &tokens(), NetworkId::Mainnet);
    assert_eq!(entries[0].commission.as_deref(), Some("5%"));
    assert_eq!(entries[0].uptime.as_deref(), Some("99.5%"));
    assert_eq!(entries[0].website.as_deref(), Some("validator.example"));
    assert_eq!(
        (
            entries[1].commission.as_deref(),
            entries[1].uptime.as_deref()
        ),
        (None, None)
    );
}

#[test]
fn percentages_round_down_so_99_996_never_reads_as_100() {
    assert_eq!(ppm_percent(999_960), "99.99%");
    assert_eq!(ppm_percent(999_999), "99.99%");
    assert_eq!(ppm_percent(1_000_000), "100%");
    assert_eq!(ppm_percent(0), "0%");
    assert_eq!(ppm_percent(123_400), "12.34%");
    assert_eq!(ppm_percent(100_000), "10%");
    assert_eq!(ppm_percent(105_000), "10.5%");
}

#[test]
fn a_cursor_is_bounded_printable_ascii() {
    assert!(is_directory_cursor("eyJhIjoxfQ=="));
    assert!(is_directory_cursor(&"x".repeat(1024)));
    for bad in ["", " ", "a b", "\u{fc}", "\u{7f}"] {
        assert!(!is_directory_cursor(bad), "{bad:?}");
    }
    assert!(!is_directory_cursor(&"x".repeat(1025)));
}
