//! `staking-directory-ops.test.ts`: the directory request (one bounded page, no key, off or
//! refused before anything is sent) and the judge of its reply.

use serde_json::{json, Value};

use super::directory::*;
use super::directory_tests::{catching_up, coverage, page, tokens, validator};
use super::test_chain::{NETWORK, POLICY};
use super::{StakingNetwork, StakingPolicy, StakingRefusal};
use crate::portfolio::wire::{HttpMethod, HttpOutcome};
use crate::NetworkId;

#[test]
fn the_request_is_one_bounded_page_with_no_key() {
    let base = "https://starkscan.test/v1";
    let request = directory_request(NetworkId::Mainnet, &POLICY, base, Some("next/page=2"))
        .unwrap()
        .unwrap();
    assert_eq!(request.method, HttpMethod::Get);
    assert_eq!(
        request.url,
        "https://starkscan.test/v1/SN_MAIN/staking/validators?limit=50&cursor=next%2Fpage%3D2"
    );
    assert_eq!(request.headers.len(), 1);
    assert_eq!(
        (
            request.headers[0].name.as_str(),
            request.headers[0].value.as_str()
        ),
        ("accept", "application/json")
    );
    let first = directory_request(NetworkId::Mainnet, &POLICY, base, None)
        .unwrap()
        .unwrap();
    assert_eq!(
        first.url,
        format!("{base}/SN_MAIN/staking/validators?limit=50")
    );
    let tricky = directory_request(NetworkId::Mainnet, &POLICY, base, Some("a*b-c.d_e~f!'()"));
    assert!(tricky
        .unwrap()
        .unwrap()
        .url
        .ends_with("&cursor=a*b-c.d_e%7Ef%21%27%28%29"));
}

#[test]
fn the_request_is_off_refused_or_invalid_before_anything_is_sent() {
    let base = "https://starkscan.test/v1";
    const NO_STARKSCAN: [StakingNetwork; 1] = [StakingNetwork {
        starkscan: false,
        ..NETWORK
    }];
    let quiet = StakingPolicy {
        networks: &[],
        ..POLICY
    };
    assert_eq!(
        directory_request(NetworkId::Mainnet, &quiet, base, None),
        Err(StakingRefusal::Unavailable)
    );
    let off = StakingPolicy {
        networks: &NO_STARKSCAN,
        ..POLICY
    };
    assert_eq!(
        directory_request(NetworkId::Mainnet, &off, base, None),
        Ok(None)
    );
    for cursor in ["", "a b", &"x".repeat(1025)] {
        assert_eq!(
            directory_request(NetworkId::Mainnet, &POLICY, base, Some(cursor)),
            Err(StakingRefusal::InvalidRequest)
        );
    }
}

fn outcome(status: u16, body: &Value) -> HttpOutcome {
    HttpOutcome {
        status,
        retry_after: None,
        body: Some(body.to_string().into_bytes()),
        timed_out: false,
    }
}

fn view(outcome: &HttpOutcome) -> Result<DirectoryView, StakingRefusal> {
    directory_view(outcome, NetworkId::Mainnet, &tokens(), &POLICY)
}

#[test]
fn lists_while_catching_up_just_behind_exactly_as_prepared() {
    let staker = "0x5a200000000000000000000000000000000000000000000000000000000beef";
    let prepared = view(&outcome(
        200,
        &page(vec![validator(staker, json!({}))], json!({})),
    ));
    let behind = page(
        vec![validator(staker, json!({}))],
        json!({ "coverage": catching_up(json!({})) }),
    );
    assert_eq!(view(&outcome(200, &behind)), prepared);
    let Ok(DirectoryView::Ready {
        entries,
        next_cursor,
    }) = prepared
    else {
        panic!("ready");
    };
    assert_eq!(next_cursor, None);
    assert_eq!(entries[0].staker, staker);
    assert_eq!(entries[0].uptime.as_deref(), Some("99.5%"));
}

#[test]
fn offers_no_validators_while_the_snapshot_is_incomplete() {
    for coverage in [
        coverage(json!({ "status": "unavailable" })),
        catching_up(json!({ "lagBlocks": 101 })),
        catching_up(json!({ "gapIntervals": [{}] })),
        catching_up(json!({ "outstandingMaterializationIntervals": [{}] })),
        catching_up(json!({ "finalizedOnly": false })),
    ] {
        let body = page(
            vec![validator("0x1", json!({}))],
            json!({ "coverage": coverage }),
        );
        assert_eq!(view(&outcome(200, &body)), Ok(DirectoryView::Incomplete));
    }
}

#[test]
fn fails_closed_on_a_failed_malformed_oversized_or_wrong_chain_answer() {
    let good = page(vec![validator("0x1", json!({}))], json!({}));
    let unreadable = Err(StakingRefusal::DirectoryUnreadable);
    assert_eq!(view(&outcome(403, &json!("Forbidden"))), unreadable);
    assert_eq!(
        view(&outcome(401, &json!({ "code": "unauthorized" }))),
        unreadable
    );
    assert_eq!(view(&outcome(503, &good)), unreadable);
    let failed = HttpOutcome {
        status: 0,
        retry_after: None,
        body: None,
        timed_out: true,
    };
    assert_eq!(view(&failed), unreadable);
    assert_eq!(
        view(&outcome(
            200,
            &page(vec![], json!({ "chainId": "SN_SEPOLIA" }))
        )),
        unreadable
    );
    let long_name = page(
        vec![validator("0x1", json!({ "name": "x".repeat(65) }))],
        json!({}),
    );
    assert_eq!(view(&outcome(200, &long_name)), unreadable);
    let thin = page(vec![], json!({ "coverage": { "status": "prepared" } }));
    assert_eq!(view(&outcome(200, &thin)), unreadable);
    let padded = json!({ "padding": "x".repeat(2 * 1024 * 1024 + 1) });
    assert_eq!(view(&outcome(200, &padded)), unreadable);
    // A body over the bound is refused before it is parsed: valid JSON, padded past the limit.
    let mut big = good.to_string().into_bytes();
    big.extend(std::iter::repeat_n(b' ', 2 * 1024 * 1024));
    let oversized = HttpOutcome {
        body: Some(big),
        ..outcome(200, &good)
    };
    assert_eq!(view(&oversized), unreadable);
    assert!(view(&outcome(200, &good)).is_ok());
}
