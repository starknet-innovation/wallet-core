//! `starkscan-nfts.test.ts`, over its schema-derived fixture (`starkscan-nft-fixture.ts`).

use serde_json::{json, Value};

use super::*;
use crate::nft::TEST_POLICY;

/// `STARKSCAN_OWNER`.
const OWNER: &str = "0x0234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";

fn position(token_id: &str, overrides: Value) -> Value {
    let mut row = json!({
        "collection": {"metadataStatus": "indexed", "name": "Punks", "symbol": "PNK"},
        "evidenceSource": "indexed_erc721_ownership",
        "lastTransferHash": "0x7a1b",
        "observedAtBlock": 1_200_000,
        "standard": "erc721",
        "tokenAddress": "0x511a2b3",
        "tokenId": token_id,
    });
    merge(&mut row, overrides);
    row
}

fn coverage(overrides: Value) -> Value {
    let mut row = json!({
        "completeWithinScope": true,
        "declaredScope": "indexed_standard_erc721_transfers",
        "globallyComplete": false,
        "indexedFromBlock": 0,
        "indexedThroughBlock": 1_250_000,
        "reasonCodes": ["unsupported_erc1155"],
        "status": "ready",
    });
    merge(&mut row, overrides);
    row
}

fn page(items: Option<Vec<Value>>, overrides: Value) -> Value {
    let items = items.unwrap_or_else(|| vec![position("1", json!({})), position("2", json!({}))]);
    let mut row = json!({
        "chainId": "SN_MAIN",
        "coverage": coverage(json!({})),
        "hasMore": false,
        "items": items,
        "nextCursor": null,
        "ownerAddress": OWNER,
        "schemaVersion": "starkscan.nft_positions.v1",
        "snapshotBlock": 1_250_000,
    });
    merge(&mut row, overrides);
    row
}

// `{ ...base, ...overrides }`, where an `undefined` override is spelled as the key removed.
fn merge(row: &mut Value, overrides: Value) {
    if let (Some(row), Value::Object(overrides)) = (row.as_object_mut(), overrides) {
        for (key, value) in overrides {
            if value == json!("<undefined>") {
                row.remove(&key);
            } else {
                row.insert(key, value);
            }
        }
    }
}

fn answer(body: &Value, status: u16) -> Option<NftDiscovery> {
    let outcome = HttpOutcome {
        status,
        retry_after: None,
        body: Some(body.to_string().into_bytes()),
        timed_out: false,
    };
    judge(&outcome, OWNER)
}

#[test]
fn requests_one_page_with_only_the_address_and_limit() {
    let request = request(OWNER, &TEST_POLICY).unwrap();
    assert_eq!(
        request.url,
        format!(
            "https://starkscan.test/v1/SN_MAIN/address/0x{}/nft-positions?limit=100",
            &OWNER[3..]
        )
    );
    assert_eq!(request.headers.len(), 1);
    assert_eq!(request.headers[0].name, "accept");
    assert_eq!(request.headers[0].value, "application/json");
}

#[test]
fn refuses_an_out_of_range_limit_before_any_request() {
    for limit in [0, 101] {
        let policy = NftPolicy {
            starkscan_page_size: limit,
            ..TEST_POLICY
        };
        assert_eq!(request(OWNER, &policy), None);
    }
    assert_eq!(request("0x0", &TEST_POLICY), None);
}

#[test]
fn refuses_a_malformed_page() {
    let many: Vec<Value> = (0..101)
        .map(|i| position(&i.to_string(), json!({})))
        .collect();
    let cases: Vec<(&str, Value)> = vec![
        ("a non-object body", json!([])),
        (
            "another schema version",
            page(None, json!({"schemaVersion": "v2"})),
        ),
        (
            "a malformed owner",
            page(None, json!({"ownerAddress": "me"})),
        ),
        (
            "a missing hasMore",
            page(None, json!({"hasMore": "<undefined>"})),
        ),
        ("a numeric cursor", page(None, json!({"nextCursor": 5}))),
        (
            "a negative snapshot block",
            page(None, json!({"snapshotBlock": -1})),
        ),
        ("more items than a page", page(Some(many), json!({}))),
        (
            "items that are not an array",
            page(None, json!({"items": {}})),
        ),
        ("a missing coverage", page(None, json!({"coverage": null}))),
        (
            "an unknown coverage status",
            page(
                Some(vec![]),
                json!({"coverage": coverage(json!({"status": "done"}))}),
            ),
        ),
        (
            "non-string reason codes",
            page(
                Some(vec![]),
                json!({"coverage": coverage(json!({"reasonCodes": [1]}))}),
            ),
        ),
        (
            "a fractional indexed block",
            page(
                Some(vec![]),
                json!({"coverage": coverage(json!({"indexedThroughBlock": 1.5}))}),
            ),
        ),
        (
            "an ERC-1155 item",
            page(
                Some(vec![position("1", json!({"standard": "erc1155"}))]),
                json!({}),
            ),
        ),
        (
            "other evidence",
            page(
                Some(vec![position("1", json!({"evidenceSource": "guess"}))]),
                json!({}),
            ),
        ),
        (
            "a non-felt contract",
            page(
                Some(vec![position("1", json!({"tokenAddress": "0xzz"}))]),
                json!({}),
            ),
        ),
        (
            "a negative token id",
            page(Some(vec![position("-1", json!({}))]), json!({})),
        ),
        (
            "a numeric token id",
            page(Some(vec![position("1", json!({"tokenId": 1}))]), json!({})),
        ),
        (
            "a malformed transfer hash",
            page(
                Some(vec![position("1", json!({"lastTransferHash": "x"}))]),
                json!({}),
            ),
        ),
        (
            "a missing observed block",
            page(
                Some(vec![position("1", json!({"observedAtBlock": null}))]),
                json!({}),
            ),
        ),
        (
            "a missing collection",
            page(
                Some(vec![position("1", json!({"collection": null}))]),
                json!({}),
            ),
        ),
        (
            "an unknown metadata status",
            page(
                Some(vec![position(
                    "1",
                    json!({"collection": {"metadataStatus": "x", "name": null, "symbol": null}}),
                )]),
                json!({}),
            ),
        ),
        (
            "a numeric collection name",
            page(
                Some(vec![position(
                    "1",
                    json!({"collection": {"metadataStatus": "indexed", "name": 5, "symbol": null}}),
                )]),
                json!({}),
            ),
        ),
    ];
    for (name, body) in cases {
        let parsed = crate::js::json::parse(&body.to_string()).and_then(|value| parse_page(&value));
        assert!(parsed.is_none(), "{name}");
        assert_eq!(answer(&body, 200), None, "{name}");
    }
}

#[test]
fn a_complete_page_yields_canonical_candidates() {
    let body = page(
        Some(vec![position(
            "0xff",
            json!({"tokenAddress": "0x0511A2B3"}),
        )]),
        json!({}),
    );
    let discovery = answer(&body, 200).unwrap();
    assert_eq!(discovery.coverage, NftCoverage::Complete);
    let [nft] = discovery.nfts.as_slice() else {
        panic!("one candidate");
    };
    assert_eq!(felt::canonical(&nft.id.contract), "0x511a2b3");
    assert_eq!(nft.id.token_id.to_decimal(), "255");
    assert_eq!(
        (
            nft.collection.as_str(),
            nft.name.as_str(),
            nft.image.as_str()
        ),
        ("Punks", "", "")
    );
}

#[test]
fn an_empty_complete_page_is_a_real_empty_answer() {
    let discovery = answer(&page(Some(vec![]), json!({})), 200).unwrap();
    assert_eq!(discovery, NftDiscovery::empty(NftCoverage::Complete));
}

#[test]
fn coverage_that_is_not_complete_is_incomplete() {
    let cases = [
        json!({"coverage": coverage(json!({"status": "partial"}))}),
        json!({"coverage": coverage(json!({"completeWithinScope": false}))}),
        json!({"coverage": coverage(json!({"declaredScope": "erc721_and_more"}))}),
        json!({"hasMore": true, "nextCursor": "c1"}),
        json!({"coverage": coverage(
            json!({"reasonCodes": ["unsupported_erc1155", "indexed_history_floor"]})
        )}),
        json!({"coverage": coverage(json!({"reasonCodes": ["new_gap"]}))}),
    ];
    for overrides in cases {
        let discovery = answer(&page(Some(vec![]), overrides.clone()), 200);
        assert_eq!(
            discovery,
            Some(NftDiscovery::empty(NftCoverage::Incomplete)),
            "{overrides}"
        );
    }
}

#[test]
fn unavailable_never_empty() {
    let past_u256 = format!("1{}", "0".repeat(77)); // 10^77 < 2^256 < 10^78
    let cases = [
        (page(None, json!({"chainId": "SN_SEPOLIA"})), 200),
        (page(None, json!({"ownerAddress": "0x999"})), 200),
        (
            page(Some(vec![position("1", json!({"tokenAddress": "0x0"}))]), json!({})),
            200,
        ),
        (
            page(
                Some(vec![position(
                    "115792089237316195423570985008687907853269984665640564039457584007913129639936",
                    json!({}),
                )]),
                json!({}),
            ),
            200,
        ),
        (
            page(
                Some(vec![position("1", json!({})), position("0x1", json!({}))]),
                json!({}),
            ),
            200,
        ),
        (json!({"items": []}), 200),
        (json!({"code": "forbidden"}), 403),
        (json!({"code": "rate_limited"}), 429),
        (json!({"code": "unavailable"}), 503),
    ];
    for (body, status) in cases {
        assert_eq!(answer(&body, status), None, "{body}");
    }
    // Just under the bound is a candidate.
    let body = page(Some(vec![position(&past_u256, json!({}))]), json!({}));
    assert!(answer(&body, 200).is_some());
    let transport = HttpOutcome {
        status: 0,
        retry_after: None,
        body: None,
        timed_out: false,
    };
    assert_eq!(judge(&transport, OWNER), None);
}

#[test]
fn a_hostile_collection_name_is_dropped_not_refusing_the_page() {
    let named = |name: Value, status: &str| {
        position(
            "1",
            json!({"collection": {"metadataStatus": status, "name": name, "symbol": null}}),
        )
    };
    for name in [
        json!("Punks\u{202e}evil"),
        json!("a\u{7}b"),
        json!("Punks\u{061c}"),
        json!("x".repeat(65)),
        Value::Null,
    ] {
        let discovery = answer(
            &page(Some(vec![named(name.clone(), "indexed")]), json!({})),
            200,
        );
        assert_eq!(discovery.unwrap().nfts[0].collection, "", "{name}");
    }
    let stale = answer(
        &page(Some(vec![named(json!("Stale"), "unavailable")]), json!({})),
        200,
    );
    assert_eq!(stale.unwrap().nfts[0].collection, "");
    let padded = answer(
        &page(Some(vec![named(json!(" Punks "), "indexed")]), json!({})),
        200,
    );
    assert_eq!(padded.unwrap().nfts[0].collection, "Punks");
}
