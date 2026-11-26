//! Nostr coordination parity with the extension, over the shared core's relay judges and sync
//! window (wallet-core plan, step 2.5b). `tests/fixtures/nostr-coordination-parity-*.json` are
//! written by `bun run mobile:parity` from the extension's own functions and re-checked by
//! `tests/conformance/mobile-nostr-parity.test.ts`. Events are numbered: event `n` is the
//! fixture's `event` with the id `n` as 64-digit hex and the case's `created_at`.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

mod common;

use std::collections::HashSet;

use bramble_core::testing::{coordination_sender_trusted, publish_many, SyncJudge};
use bramble_core::{
    nostr_relay_url_normalize, CoordinationSyncStep, CoreError, MultisigAuthorityRead,
    MultisigAuthorityStatus, NetworkId, RelayStep, TokenPreferences, WalletCore,
};
use serde_json::{json, Value};
use starknet_wallet_core::nostr::relay::{NostrGiftWrapDelivery, RelayError};

/// Cases where mobile differs from the extension, left out of the assertions below rather than
/// asserted either way. Every other case is asserted.
const MOBILE_DIFFERS: [&str; 3] = [
    // Mobile refuses `ws:` relays, loopback included: App Transport Security and
    // `usesCleartextTraffic=false` block cleartext sockets.
    "ws://localhost:7000",
    "ws://127.0.0.1:7000",
    "a loopback relay",
];

fn differs(case: &Value, key: &str) -> bool {
    case[key]
        .as_str()
        .is_some_and(|name| MOBILE_DIFFERS.contains(&name))
}

fn part(name: &str) -> Value {
    let path = format!("tests/fixtures/nostr-coordination-parity-{name}.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn text(value: &Value) -> String {
    value.as_str().unwrap().to_string()
}

fn strings(value: &Value) -> Vec<String> {
    value.as_array().unwrap().iter().map(text).collect()
}

fn numbers(value: &Value) -> Vec<u64> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_u64().unwrap())
        .collect()
}

fn id_of(n: u64) -> String {
    format!("{n:064x}")
}

/// Event `n` at `created_at`, from the fixture's template.
fn event(template: &Value, n: u64, created_at: u64) -> Value {
    let mut event = template.clone();
    event["id"] = json!(id_of(n));
    event["created_at"] = json!(created_at);
    event
}

#[test]
fn relay_urls_match_the_extension() {
    for case in part("rules")["relayUrls"].as_array().unwrap() {
        let value = text(&case["value"]);
        // `nostr_relay_url_normalize` trims and reads empty as "use the defaults"; only a value it
        // passes through unchanged tests the grammar itself.
        if value.trim() != value || value.is_empty() || differs(case, "value") {
            continue;
        }
        let mobile = matches!(nostr_relay_url_normalize(value.clone()), Ok(Some(_)));
        assert_eq!(mobile, case["accepted"], "{value}");
    }
}

#[test]
fn relay_policy_matches_the_extension() {
    let rules = part("rules");
    let (_dir, _clock, core) = common::imported();
    let defaults = strings(&rules["relayPolicy"]["defaults"]);
    assert_eq!(core.relay_policy().unwrap().relay_urls, defaults);
    for case in rules["relayPolicy"]["cases"].as_array().unwrap() {
        if differs(case, "name") {
            continue;
        }
        core.set_nostr_relay_url(String::new()).unwrap();
        if let Some(stored) = case["stored"].as_str() {
            core.set_nostr_relay_url(stored.into()).unwrap();
        }
        let set = case["input"]
            .as_str()
            .map(|input| core.set_nostr_relay_url(input.into()));
        match case.get("relayUrls") {
            Some(urls) => {
                assert!(set.is_none_or(|set| set.is_ok()), "{case}");
                assert_eq!(
                    core.relay_policy().unwrap().relay_urls,
                    strings(urls),
                    "{case}"
                );
            }
            None => assert!(set.is_some_and(|set| set.is_err()), "{case}"),
        }
    }
}

fn respond(read: &MultisigAuthorityRead, result: Value) {
    let request = read.next_request().unwrap();
    let body = json!({"jsonrpc": "2.0", "id": request.id, "result": result}).to_string();
    read.accept_response(request.id, body).unwrap();
}

/// A wallet holding one joined Sepolia multisig (the first join in the multisig fixture).
fn wallet_with_multisig() -> (tempfile::TempDir, std::sync::Arc<WalletCore>, String) {
    let multisig: Value =
        serde_json::from_str(include_str!("fixtures/nostr-multisig-parity.json")).unwrap();
    let join = &multisig["joins"][0];
    let (dir, _clock, core) = common::imported();
    let read = MultisigAuthorityRead::new(NetworkId::Sepolia, text(&join["address"])).unwrap();
    respond(&read, json!("0x534e5f5345504f4c4941"));
    respond(&read, json!({"block_hash": "0x1", "block_number": 1}));
    respond(&read, multisig["classHash"].clone());
    respond(
        &read,
        json!([format!("0x{:x}", join["threshold"].as_u64().unwrap())]),
    );
    let guids = join["signerGuids"].as_array().unwrap();
    let mut answer = vec![json!(format!("0x{:x}", guids.len()))];
    answer.extend(guids.iter().rev().cloned());
    respond(&read, Value::Array(answer));
    assert_eq!(read.status(), Some(MultisigAuthorityStatus::Available));
    core.add_multisig(read).unwrap();
    (dir, core, text(&join["record"]["id"]))
}

/// The `since` of the first relay request a sync started at `now` makes.
fn sync_since(core: &WalletCore, id: &str, now: u64) -> u64 {
    let plan = core.coordination_sync_begin(id.into(), now).unwrap();
    let frame: Value = serde_json::from_str(&plan.open_frame(0)).unwrap();
    frame[2]["since"].as_u64().unwrap()
}

/// Completes an empty sync at `now`, which records the checkpoint `syncedAt = now`.
fn record_sync_at(core: &WalletCore, id: &str, now: u64) {
    let plan = core.coordination_sync_begin(id.into(), now).unwrap();
    for relay in 0..plan.relay_urls().len() as u32 {
        plan.accept_frame(relay, json!(["EOSE", "mcw-1"]).to_string());
    }
    let step = core
        .coordination_sync_open(plan, TokenPreferences::new(None))
        .unwrap();
    assert!(matches!(step, CoordinationSyncStep::Done { .. }));
}

#[test]
fn the_sync_since_matches_the_extension() {
    for case in part("rules")["syncWindow"]["since"].as_array().unwrap() {
        let (_dir, core, id) = wallet_with_multisig();
        if let Some(synced_at) = case["syncedAt"].as_u64() {
            record_sync_at(&core, &id, synced_at);
        }
        let now = case["now"].as_u64().unwrap();
        assert_eq!(sync_since(&core, &id, now), case["since"], "{case}");
    }
}

/// The reason a `CoreError` carries, which is the core's message for relay failures.
fn reason(error: CoreError) -> String {
    match error {
        CoreError::InvalidInput { reason } | CoreError::Coordination { reason } => reason,
        other => other.to_string(),
    }
}

/// Replays one paging case through mobile's sync judge, the one `NostrSyncPlan` drives, as the
/// host's relay loop would: each relay answers each recorded request with the recorded events (or
/// CLOSED when it refuses) and EOSE, the host follows the exported `RelayStep`, and every REQ the
/// judge asks for must be the recorded frame. The ordered result, or the error message.
fn mobile_candidates(recipient: &str, template: &Value, case: &Value) -> Result<Vec<u64>, String> {
    let recipient = case["recipient"].as_str().unwrap_or(recipient);
    let since = case["since"].as_i64().unwrap();
    let relays = case["relays"].as_array().unwrap();
    let urls = relays.iter().map(|relay| text(&relay["url"])).collect();
    let seen: HashSet<String> = numbers(&case["seen"]).into_iter().map(id_of).collect();
    let judge = SyncJudge::new(recipient, since, urls, seen).map_err(reason)?;
    for (index, relay) in relays.iter().enumerate() {
        let name = format!("{} / {}", case["name"], relay["url"]);
        let stored = numbers(&relay["ids"]);
        let created = numbers(&relay["createdAt"]);
        let requests = relay["requests"].as_array().unwrap();
        let mut request = judge.open_frame();
        for (page, recorded) in requests.iter().enumerate() {
            assert_eq!(request, text(&recorded["frame"]), "{name}: request {page}");
            let subscription = serde_json::from_str::<Value>(&request).unwrap()[1].clone();
            let mut step = RelayStep::Continue;
            if relay.get("refuses").is_some() {
                let closed = json!(["CLOSED", subscription, "restricted"]);
                step = judge.accept_frame(index, &closed.to_string());
            } else {
                for n in numbers(&recorded["answer"]) {
                    let at = created[stored.iter().position(|id| *id == n).unwrap()];
                    let frame = json!(["EVENT", subscription, event(template, n, at)]);
                    if step == RelayStep::Continue {
                        step = judge.accept_frame(index, &frame.to_string());
                    }
                }
                if step == RelayStep::Continue {
                    let eose = json!(["EOSE", subscription]).to_string();
                    step = judge.accept_frame(index, &eose);
                }
            }
            let last = page + 1 == requests.len();
            match step {
                RelayStep::NextPage {
                    close_frame,
                    request: next,
                } if !last => {
                    assert_eq!(close_frame, json!(["CLOSE", subscription]).to_string());
                    request = next;
                }
                RelayStep::Finish { .. } if last => {}
                step => panic!("{name}: request {page} ended with {step:?}"),
            }
        }
    }
    let candidates = judge.candidates().map_err(reason)?;
    Ok(candidates
        .into_iter()
        .map(|event| u64::from_str_radix(&event.id, 16).unwrap())
        .collect())
}

#[test]
fn sync_paging_matches_the_extension() {
    let (paging, relays) = (part("paging"), part("relays"));
    let template = &relays["event"];
    let recipient = text(&relays["recipient"]);
    for case in paging.as_array().unwrap() {
        if differs(case, "name") {
            continue;
        }
        let mobile = mobile_candidates(&recipient, template, case);
        match case.get("result") {
            Some(result) => assert_eq!(mobile, Ok(numbers(result)), "{}", case["name"]),
            None => assert_eq!(mobile, Err(text(&case["error"])), "{}", case["name"]),
        }
    }
}

/// Publishes through mobile, replying to each wrap a relay was sent with `reply(relay, id)`, an
/// `OK` frame or nothing. The counts, or the error message.
fn publish(
    deliveries: &[NostrGiftWrapDelivery],
    relay_urls: &[String],
    reply: impl Fn(usize, &str) -> Option<String>,
) -> Result<Vec<u64>, String> {
    let mut publish = publish_many(deliveries, relay_urls).map_err(reason)?;
    for relay in 0..relay_urls.len() {
        for frame in publish.event_frames() {
            let sent: Value = serde_json::from_str(&frame).unwrap();
            if let Some(ok) = reply(relay, sent[1]["id"].as_str().unwrap()) {
                publish.accept_frame(relay, &ok);
            }
        }
    }
    let counts = publish
        .acknowledgements()
        .map_err(|error: RelayError| error.to_string())?;
    Ok(counts.into_iter().map(u64::from).collect())
}

fn outcome(case: &Value) -> Result<Vec<u64>, String> {
    match case.get("acknowledgements") {
        Some(counts) => Ok(numbers(counts)),
        None => Err(text(&case["error"])),
    }
}

#[test]
fn gift_wrap_batches_match_the_extension() {
    let relays = part("relays");
    let (template, recipient) = (&relays["event"], text(&relays["recipient"]));
    let batches = &relays["giftWrapBatches"];
    for case in batches["cases"].as_array().unwrap() {
        let mut deliveries: Vec<NostrGiftWrapDelivery> = numbers(&case["ids"])
            .into_iter()
            .map(|n| NostrGiftWrapDelivery {
                event_json: event(template, n, 1).to_string(),
                recipient_public_key: recipient.clone(),
            })
            .collect();
        deliveries.extend(case["extra"].as_array().unwrap().iter().map(|extra| {
            NostrGiftWrapDelivery {
                event_json: text(&extra["eventJson"]),
                recipient_public_key: text(&extra["recipientPublicKey"]),
            }
        }));
        // One relay that accepts every wrap it is sent.
        let accepting = |_: usize, id: &str| Some(json!(["OK", id, true, ""]).to_string());
        let counts = publish(&deliveries, &strings(&batches["relayUrls"]), accepting);
        assert_eq!(counts, outcome(case), "{}", case["name"]);
    }
}

#[test]
fn acknowledgement_counts_match_the_extension() {
    let relays = part("relays");
    let (template, recipient) = (&relays["event"], text(&relays["recipient"]));
    for case in relays["acknowledgements"].as_array().unwrap() {
        let ids = numbers(&case["ids"]);
        let deliveries: Vec<NostrGiftWrapDelivery> = ids
            .iter()
            .map(|n| NostrGiftWrapDelivery {
                event_json: event(template, *n, 1).to_string(),
                recipient_public_key: recipient.clone(),
            })
            .collect();
        let scripted = case["relays"].as_array().unwrap();
        let urls: Vec<String> = scripted.iter().map(|relay| text(&relay["url"])).collect();
        // Each relay's reply to a wrap, by delivery order; "socket error" replies to nothing.
        let reply = |relay: usize, id: &str| {
            let replies = scripted[relay]["replies"].as_array()?;
            let index = ids.iter().position(|n| id_of(*n) == id)?;
            let [accepted, message] = replies[index].as_array()?.as_slice() else {
                return None;
            };
            Some(json!(["OK", id, accepted, message]).to_string())
        };
        let counts = publish(&deliveries, &urls, reply);
        assert_eq!(counts, outcome(case), "{}", case["name"]);
    }
}

#[test]
fn the_sender_rule_matches_the_extension() {
    let recipients = &part("rules")["recipients"];
    let contacts = recipients["contacts"].to_string();
    let guids = strings(&recipients["signerGuids"]);
    for case in recipients["senders"].as_array().unwrap() {
        let trusted = coordination_sender_trusted(
            &contacts,
            case["networkId"].as_str().unwrap(),
            case["senderPublicKey"].as_str().unwrap(),
            &guids,
        );
        assert_eq!(trusted, case["signer"], "{case}");
    }
}
