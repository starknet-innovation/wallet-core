//! Shared by the staking parity suites: `extension-staking.jsonl`, its context, Bramble's policy
//! pointed at the fixture chain's staking contract, the pinned read driven over the recorded
//! node, and the extension's JSON shapes for a row and a directory entry.
#![allow(dead_code)] // Each staking suite reads a different subset.

use bramble_core::catalogs::networks;
use bramble_core::staking::STAKING_POLICY;
use serde_json::{json, Value};
use starknet_wallet_core::catalog::TokenInfo;
use starknet_wallet_core::history::events::ScanStep;
use starknet_wallet_core::portfolio::model::TokenConfig;
use starknet_wallet_core::portfolio::wire::RpcReply;
use starknet_wallet_core::send::types::{Account, Authorization};
use starknet_wallet_core::staking::calls::PositionState;
use starknet_wallet_core::staking::directory::DirectoryEntry;
use starknet_wallet_core::staking::read::{StakingRead, StakingReading};
use starknet_wallet_core::staking::view::StakingRow;
use starknet_wallet_core::staking::{StakingNetwork, StakingPolicy};
use starknet_wallet_core::{felt, Felt, NetworkId};

/// The fixture chain's staking contract (`tests/support/staking-chain.ts`).
pub const CONTRACT: &str = "0x5a1";
/// The context's clock, in Unix seconds; each case carries its own (`now`).
pub const NOW: u64 = 1_800_000_000;

/// The clock the extension ran `case` at.
pub fn now(case: &Value) -> u64 {
    case["now"].as_u64().unwrap()
}

/// The explorer lane's base every Starkscan request was made against.
pub const BASE: &str = "https://starkscan.test/v1";

/// Every case of `block`, in file order.
pub fn cases(block: &str) -> Vec<Value> {
    let found: Vec<Value> = include_str!("../fixtures/extension-staking.jsonl")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("a JSON line"))
        .filter(|case| case["block"] == block)
        .collect();
    assert!(!found.is_empty(), "no {block} cases");
    found
}

fn context() -> Value {
    cases("context").remove(0)
}

const ON: &[StakingNetwork] = &[StakingNetwork {
    network: NetworkId::Mainnet,
    enabled: true,
    staking_contract: CONTRACT,
    starkscan: true,
}];
const OFF: &[StakingNetwork] = &[StakingNetwork {
    enabled: false,
    ..ON[0]
}];

/// `STAKING_POLICY` with the fixture chain's contract on Mainnet, switched on or off.
pub fn policy(enabled: bool) -> StakingPolicy {
    StakingPolicy {
        networks: if enabled { ON } else { OFF },
        ..STAKING_POLICY
    }
}

/// The acting account (the extension's `invokeAccount`, Mainnet). The key is never read.
pub fn account() -> Account {
    Account {
        network: NetworkId::Mainnet,
        address: felt::parse("0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcde")
            .unwrap(),
        public_key: Felt::from(3u8),
        authorization: Authorization::StarkPair,
        deployment: None,
    }
}

/// The Mainnet tokens the extension's default preferences track, as the fixture lists them.
pub fn tokens() -> Vec<TokenConfig> {
    context()["tokens"]
        .as_array()
        .unwrap()
        .iter()
        .map(|token| {
            assert_eq!(token["networkId"], "SN_MAIN");
            TokenConfig {
                id: token["id"].as_str().unwrap().into(),
                network: NetworkId::Mainnet,
                address: token["address"].as_str().unwrap().into(),
                symbol: token["symbol"].as_str().unwrap().into(),
                name: token["name"].as_str().unwrap().into(),
                decimals: token["decimals"].as_u64().unwrap() as u8,
            }
        })
        .collect()
}

/// The rewards token a read checks: the Mainnet catalog's STRK.
pub fn rewards() -> TokenInfo {
    networks::info(NetworkId::Mainnet)
        .tokens
        .iter()
        .find(|token| token.symbol == "STRK")
        .unwrap()
        .clone()
}

// starknet.js sends `starknet_blockHashAndNumber` with no `params`; the core sends `[]`. Both
// are the same call.
fn same_params(recorded: &Value, sent: &Value) -> bool {
    recorded == sent || (recorded.is_null() && *sent == json!([]))
}

/// Drives `read` over the node the extension spoke to: every body the core sends must be one the
/// extension sent, answered with that recorded reply (each recorded exchange answers once; of
/// identical bodies, the earliest unused answers first), and the core must send every one the
/// extension did. Order across different bodies is not compared: the reads are independent, and
/// the core batches them as its pinned read does, not as the extension's per-token reads did.
pub fn run_read(read: &mut StakingRead, rpc: &Value, name: &str) -> Option<StakingReading> {
    let recorded = rpc.as_array().unwrap();
    let mut used = vec![false; recorded.len()];
    loop {
        let calls = read.next_calls();
        if calls.is_empty() {
            break;
        }
        for call in calls {
            let body: Value = serde_json::from_str(&call.body).unwrap();
            let index = (0..recorded.len())
                .find(|&i| {
                    !used[i]
                        && recorded[i]["method"] == body["method"]
                        && same_params(&recorded[i]["params"], &body["params"])
                })
                .unwrap_or_else(|| panic!("{name}: the extension never sent {body}"));
            used[index] = true;
            let mut reply = json!({ "jsonrpc": "2.0", "id": call.id });
            for (key, value) in recorded[index]["reply"].as_object().unwrap() {
                reply[key] = value.clone();
            }
            let step = read.accept(RpcReply {
                id: call.id,
                status: 200,
                body: Some(reply.to_string()),
            });
            assert_eq!(step, ScanStep::Continue, "{name}");
        }
    }
    // The one departure (PLAN P13): the extension reads a staker once per token, the core once
    // per pinned read. An exchange the core skipped must repeat, body and reply, one it sent.
    let missed: Vec<&Value> = recorded
        .iter()
        .zip(&used)
        .filter(|(exchange, skipped)| {
            !**skipped
                && !recorded
                    .iter()
                    .zip(&used)
                    .any(|(sent, was)| *was && sent == *exchange)
        })
        .map(|(exchange, _)| exchange)
        .collect();
    assert!(missed.is_empty(), "{name}: the core never sent {missed:?}");
    read.finish()
}

fn state(state: PositionState) -> &'static str {
    match state {
        PositionState::NotStaked => "none",
        PositionState::Staked => "staked",
        PositionState::ExitPending => "exit-pending",
        PositionState::Withdrawable => "withdrawable",
    }
}

/// A row as the extension's `StakingRowView` spells it. Rewards are labelled with the read's
/// rewards token, which the read confirmed is STRK.
pub fn row_json(row: &StakingRow) -> Value {
    let mut out = json!({ "staker": row.staker, "symbol": row.symbol, "tokenId": row.token_id });
    match &row.figures {
        Err(refusal) => out["refusal"] = json!(refusal.to_string()),
        Ok(figures) => {
            out["accepting"] = json!(figures.accepting);
            out["commission"] = json!(figures.commission);
            out["delegated"] = json!(figures.delegated);
            if let Some(to) = &figures.rewards_to {
                out["rewardsTo"] = json!(to);
            }
            out["staked"] = json!(figures.staked);
            out["state"] = json!(state(figures.state));
            out["rewardSymbol"] = json!(rewards().symbol);
            out["unclaimed"] = json!(figures.unclaimed);
            out["unpoolAmount"] = json!(figures.unpool_amount);
            if let Some(time) = figures.unpool_time {
                out["unpoolTime"] = json!(time);
            }
        }
    }
    out
}

/// A directory entry as the extension's `StakingDirectoryEntry` spells it.
pub fn entry_json(entry: &DirectoryEntry) -> Value {
    let pools: Vec<Value> = entry
        .pools
        .iter()
        .map(|pool| json!({ "delegated": pool.delegated, "symbol": pool.symbol }))
        .collect();
    json!({
        "commission": entry.commission,
        "name": entry.name,
        "pools": pools,
        "staker": entry.staker,
        "uptime": entry.uptime,
        "website": entry.website,
    })
}

/// The export suite's half (`tests/staking.rs`, mobile's build): a canned Mainnet chain behind
/// the real staking contract, and a wallet that prepares, plans and signs through `WalletCore`.
#[cfg(feature = "testing")]
pub mod export;
