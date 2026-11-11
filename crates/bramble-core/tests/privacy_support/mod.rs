//! Shared helpers for the privacy tests: a test `PrivacyHost` on the published parity phrase that
//! can simulate a lock, Bramble's privacy routing (`fixtures/privacy.json`) and network catalog
//! read into the core's records, the committed privacy fixtures, and a scripted service that
//! answers a read plan. Nothing here touches a network; every key is derived from the public
//! parity phrase.
#![allow(dead_code)] // Each privacy suite reads a different subset.

use std::cell::Cell;
use std::collections::VecDeque;

use serde_json::{json, Value};
use starknet_wallet_core::accounts::Ladder;
use starknet_wallet_core::catalog::{network_info, NetworkInfo};
use starknet_wallet_core::privacy::config::{Limits, Route};
use starknet_wallet_core::privacy::plan::{PrivacyHost, PrivacyReadPlan};
use starknet_wallet_core::privacy::types::{
    PrivacyFailure, PrivacyPurpose, PrivacyReply, PrivacyState, PrivacyStep,
};
use starknet_wallet_core::{account_key, derive_strk20_viewing_key, felt, NetworkId, SecretFelt};

use crate::portfolio_support::networks;

pub const PHRASE: &str =
    "habit hope tip crystal because grunt nation idea electric witness alert like";
pub const NOW: u64 = 1_790_000_000_000;
pub const MAINNET_CHAIN: &str = "0x534e5f4d41494e";
const SENDER: &str = "0x3fa78c2861131190760b151f3ee110f473940eca3fec8cc423ecc307b5241a3";
const ETH: &str = "0x49d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7";
const STRK: &str = "0x4718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d";

fn decode_rows(value: &mut Value, keys: &[&str]) {
    for key in keys {
        let rows: Vec<Value> = value[*key]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| serde_json::from_str(row.as_str().unwrap()).unwrap())
            .collect();
        value[*key] = Value::Array(rows);
    }
}

/// `extension-privacy-wire.json` with its row strings decoded.
pub fn wire() -> Value {
    let mut value: Value =
        serde_json::from_str(include_str!("../fixtures/extension-privacy-wire.json")).unwrap();
    decode_rows(&mut value, &["capture", "cases"]);
    value
}

/// `extension-privacy-rules.json` with its row strings decoded.
pub fn rules() -> Value {
    let mut value: Value =
        serde_json::from_str(include_str!("../fixtures/extension-privacy-rules.json")).unwrap();
    decode_rows(
        &mut value,
        &["compactAmounts", "journeys", "static", "unified"],
    );
    value
}

/// Bramble's routing file as committed (`generated/privacy.json`).
pub const ROUTING_JSON: &str = include_str!("../fixtures/privacy.json");

/// Reads a routing file into the core's records as written: every network entry that names a
/// discovery URL and pool becomes a `Route`, OHTTP flag included. A reader only; Bramble's
/// loader (its validation and debug override) stays in Bramble.
pub fn read_routing(text: &str) -> (Vec<Route>, Limits) {
    let file: Value = serde_json::from_str(text).unwrap();
    let limits = &file["limits"];
    let count = |key: &str| usize::try_from(limits[key].as_u64().unwrap()).unwrap();
    let max_pool_fee = limits["maxPoolFee"].as_str().unwrap();
    let limits = Limits {
        balances: count("balances"),
        discovered_notes: count("discoveredNotes"),
        max_pool_fee: u128::from_str_radix(max_pool_fee.strip_prefix("0x").unwrap(), 16).unwrap(),
        max_service_attempts: u32::try_from(limits["maxServiceAttempts"].as_u64().unwrap())
            .unwrap(),
        tokens: count("tokens"),
        discovery_budget_ms: file["waitBudgets"]["discoveryMs"].as_u64().unwrap(),
    };
    let routes = file["networks"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|entry| {
            let pool_address = entry.get("poolAddress")?.as_str()?.to_string();
            Some(Route {
                network: NetworkId::from_catalog_id(entry["id"].as_str()?)?,
                ohttp: entry["ohttp"].as_bool()?,
                discovery_url: entry.get("discoveryUrl")?.as_str()?.to_string(),
                pool: felt::parse_non_zero(&pool_address)?,
                pool_address,
            })
        })
        .collect();
    (routes, limits)
}

/// Bramble's routes and limits.
pub fn routing() -> (Vec<Route>, Limits) {
    read_routing(ROUTING_JSON)
}

pub fn routes() -> Vec<Route> {
    routing().0
}

/// The wallet behind a plan: the parity phrase's account 0 key, and a session generation the
/// test can end. A lock bumps the generation, so even a new unlock leaves the plan's session over.
pub struct Host {
    key: SecretFelt,
    prepared: u64,
    generation: Cell<u64>,
    /// How many viewing keys the plan asked for.
    pub derivations: Cell<u32>,
}

impl Host {
    pub fn new() -> Self {
        Self {
            key: account_key(PHRASE, None, Ladder::Account, 0).unwrap(),
            prepared: 0,
            generation: Cell::new(0),
            derivations: Cell::new(0),
        }
    }

    /// Locks (and, for the plan, every later unlock is a new session).
    pub fn lock(&self) {
        self.generation.set(self.generation.get() + 1);
    }
}

impl PrivacyHost for Host {
    fn viewing_key(&self, chain_id: &str, pool_address: &str) -> Option<SecretFelt> {
        if self.generation.get() != self.prepared {
            return None;
        }
        self.derivations.set(self.derivations.get() + 1);
        derive_strk20_viewing_key(&self.key, chain_id, pool_address).ok()
    }
}

/// The parity phrase's OpenZeppelin account 0 on Mainnet, as the extension recorded it.
pub fn account_address() -> String {
    wire()["viewingKeys"]["accounts"][0]["address"]
        .as_str()
        .unwrap()
        .to_string()
}

pub fn mainnet() -> &'static NetworkInfo {
    network_info(networks(), NetworkId::Mainnet).unwrap()
}

/// A read plan for account 0 on Mainnet over Bramble's catalog and routing.
pub fn mainnet_plan() -> PrivacyReadPlan {
    let (routes, limits) = routing();
    let address = account_address();
    let route = routes
        .into_iter()
        .find(|route| route.network == NetworkId::Mainnet)
        .unwrap();
    PrivacyReadPlan::new(
        mainnet(),
        format!("SN_MAIN:{address}"),
        felt::parse(&address).unwrap(),
        route,
        limits,
    )
    .unwrap()
}

/// `synthPages` from `scripts/mobile-privacy-cases.ts`, expanded identically.
pub fn synth_pages(spec: &Value) -> Vec<Value> {
    let tokens = spec["tokens"].as_u64().unwrap();
    let per = spec["notesPerToken"].as_u64().unwrap();
    let amount = spec["amount"].as_str().unwrap();
    let mut notes = Vec::new();
    for t in 0..tokens {
        let token = match t {
            0 => ETH.to_string(),
            1 => STRK.to_string(),
            n => format!("0x{:x}", 0x1000 + n),
        };
        for n in 0..per {
            notes.push(json!({
                "amount": amount, "block_number": 7, "index": n,
                "note_id": format!("0x{:x}", n + 1), "salt": "12345",
                "sender_addr": SENDER, "token": token,
            }));
        }
    }
    let body = json!({
        "block_ref": "0x5a1f00d",
        "channels": [{ "channel_key": "0x77", "sender_addr": SENDER }],
        "cursor": { "channel_discovery_complete": true },
        "notes": notes,
        "subchannels": [],
    });
    vec![json!({ "status": 200, "body": body.to_string() })]
}

/// Scripted replies for one plan, in the fixture's shape.
#[derive(Default)]
pub struct Script {
    pub preflight: VecDeque<(u16, Vec<u8>)>,
    pub pages: VecDeque<(u16, Vec<u8>)>,
    pub fee: VecDeque<String>,
}

fn replies(value: &Value) -> VecDeque<(u16, Vec<u8>)> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["status"].as_u64().unwrap() as u16,
                r["body"].as_str().unwrap().as_bytes().to_vec(),
            )
        })
        .collect()
}

impl Script {
    pub fn from_case(script: &Value) -> Self {
        let pages = match script.get("synth") {
            Some(spec) => Value::Array(synth_pages(spec)),
            None => script["pages"].clone(),
        };
        Self {
            preflight: replies(&script["preflight"]),
            pages: replies(&pages),
            fee: script["fee"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["body"].as_str().unwrap().to_string())
                .collect(),
        }
    }
}

/// Everything a scripted run observed.
#[derive(Debug, Default)]
pub struct Run {
    pub outcome: Option<Result<PrivacyState, PrivacyFailure>>,
    pub posts: Vec<(String, Vec<u8>, PrivacyPurpose)>,
    pub rpcs: Vec<(String, PrivacyPurpose)>,
    pub waits: Vec<u32>,
    pub steps: Vec<PrivacyStep>,
}

impl Run {
    pub fn count(&self, purpose: PrivacyPurpose) -> usize {
        self.posts.iter().filter(|(_, _, p)| *p == purpose).count()
            + self.rpcs.iter().filter(|(_, p)| *p == purpose).count()
    }
}

/// Drives `plan` to a terminal step, answering from `script`; the host clock advances 10 ms per
/// reply unless `clock` says otherwise.
pub fn drive(
    plan: &mut PrivacyReadPlan,
    host: &Host,
    mut script: Script,
    clock: &mut dyn FnMut(u64) -> u64,
) -> Run {
    let mut run = Run::default();
    let mut now = NOW;
    let mut reply = PrivacyReply::Start { now_ms: now };
    loop {
        let step = plan
            .advance(host, reply)
            .expect("plan accepts the scripted reply");
        run.steps.push(step.clone());
        now = clock(now);
        reply = match step {
            PrivacyStep::Post {
                id,
                url,
                body,
                purpose,
            } => {
                run.posts.push((url, body.to_vec(), purpose));
                let queue = if purpose == PrivacyPurpose::PreflightCheck {
                    &mut script.preflight
                } else {
                    &mut script.pages
                };
                match queue.pop_front() {
                    Some((status, body)) => PrivacyReply::Http {
                        id,
                        status,
                        body,
                        now_ms: now,
                    },
                    None => PrivacyReply::Http {
                        id,
                        status: 599,
                        body: b"exhausted".to_vec(),
                        now_ms: now,
                    },
                }
            }
            PrivacyStep::Rpc { id, body, purpose } => {
                run.rpcs.push((body, purpose));
                let text = match purpose {
                    PrivacyPurpose::ChainId => {
                        format!(r#"{{"jsonrpc":"2.0","id":{id},"result":"{MAINNET_CHAIN}"}}"#)
                    }
                    _ => script
                        .fee
                        .pop_front()
                        .unwrap_or_default()
                        .replace("\"id\":0", &format!("\"id\":{id}")),
                };
                PrivacyReply::Rpc {
                    id,
                    body: text,
                    now_ms: now,
                }
            }
            PrivacyStep::Wait { delay_ms } => {
                run.waits.push(delay_ms);
                PrivacyReply::WaitElapsed { now_ms: now }
            }
            PrivacyStep::Finished { state } => {
                run.outcome = Some(Ok(state));
                return run;
            }
            PrivacyStep::Failed { failure } => {
                run.outcome = Some(Err(failure));
                return run;
            }
        };
    }
}

pub fn steady(now: u64) -> u64 {
    now + 10
}

/// Drives a fresh Mainnet plan over a fresh, unlocked host.
pub fn run_mainnet(script: Script, clock: &mut dyn FnMut(u64) -> u64) -> Run {
    drive(&mut mainnet_plan(), &Host::new(), script, clock)
}
