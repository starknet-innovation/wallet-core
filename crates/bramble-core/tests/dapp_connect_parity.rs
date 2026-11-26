//! P16's `connect`, `grants` and `queue` blocks (`fixtures/extension-dapp.jsonl`):
//!
//! - `connect`: `dapp::connect` over the wallet state each case fixes answers every page request
//!   as the extension's background did, and a connect card's decision grants, refuses or answers
//!   the waiting requests as its `applyConnect` and deny did;
//! - `grants`: `dapp::grants` over the vault's grants, step by step, writes and reads what the
//!   extension's vault service wrote and read;
//! - `queue`: `ApprovalQueue` under `DAPP_QUEUE_LIMITS` queues, joins, refuses, expires and
//!   evicts as the extension's queue and signature waiters did, at the same clock.

use crate::dapp_parity_support;

use std::collections::HashMap;

use bramble_core::dapp::{
    read_page_request, PageRequest, DAPP_QUEUE_LIMITS, SUPPORTED_SPECS, SUPPORTED_WALLET_API,
};
use dapp_parity_support::*;
use serde_json::{json, Value};
use starknet_wallet_core::dapp::connect::{
    accounts_view, addresses, answer, approve_connect, deny_connect, request_accounts, Connection,
};
use starknet_wallet_core::dapp::grants::{
    connected_sites, granted_account, with_origin_grant, without_origin_access,
};
use starknet_wallet_core::dapp::queue::{Refusal, Reservation};
use starknet_wallet_core::dapp::{
    Answer, ApprovalId, ApprovalQueue, DappAccount, OriginGrant, Site, Unlocked, Versions,
};

const VERSIONS: Versions = Versions {
    specs: SUPPORTED_SPECS,
    wallet_api: SUPPORTED_WALLET_API,
};

/// The background stub's second account (`secondAccount`), the one the grant never names.
fn second() -> DappAccount {
    DappAccount {
        id: "SN_SEPOLIA:0xfeedface".into(),
        address: "0xfeedface".into(),
        network_id: "SN_SEPOLIA".into(),
    }
}

fn account(value: &Value) -> DappAccount {
    DappAccount {
        id: text(&value["id"]).into(),
        address: text(&value["address"]).into(),
        network_id: text(&value["networkId"]).into(),
    }
}

fn grants(value: &Value) -> Vec<OriginGrant> {
    serde_json::from_value(value.clone()).expect("grants")
}

fn grants_json(grants: &[OriginGrant]) -> Value {
    serde_json::to_value(grants).unwrap()
}

/// What the page receives for one request, or `"pending"` while a card holds it.
fn page_answer(read: &PageRequest, site: &Site) -> Value {
    let refused = |error| json!({ "error": page_error(&error) });
    match read {
        PageRequest::GrantedAccounts => {
            let granted: Vec<&DappAccount> = site.granted().into_iter().collect();
            json!({ "result": plain(&accounts_view(&granted)) })
        }
        PageRequest::RequestAccounts => match request_accounts(site, false) {
            Ok(Connection::Granted(account)) => {
                json!({ "result": plain(&accounts_view(&[account])) })
            }
            Ok(Connection::Silent) => json!({ "result": plain(&accounts_view(&[])) }),
            Ok(Connection::Ask) => json!("pending"),
            Err(error) => refused(error),
        },
        PageRequest::Spec(request) => match answer(request, site, &VERSIONS) {
            Some(Ok(Answer::Result(value))) => json!({ "result": plain(&value) }),
            Some(Ok(Answer::Connect)) => json!("pending"),
            Some(Err(error)) => refused(error),
            None => panic!("not a connect request"),
        },
    }
}

/// A settled card's answer to one waiting request.
fn settled(read: &PageRequest, outcome: &Result<&DappAccount, Value>) -> Value {
    match outcome {
        Ok(account) => match read {
            PageRequest::Spec(_) => json!({ "result": plain(&addresses(&[account])) }),
            _ => json!({ "result": plain(&accounts_view(&[account])) }),
        },
        Err(error) => json!({ "error": error }),
    }
}

/// The wallet as `Site` reads it: blocked or not, locked or not, and which account is active.
struct State<'a> {
    origin: &'a str,
    chain: &'a str,
    grants: &'a [OriginGrant],
    blocked: bool,
    locked: bool,
    active: &'a DappAccount,
}

impl<'a> State<'a> {
    fn site(&self) -> Site<'a> {
        Site {
            origin: self.origin,
            blocked: self.blocked,
            unlocked: (!self.locked).then_some(Unlocked {
                grants: self.grants,
                active: Some(self.active),
            }),
            chain_id: self.chain,
        }
    }
}

#[test]
fn each_site_is_answered_and_each_card_decided_as_the_extension_did() {
    let stub_first = DappAccount {
        id: "SN_SEPOLIA:0xabcabcdef".into(),
        address: "0xabcabcdef".into(),
        network_id: "SN_SEPOLIA".into(),
    };
    for case in cases("connect", 59) {
        let name = name(&case);
        let state = &case["state"];
        let first = account(&state["active"]);
        let other = if first.id == second().id {
            stub_first.clone()
        } else {
            second()
        };
        let stored = grants(&state["grants"]);
        let now = State {
            origin: text(&state["origin"]),
            chain: text(&state["chainId"]),
            grants: &stored,
            blocked: state["blocked"] == true,
            locked: state["locked"] == true,
            active: &first,
        };
        let reads: Vec<PageRequest> = case["asks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|ask| {
                let params = ask.get("params").map(Value::to_string);
                read_page_request(text(&ask["method"]), params.as_deref()).expect(&name)
            })
            .collect();
        let mut answers: Vec<Value> = reads
            .iter()
            .map(|read| page_answer(read, &now.site()))
            .collect();

        let decision = &case["decided"];
        if !decision.is_null() {
            // What changed between the card and the click, as the extension's case set it up.
            // A site that asked while locked is decided once the wallet is unlocked.
            let at = match decision["before"].as_str() {
                Some("switch") => State {
                    active: &other,
                    locked: false,
                    ..now
                },
                Some("lock") => State {
                    locked: true,
                    ..now
                },
                Some("block") => State {
                    blocked: true,
                    locked: false,
                    ..now
                },
                _ => State {
                    locked: false,
                    ..now
                },
            };
            let outcome: Result<&DappAccount, Value> = if decision["decide"] == "deny" {
                Err(page_error(&deny_connect()))
            } else {
                match approve_connect(&at.site(), decision["echo"].as_str()) {
                    Ok((written, granted)) => {
                        // The grant the extension's `grantOrigin` was asked to write.
                        let expected = &case["grants"][0];
                        assert_eq!(granted.id, text(&expected["accountId"]), "{name}");
                        let site_grant = written
                            .iter()
                            .find(|grant| grant.origin == now.origin)
                            .expect(&name);
                        assert!(site_grant.account_ids.contains(&granted.id), "{name}");
                        Ok(granted)
                    }
                    Err(error) => {
                        assert!(case["grants"].as_array().unwrap().is_empty(), "{name}");
                        assert!(decision["reply"].get("error").is_some(), "{name}");
                        Err(page_error(&error))
                    }
                }
            };
            answers = reads.iter().map(|read| settled(read, &outcome)).collect();
        }
        assert_eq!(json!(answers), case["answers"], "{name}");
    }
}

#[test]
fn grants_are_written_and_read_as_the_vault_service_does() {
    let mut stored: Vec<OriginGrant> = Vec::new();
    let mut active: Option<String> = None;
    for (index, case) in cases("grants", 14).into_iter().enumerate() {
        let name = name(&case);
        let step = &case["step"];
        if index == 0 {
            active = case["active"].as_str().map(str::to_owned);
        }
        if case["refused"].is_null() {
            let origin = step["origin"].as_str().unwrap_or("");
            let named = step["accountId"].as_str();
            match text(&step["op"]) {
                "grant" => {
                    let id = named.or(active.as_deref()).expect(&name).to_owned();
                    stored = with_origin_grant(&stored, origin, &id);
                }
                "revoke" => {
                    if let Some(next) = without_origin_access(&stored, origin, named) {
                        stored = next;
                    }
                }
                _ => active = named.map(str::to_owned),
            }
        } else {
            // `grantOrigin` checks the origin first; the core's `DappSession` checks it before
            // anything reaches `grants` (`isWalletOrigin`), so nothing is written here either.
            assert_eq!(case["refused"], "Invalid origin", "{name}");
        }
        assert_eq!(case["active"].as_str(), active.as_deref(), "{name}");
        assert_eq!(grants_json(&stored), case["grants"], "{name}");
        assert_eq!(
            grants_json(&connected_sites(&stored)),
            case["connected"],
            "{name}"
        );
        for (origin, sees) in case["sees"].as_object().unwrap() {
            let ours: Vec<&str> = granted_account(&stored, active.as_deref(), origin)
                .into_iter()
                .collect();
            assert_eq!(json!(ours), *sees, "{name}: {origin}");
        }
    }
}

fn refusal(refusal: Refusal) -> &'static str {
    match refusal {
        Refusal::SiteFull => "Too many pending approvals for this site",
        Refusal::QueueFull => "Too many pending approvals",
    }
}

#[test]
fn the_queue_admits_joins_expires_and_evicts_as_the_extension_does() {
    for case in cases("queue", 9) {
        let name = name(&case);
        let mut now = case["start"].as_u64().unwrap();
        let mut queue: ApprovalQueue<()> = ApprovalQueue::new(DAPP_QUEUE_LIMITS);
        let mut labels: HashMap<ApprovalId, String> = HashMap::new();
        let mut reservations: Vec<Option<Reservation>> = Vec::new();
        let label = |labels: &mut HashMap<ApprovalId, String>, id: &ApprovalId| {
            let next = format!("c{}", labels.len());
            labels.entry(id.clone()).or_insert(next).clone()
        };
        for entry in case["steps"].as_array().unwrap() {
            let step = &entry["step"];
            let origin = step["origin"].as_str().unwrap_or("");
            let mut enqueue =
                |dedupe: String| match queue.enqueue(origin, Some(dedupe), (), now, None) {
                    Ok(Ok(queued)) => {
                        json!({ "card": label(&mut labels, &queued.id), "joined": queued.joined })
                    }
                    Ok(Err(refused)) => json!({ "refused": refusal(refused) }),
                    Err(error) => panic!("{name}: {error}"),
                };
            let result = match text(&step["op"]) {
                "connect" => enqueue("connect".into()),
                "switch" => enqueue(format!("switch:{}", text(&step["chainId"]))),
                "reserve" => match queue.reserve(origin, now) {
                    Ok(held) => {
                        reservations.push(Some(held));
                        json!({ "reservation": format!("r{}", reservations.len() - 1) })
                    }
                    Err(refused) => json!({ "refused": refusal(refused) }),
                },
                "release" => {
                    let at: usize = text(&step["reservation"])[1..].parse().unwrap();
                    if let Some(held) = reservations[at].take() {
                        queue.release(held);
                    }
                    Value::Null
                }
                "canAdmit" => {
                    let refused = queue.can_admit(origin, now).err().map(refusal);
                    json!({ "refusal": refused })
                }
                "claim" => {
                    let card = text(&step["card"]);
                    let id = labels
                        .iter()
                        .find(|(_, known)| *known == card)
                        .map_or_else(|| ApprovalId::from_echo("unknown"), |(id, _)| id.clone());
                    json!({ "claimed": queue.claim(&id, now).is_some() })
                }
                "advance" => {
                    now += step["ms"].as_u64().unwrap();
                    Value::Null
                }
                "list" => {
                    let listed: Vec<(ApprovalId, String)> = queue
                        .list(now)
                        .into_iter()
                        .map(|approval| (approval.id.clone(), approval.origin.clone()))
                        .collect();
                    let cards: Vec<Value> = listed
                        .iter()
                        .map(|(id, origin)| json!({ "card": label(&mut labels, id), "origin": origin }))
                        .collect();
                    json!({ "cards": cards })
                }
                other => panic!("unknown step {other}"),
            };
            let dropped: Vec<String> = queue
                .take_dropped()
                .iter()
                .map(|id| label(&mut labels, id))
                .collect();
            assert_eq!(result, entry["result"], "{name}: {step}");
            assert_eq!(json!(dropped), entry["dropped"], "{name}: {step}");
        }
    }
}
