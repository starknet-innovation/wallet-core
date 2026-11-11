//! `PrivacyReadPlan` behaviour the fixture cannot show: reply ordering, lock, budgets, the page
//! cap, retries restarting from page one, the pinned `block_ref`, size bounds, configuration, and
//! where the viewing key may appear. Driven over a test `PrivacyHost`; the wallet's own checks
//! (an unknown account, a locked wallet at prepare) are Bramble mobile's tests.

mod portfolio_support;
mod privacy_support;

use privacy_support::{
    account_address, drive, mainnet, mainnet_plan, routes, routing, run_mainnet, steady, wire,
    Host, Script, MAINNET_CHAIN, NOW, PHRASE,
};
use serde_json::json;
use starknet_wallet_core::accounts::Ladder;
use starknet_wallet_core::catalog::network_info;
use starknet_wallet_core::privacy::config::route_for;
use starknet_wallet_core::privacy::plan::{requested_token_keys, PrivacyReadPlan};
use starknet_wallet_core::privacy::types::{
    PrivacyError, PrivacyFailure, PrivacyPurpose, PrivacyReply, PrivacyStep,
};
use starknet_wallet_core::{account_key, derive_strk20_viewing_key, felt, NetworkId};

fn chain_reply(id: u64, now: u64) -> PrivacyReply {
    PrivacyReply::Rpc {
        id,
        body: format!(r#"{{"jsonrpc":"2.0","id":{id},"result":"{MAINNET_CHAIN}"}}"#),
        now_ms: now,
    }
}

fn id_of(step: &PrivacyStep) -> u64 {
    match step {
        PrivacyStep::Post { id, .. } | PrivacyStep::Rpc { id, .. } => *id,
        other => panic!("no id on {other:?}"),
    }
}

fn http(status: u16, body: serde_json::Value) -> (u16, Vec<u8>) {
    (status, body.to_string().into_bytes())
}

fn registered() -> (u16, Vec<u8>) {
    http(
        200,
        json!({ "block_ref": "0x1", "sender_registered": true }),
    )
}

fn fee() -> String {
    r#"{"jsonrpc":"2.0","id":0,"result":["0x5"]}"#.into()
}

fn incomplete_page(block: &str) -> (u16, Vec<u8>) {
    http(
        200,
        json!({ "block_ref": block, "channels": [], "notes": [], "cursor": { "channel_discovery_complete": false } }),
    )
}

fn complete_page(block: &str) -> (u16, Vec<u8>) {
    http(
        200,
        json!({ "block_ref": block, "channels": [], "notes": [], "cursor": { "channel_discovery_complete": true } }),
    )
}

fn script(pages: Vec<(u16, Vec<u8>)>) -> Script {
    Script {
        preflight: vec![registered()].into(),
        pages: pages.into(),
        fee: vec![fee()].into(),
    }
}

#[test]
fn a_reply_out_of_order_closes_the_plan() {
    let host = Host::new();
    let mut plan = mainnet_plan();
    let chain = plan
        .advance(&host, PrivacyReply::Start { now_ms: NOW })
        .unwrap();
    assert!(matches!(
        chain,
        PrivacyStep::Rpc {
            purpose: PrivacyPurpose::ChainId,
            ..
        }
    ));
    let wrong = plan.advance(
        &host,
        PrivacyReply::Http {
            id: id_of(&chain),
            status: 200,
            body: vec![],
            now_ms: NOW,
        },
    );
    assert_eq!(wrong, Err(PrivacyError::OutOfOrder));
    assert_eq!(
        plan.advance(&host, chain_reply(id_of(&chain), NOW)),
        Err(PrivacyError::PlanClosed)
    );

    let mut plan = mainnet_plan();
    let chain = plan
        .advance(&host, PrivacyReply::Start { now_ms: NOW })
        .unwrap();
    assert_eq!(
        plan.advance(&host, chain_reply(id_of(&chain) + 1, NOW)),
        Err(PrivacyError::OutOfOrder)
    );

    let mut plan = mainnet_plan();
    plan.close();
    assert_eq!(
        plan.advance(&host, PrivacyReply::Start { now_ms: NOW }),
        Err(PrivacyError::PlanClosed)
    );
}

#[test]
fn a_wrong_chain_stops_before_any_post() {
    let host = Host::new();
    let mut plan = mainnet_plan();
    let chain = plan
        .advance(&host, PrivacyReply::Start { now_ms: NOW })
        .unwrap();
    let id = id_of(&chain);
    let step = plan
        .advance(
            &host,
            PrivacyReply::Rpc {
                id,
                body: format!(r#"{{"jsonrpc":"2.0","id":{id},"result":"0x534e5f5345504f4c4941"}}"#),
                now_ms: NOW,
            },
        )
        .unwrap();
    assert_eq!(
        step,
        PrivacyStep::Failed {
            failure: PrivacyFailure::WrongChain
        }
    );
    assert_eq!(host.derivations.get(), 0, "no key was asked for");
}

#[test]
fn a_lock_fails_the_next_derivation_even_after_a_new_unlock() {
    let host = Host::new();
    let mut plan = mainnet_plan();
    let chain = plan
        .advance(&host, PrivacyReply::Start { now_ms: NOW })
        .unwrap();
    let preflight = plan
        .advance(&host, chain_reply(id_of(&chain), NOW))
        .unwrap();
    assert!(matches!(
        preflight,
        PrivacyStep::Post {
            purpose: PrivacyPurpose::PreflightCheck,
            ..
        }
    ));
    // The test host's lock ends the plan's session; an unlock would start a new one.
    host.lock();
    let (status, body) = registered();
    let step = plan
        .advance(
            &host,
            PrivacyReply::Http {
                id: id_of(&preflight),
                status,
                body,
                now_ms: NOW,
            },
        )
        .unwrap();
    assert_eq!(
        step,
        PrivacyStep::Failed {
            failure: PrivacyFailure::Locked
        }
    );
    assert_eq!(
        host.derivations.get(),
        1,
        "only the preflight body had a key"
    );
}

#[test]
fn preparing_needs_plain_routing_for_the_network() {
    let routes = routes();
    assert!(route_for(&routes, NetworkId::Sepolia).is_none());
    let (_, limits) = routing();
    let address = felt::parse(&account_address()).unwrap();
    let mainnet_route = route_for(&routes, NetworkId::Mainnet).unwrap().clone();
    let ohttp = starknet_wallet_core::privacy::config::Route {
        ohttp: true,
        ..mainnet_route.clone()
    };
    assert!(matches!(
        PrivacyReadPlan::new(mainnet(), "a".into(), address, ohttp, limits.clone()),
        Err(PrivacyError::NotConfigured)
    ));
    let sepolia = network_info(portfolio_support::networks(), NetworkId::Sepolia).unwrap();
    assert!(matches!(
        PrivacyReadPlan::new(sepolia, "a".into(), address, mainnet_route, limits),
        Err(PrivacyError::NotConfigured)
    ));
}

#[test]
fn bramble_catalog_is_a_token_list_the_extension_would_accept() {
    let (_, limits) = routing();
    for network in [NetworkId::Mainnet, NetworkId::Sepolia] {
        let tokens = &network_info(portfolio_support::networks(), network)
            .unwrap()
            .tokens;
        let keys = requested_token_keys(tokens, &limits).expect("bundled tokens");
        assert_eq!(keys.len(), tokens.len());
        assert!(!tokens.is_empty());
    }
}

#[test]
fn the_discovery_budget_is_per_attempt_on_host_time() {
    // One page takes 61 s of host time: the attempt's 60 s budget has lapsed.
    let mut slow = |now: u64| now + 61_000;
    let run = run_mainnet(script(vec![complete_page("0x1")]), &mut slow);
    assert_eq!(
        run.outcome.clone().unwrap().unwrap_err(),
        PrivacyFailure::BudgetExceeded
    );

    // 50 s, a throttled reply, a pause, then 50 s again: each attempt stays within its own budget.
    let mut half = |now: u64| now + 50_000;
    let throttled = http(429, json!({ "error": "Too Many Requests" }));
    let run = run_mainnet(script(vec![throttled, complete_page("0x1")]), &mut half);
    assert!(run.outcome.clone().unwrap().is_ok());
    assert_eq!(run.waits, vec![1_000]);
}

#[test]
fn a_retry_restarts_discovery_from_the_first_page() {
    let throttled = http(503, json!({ "error": "rate limit" }));
    let run = run_mainnet(
        script(vec![
            incomplete_page("0x1"),
            throttled,
            complete_page("0x2"),
        ]),
        &mut steady,
    );
    assert!(run.outcome.clone().unwrap().is_ok());
    let incoming: Vec<_> = run
        .posts
        .iter()
        .filter(|(_, _, p)| *p == PrivacyPurpose::IncomingState)
        .collect();
    assert_eq!(incoming.len(), 3);
    // The retried attempt starts over: its body is the first page's, with an empty cursor.
    assert_eq!(incoming[2].1, incoming[0].1);
    assert!(std::str::from_utf8(&incoming[2].1)
        .unwrap()
        .ends_with(r#""cursor":{}}"#));
    assert_eq!(run.waits, vec![1_000]);
}

#[test]
fn failures_keep_their_classification() {
    let cases = [
        (
            http(409, json!({ "error": "BLOCK_REORGED" })),
            PrivacyFailure::Reorg,
        ),
        ((429, Vec::new()), PrivacyFailure::ServiceUnavailable),
        (
            (500, b"internal".to_vec()),
            PrivacyFailure::ServiceUnavailable,
        ),
        (
            (200, b"{not json".to_vec()),
            PrivacyFailure::InvalidResponse,
        ),
        (
            http(
                200,
                json!({ "block_ref": "pending", "channels": [], "notes": [], "cursor": {} }),
            ),
            PrivacyFailure::InvalidResponse,
        ),
        (
            (200, vec![b' '; 2 * 1024 * 1024 + 1]),
            PrivacyFailure::InvalidResponse,
        ),
    ];
    for ((status, body), failure) in cases {
        let run = run_mainnet(script(vec![(status, body)]), &mut steady);
        assert_eq!(
            run.outcome.clone().unwrap().unwrap_err(),
            failure,
            "{status}"
        );
    }
    let throttled = || http(429, json!({ "error": "rate limit" }));
    let run = run_mainnet(
        script(vec![throttled(), throttled(), throttled()]),
        &mut steady,
    );
    assert_eq!(
        run.outcome.clone().unwrap().unwrap_err(),
        PrivacyFailure::RateLimited
    );
    assert_eq!(run.waits, vec![1_000, 2_000]);
}

#[test]
fn a_changed_block_ref_is_refused() {
    let run = run_mainnet(
        script(vec![incomplete_page("0x1"), complete_page("0x2")]),
        &mut steady,
    );
    assert_eq!(
        run.outcome.clone().unwrap().unwrap_err(),
        PrivacyFailure::InvalidResponse
    );
    let run = run_mainnet(
        script(vec![incomplete_page("0x1"), complete_page("0x1")]),
        &mut steady,
    );
    assert!(run.outcome.clone().unwrap().is_ok());
    // The pinned block is echoed on the later page.
    let later = &run
        .posts
        .iter()
        .filter(|(_, _, p)| *p == PrivacyPurpose::IncomingState)
        .nth(1)
        .unwrap()
        .1;
    assert!(std::str::from_utf8(later)
        .unwrap()
        .ends_with(r#""block_ref":"0x1"}"#));
}

#[test]
fn pagination_stops_at_the_page_cap() {
    let pages = (0..300).map(|_| incomplete_page("0x1")).collect();
    let run = run_mainnet(script(pages), &mut steady);
    assert_eq!(
        run.outcome.clone().unwrap().unwrap_err(),
        PrivacyFailure::InvalidResponse
    );
    assert_eq!(run.count(PrivacyPurpose::IncomingState), 256);
}

#[test]
fn an_unregistered_account_reads_no_notes() {
    let script = Script {
        preflight: vec![http(
            200,
            json!({ "block_ref": "0x1", "sender_registered": false }),
        )]
        .into(),
        pages: Default::default(),
        fee: vec![fee()].into(),
    };
    let run = run_mainnet(script, &mut steady);
    let state = run.outcome.clone().unwrap().unwrap();
    assert!(state.balances.is_empty());
    assert_eq!(state.pool_fee_fri, "0x5");
    assert_eq!(run.count(PrivacyPurpose::IncomingState), 0);
}

fn forms(key: &str) -> [String; 3] {
    let digits = key.trim_start_matches("0x");
    [
        key.to_string(),
        format!("{digits:0>64}"),
        digits.to_string(),
    ]
}

/// The core's half of Bramble mobile's `privacy_key_boundary.rs`: the viewing key leaves the plan
/// only inside a discovery `Post` body, never in any step's `Debug`, a state, or a failure.
#[test]
fn the_key_appears_only_inside_discovery_bodies() {
    let wire = wire();
    let keys = &wire["viewingKeys"];
    let account = account_key(PHRASE, None, Ladder::Account, 0).unwrap();
    let key = derive_strk20_viewing_key(
        &account,
        keys["chainId"].as_str().unwrap(),
        keys["pool"].as_str().unwrap(),
    )
    .unwrap()
    .expose_secret_hex();
    let mut debug = String::new();
    let mut in_bodies = 0;
    let host = Host::new();
    for case in wire["cases"].as_array().unwrap() {
        let run = drive(
            &mut mainnet_plan(),
            &host,
            Script::from_case(&case["script"]),
            &mut steady,
        );
        for step in &run.steps {
            debug.push_str(&format!("{step:?}\n"));
            if let PrivacyStep::Post { body, purpose, .. } = step {
                assert!(matches!(
                    purpose,
                    PrivacyPurpose::PreflightCheck | PrivacyPurpose::IncomingState
                ));
                if std::str::from_utf8(body).unwrap().contains(key.as_str()) {
                    in_bodies += 1;
                }
            }
        }
        debug.push_str(&format!("{:?}\n", run.outcome));
    }
    assert!(in_bodies > 0, "the discovery bodies carry the key");
    assert!(
        debug.contains("[redacted"),
        "Post bodies are redacted in Debug"
    );
    for form in forms(&key) {
        assert!(!debug.contains(&form), "the key leaked into Debug output");
    }
}
