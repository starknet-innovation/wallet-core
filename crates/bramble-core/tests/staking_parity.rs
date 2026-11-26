//! Native staking (P13) against what the extension's own staking background produced
//! (`fixtures/extension-staking.jsonl`, `bun run mobile:parity`): Starkscan's coverage rule,
//! directory and positions as requests and judges, the decoders and `verify_staking_pool`'s
//! verdicts, the pinned chain read put on the same wire (every JSON-RPC body the extension sent,
//! answered as its node answered), the rows the screens show, and each action's calls, review,
//! activity text and allowance, under Bramble's `STAKING_POLICY`.
//!
//! The fixture's chain is the extension's test chain, whose staking contract is `0x5a1`, so the
//! policy here is `STAKING_POLICY` with that contract on Mainnet; every other number is Bramble's.

use crate::staking_support;

use bramble_core::staking::STAKING_POLICY;
use serde_json::{json, Value};
use staking_support::*;
use starknet_wallet_core::js::json::parse;
use starknet_wallet_core::portfolio::wire::HttpOutcome;
use starknet_wallet_core::staking::coverage::{staking_coverage, Coverage};
use starknet_wallet_core::staking::decode::*;
use starknet_wallet_core::staking::directory::{directory_request, directory_view, DirectoryView};
use starknet_wallet_core::staking::positions::{positions_page, positions_request};
use starknet_wallet_core::staking::read::StakingRead;
use starknet_wallet_core::staking::view::{
    overview, overview_pairs, positions_pairs, positions_view,
};
use starknet_wallet_core::staking::{staker_address, StakingRefusal};
use starknet_wallet_core::{felt, Felt, NetworkId};

const MAINNET: NetworkId = NetworkId::Mainnet;

fn error(refusal: StakingRefusal) -> Value {
    json!({ "error": refusal.to_string() })
}

#[test]
fn the_context_is_the_one_the_policy_reads() {
    let context = &cases("context")[0];
    assert_eq!(context["stakingContract"], CONTRACT);
    assert_eq!(context["now"], NOW);
    assert_eq!(context["starkscanBase"], BASE);
    assert_eq!(context["account"], felt::canonical(&account().address));
    assert!(!tokens().is_empty());
}

#[test]
fn coverage_decides_as_the_extension_does() {
    for case in cases("coverage") {
        let value = parse(&case["coverage"].to_string()).unwrap();
        let ours = staking_coverage(Some(&value), STAKING_POLICY.max_catching_up_lag_blocks);
        let ours = match ours {
            Some(Coverage::Usable) => json!("usable"),
            Some(Coverage::Incomplete) => json!("incomplete"),
            None => Value::Null,
        };
        assert_eq!(ours, case["result"], "{}", case["name"]);
    }
}

fn hex(value: &Felt) -> Value {
    json!(felt::canonical(value))
}

fn values(case: &Value) -> Vec<String> {
    serde_json::from_value(case["values"].clone()).unwrap()
}

fn pool_info_json(info: &StakerPoolInfo) -> Value {
    let pools: Vec<Value> = info
        .pools
        .iter()
        .map(|p| json!({ "amount": p.amount.to_string(), "pool": hex(&p.pool), "token": hex(&p.token) }))
        .collect();
    json!({ "commission": info.commission, "pools": pools })
}

fn status_json(status: StakerStatus) -> Value {
    json!(match status {
        StakerStatus::Active => "active",
        StakerStatus::Leaving => "leaving",
        StakerStatus::Unknown => "unknown",
    })
}

#[test]
fn every_decoder_reads_a_reply_as_the_extension_does() {
    for case in cases("decode") {
        let values = values(&case);
        let ours = match case["decoder"].as_str().unwrap() {
            "stakerPoolInfo" => decode_staker_pool_info(&values).map(|i| pool_info_json(&i)),
            "stakerStatus" => decode_staker_status(&values).map(status_json),
            "poolParameters" => decode_pool_parameters(&values).map(|p| {
                json!({
                    "commission": p.commission,
                    "staker": hex(&p.staker),
                    "stakerRemoved": p.staker_removed,
                    "stakingContract": hex(&p.staking_contract),
                    "token": hex(&p.token),
                })
            }),
            "poolMember" => decode_pool_member(&values).map(|member| match member {
                None => json!("none"),
                Some(m) => json!({
                    "amount": m.amount.to_string(),
                    "rewardAddress": hex(&m.reward_address),
                    "unclaimedRewards": m.unclaimed_rewards.to_string(),
                    "unpoolAmount": m.unpool_amount.to_string(),
                    "unpoolTime": m.unpool_time,
                }),
            }),
            "activeTokens" => {
                decode_active_tokens(&values).map(|t| json!(t.iter().map(hex).collect::<Vec<_>>()))
            }
            "exitWindow" => decode_exit_window(&values).map(|s| json!(s)),
            other => panic!("unknown decoder {other}"),
        };
        assert_eq!(
            ours.unwrap_or(Value::Null),
            case["result"],
            "{}",
            case["name"]
        );
    }
}

#[test]
fn a_pool_is_verified_or_refused_as_the_extension_does() {
    for case in cases("verify") {
        let raw = |key: &str| -> Option<Vec<String>> {
            serde_json::from_value(case["reads"][key].clone()).ok()
        };
        let reads = StakingPoolReads {
            parameters: raw("parameters").and_then(|v| decode_pool_parameters(&v)),
            pool_info: decode_staker_pool_info(&raw("poolInfo").unwrap()),
            status: decode_staker_status(&raw("status").unwrap()),
        };
        let expected = &case["expected"];
        let address = |key: &str| felt::parse(expected[key].as_str().unwrap()).unwrap();
        let ours = match verify_staking_pool(
            &reads,
            &address("staker"),
            &address("stakingContract"),
            &address("token"),
        ) {
            Ok(pool) => json!({
                "accepting": pool.accepting(),
                "commission": pool.commission(),
                "delegated": pool.delegated().to_string(),
                "pool": hex(&pool.pool()),
            }),
            Err(refusal) => json!({ "refusal": refusal.to_string() }),
        };
        assert_eq!(ours, case["verdict"], "{}", case["name"]);
    }
}

#[test]
fn the_overview_reads_and_shows_what_the_extension_does() {
    let tokens = tokens();
    for case in cases("overview") {
        let name = case["name"].as_str().unwrap();
        let policy = policy(case["enabled"].as_bool().unwrap());
        let Some(network) = policy.network(MAINNET) else {
            assert!(case["rpc"].as_array().unwrap().is_empty(), "{name}");
            assert_eq!(
                case["response"],
                error(StakingRefusal::Unavailable),
                "{name}"
            );
            continue;
        };
        let staker = staker_address(case["staker"].as_str().unwrap()).unwrap();
        let account = account().address;
        let pairs = overview_pairs(&staker, &tokens, MAINNET);
        let mut read = StakingRead::new(network, &rewards(), account, pairs).unwrap();
        let reading = run_read(&mut read, &case["rpc"], name);
        let ours = match overview(reading.as_ref(), &tokens, &account, MAINNET, now(&case)) {
            Ok(view) => json!({
                "ok": true,
                "staking": {
                    "exitWindowSeconds": view.exit_window_seconds,
                    "rows": view.rows.iter().map(row_json).collect::<Vec<_>>(),
                },
            }),
            Err(refusal) => error(refusal),
        };
        assert_eq!(ours, case["response"], "{name}");
    }
}

fn outcome(exchange: &Value) -> HttpOutcome {
    let reply = &exchange["reply"];
    HttpOutcome {
        status: reply["status"].as_u64().unwrap() as u16,
        retry_after: None,
        body: Some(reply["body"].as_str().unwrap().as_bytes().to_vec()),
        timed_out: false,
    }
}

/// The request the extension's Starkscan client sent, against the one the core asks the host to.
fn same_request(
    ours: &starknet_wallet_core::portfolio::wire::HttpRequest,
    sent: &Value,
    name: &str,
) {
    let request = &sent["request"];
    assert_eq!(ours.url, request["url"].as_str().unwrap(), "{name}");
    assert_eq!(
        format!("{:?}", ours.method).to_uppercase(),
        request["method"],
        "{name}"
    );
    let accept = ours
        .headers
        .iter()
        .find(|h| h.name == "accept")
        .map(|h| h.value.as_str());
    assert_eq!(accept, request["accept"].as_str(), "{name}");
}

#[test]
fn a_directory_page_is_asked_for_and_judged_as_the_extension_does() {
    let tokens = tokens();
    for case in cases("directory") {
        let name = case["name"].as_str().unwrap();
        let policy = policy(case["enabled"].as_bool().unwrap());
        let sent = case["starkscan"].as_array().unwrap();
        let request = match directory_request(MAINNET, &policy, BASE, case["cursor"].as_str()) {
            Err(refusal) => {
                assert!(sent.is_empty(), "{name}");
                assert_eq!(case["response"], error(refusal), "{name}");
                continue;
            }
            Ok(request) => request.expect("the directory is read on Mainnet"),
        };
        let [exchange] = &sent[..] else {
            panic!("{name}: one request expected");
        };
        same_request(&request, exchange, name);
        let ours = match directory_view(&outcome(exchange), MAINNET, &tokens, &policy) {
            Ok(DirectoryView::Incomplete) => {
                json!({ "ok": true, "stakingDirectory": { "state": "incomplete" } })
            }
            Ok(DirectoryView::Ready {
                entries,
                next_cursor,
            }) => json!({
                "ok": true,
                "stakingDirectory": {
                    "entries": entries.iter().map(entry_json).collect::<Vec<_>>(),
                    "nextCursor": next_cursor,
                    "state": "ready",
                },
            }),
            Err(refusal) => error(refusal),
        };
        assert_eq!(ours, case["response"], "{name}");
    }
}

#[test]
fn your_stakes_are_asked_for_read_and_shown_as_the_extension_does() {
    let tokens = tokens();
    let account = account().address;
    for case in cases("positions") {
        let name = case["name"].as_str().unwrap();
        let policy = policy(case["enabled"].as_bool().unwrap());
        let sent = case["starkscan"].as_array().unwrap();
        let explorer = case["explorerFallback"].as_bool().unwrap();
        let request = match positions_request(MAINNET, &policy, BASE, &account, explorer) {
            Err(refusal) => {
                assert!(sent.is_empty(), "{name}");
                assert_eq!(case["response"], error(refusal), "{name}");
                continue;
            }
            Ok(None) => {
                assert!(sent.is_empty(), "{name}");
                let off = json!({ "ok": true, "stakingPositions": { "state": "off" } });
                assert_eq!(case["response"], off, "{name}");
                continue;
            }
            Ok(Some(request)) => request,
        };
        let [exchange] = &sent[..] else {
            panic!("{name}: one request expected");
        };
        same_request(&request, exchange, name);
        let ours =
            positions_page(&outcome(exchange), MAINNET, &account, &policy).and_then(|page| {
                let (pairs, complete) = positions_pairs(&page, &tokens, MAINNET, &policy);
                let network = policy.network(MAINNET).unwrap();
                let mut read = StakingRead::new(network, &rewards(), account, pairs).unwrap();
                let reading = run_read(&mut read, &case["rpc"], name);
                positions_view(
                    reading.as_ref(),
                    &tokens,
                    &account,
                    MAINNET,
                    now(&case),
                    complete,
                )
            });
        let ours = match ours {
            Ok(view) => json!({
                "ok": true,
                "stakingPositions": {
                    "complete": view.complete,
                    "rows": view.rows.iter().map(row_json).collect::<Vec<_>>(),
                    "state": "ready",
                },
            }),
            Err(refusal) => error(refusal),
        };
        assert_eq!(ours, case["response"], "{name}");
    }
}
