//! Multisig coordination parity with the extension (docs/PLAN.md, step 2.5b): every case in
//! `nostr-coordination-parity-{rules,relays,paging}.json`, written by Bramble's
//! `scripts/mobile-nostr-coordination-*.ts` from the extension's own functions. Events are
//! numbered: event `n` is the relays fixture's `event` with the id `n` as 64-digit hex and the
//! case's `created_at`. Relays are replayed from the recorded frames, so every REQ the core builds
//! is compared verbatim with the one the extension sent.

use std::collections::HashSet;

use bramble_core::coordination::{
    coordination_recipients, coordination_relay_urls, is_coordination_signer, with_nostr_relay_url,
    AddressBookEntry, MAX_SYNC_PAGES, MULTISIG_NOSTR_IDENTIFIER, MULTISIG_NOSTR_RELAYS,
    NOSTR_INCREMENTAL_LOOKBACK_SECONDS, NOSTR_SYNC_LOOKBACK_SECONDS, RELAY_LIMITS, SYNC_LOOKBACKS,
};
use bramble_core::CoreError;
use serde_json::{json, Value};
use starknet_wallet_core::accounts::multisig::signer_guid_hex;
use starknet_wallet_core::nostr::relay::{
    publish_many_nostr_gift_wraps, NostrGiftWrapDelivery, RelayStep, MAX_RELAY_FRAME_BYTES,
};
use starknet_wallet_core::nostr::relay_url::is_nostr_relay_url;
use starknet_wallet_core::nostr::sync::sync_nostr_gift_wraps;
use starknet_wallet_core::nostr::sync_window::{
    is_wrap_past_retry, nostr_sync_since, retryable_synced_at,
};
use starknet_wallet_core::nostr::MAX_BATCH_ITEMS;

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn part(name: &str) -> Value {
    fixture(&format!("nostr-coordination-parity-{name}"))
}

fn text(value: &Value) -> String {
    value.as_str().unwrap().to_string()
}

fn strings(value: &Value) -> Vec<String> {
    value.as_array().unwrap().iter().map(text).collect()
}

fn numbers(value: &Value) -> Vec<u64> {
    let values = value.as_array().unwrap();
    values.iter().map(|n| n.as_u64().unwrap()).collect()
}

fn id_of(n: u64) -> String {
    format!("{n:064x}")
}

fn number_of(id: &str) -> u64 {
    u64::from_str_radix(id, 16).unwrap()
}

/// Event `n` at `created_at`, from the fixture's template.
fn event(template: &Value, n: u64, created_at: u64) -> Value {
    let mut event = template.clone();
    event["id"] = json!(id_of(n));
    event["created_at"] = json!(created_at);
    event
}

#[test]
fn bramble_constants_match_the_extension() {
    let rules = part("rules");
    assert_eq!(
        strings(&rules["relayPolicy"]["defaults"]),
        MULTISIG_NOSTR_RELAYS
    );
    let window = &rules["syncWindow"];
    assert_eq!(window["lookbackSeconds"], NOSTR_SYNC_LOOKBACK_SECONDS);
    assert_eq!(
        window["incrementalLookbackSeconds"],
        NOSTR_INCREMENTAL_LOOKBACK_SECONDS
    );
    assert_eq!(RELAY_LIMITS.max_relays, MULTISIG_NOSTR_RELAYS.len());
    assert_eq!(RELAY_LIMITS.max_sync_pages, MAX_SYNC_PAGES);
    // Bramble's generated constants (`generated/multisig.json`) name the same values.
    let generated = fixture("multisig");
    assert_eq!(
        generated["coordination"]["identifier"],
        MULTISIG_NOSTR_IDENTIFIER
    );
    assert_eq!(
        generated["coordination"]["syncLookbackSeconds"],
        NOSTR_SYNC_LOOKBACK_SECONDS
    );
    assert_eq!(
        generated["coordination"]["incrementalLookbackSeconds"],
        NOSTR_INCREMENTAL_LOOKBACK_SECONDS
    );
    assert_eq!(
        strings(&generated["relays"]["defaults"]),
        MULTISIG_NOSTR_RELAYS
    );
    assert_eq!(generated["nostr"]["maxBatchItems"], MAX_BATCH_ITEMS);
    assert_eq!(generated["relays"]["maxFrameBytes"], MAX_RELAY_FRAME_BYTES);
    // The page cap: the extension stopped after exactly this many requests.
    let capped = part("paging")
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "paging stops after the page cap")
        .unwrap()["relays"][0]["requests"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(capped, MAX_SYNC_PAGES);
}

#[test]
fn relay_urls_match_the_extension() {
    let cases = part("rules")["relayUrls"].as_array().unwrap().clone();
    assert_eq!(cases.len(), 41);
    for case in cases {
        let value = text(&case["value"]);
        assert_eq!(is_nostr_relay_url(&value), case["accepted"], "{value}");
    }
}

/// `CoreError`'s reason, or the stored relay preference.
fn set_relay(input: &str) -> Result<Option<String>, String> {
    with_nostr_relay_url(input).map_err(|error| match error {
        CoreError::InvalidInput { reason } => reason,
        other => other.to_string(),
    })
}

#[test]
fn relay_policy_matches_the_extension() {
    let rules = part("rules");
    let cases = rules["relayPolicy"]["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 10);
    for case in cases {
        let mut stored = case["stored"].as_str().map(str::to_string);
        let set = case["input"].as_str().map(set_relay);
        match (set, case.get("relayUrls")) {
            (Some(Err(reason)), None) => assert_eq!(reason, case["error"], "{case}"),
            (set, Some(urls)) => {
                if let Some(set) = set {
                    stored = set.unwrap();
                }
                assert_eq!(
                    coordination_relay_urls(stored.as_deref()),
                    strings(urls),
                    "{case}"
                );
            }
            (set, None) => panic!("{case}: {set:?}"),
        }
    }
    // The earlier fixture's settings vectors: trimmed, then the grammar or the defaults.
    for case in fixture("nostr-multisig-parity")["relayUrls"]
        .as_array()
        .unwrap()
    {
        let value = text(&case["value"]);
        let expected = match case["extension"].as_str().unwrap() {
            "valid" => Ok(Some(value.trim().to_string())),
            "defaults" => Ok(None),
            _ => Err("Invalid Nostr relay endpoint".to_string()),
        };
        assert_eq!(set_relay(&value), expected, "{value}");
    }
}

#[test]
fn the_sync_window_matches_the_extension() {
    let window = &part("rules")["syncWindow"];
    for case in window["since"].as_array().unwrap() {
        let since = nostr_sync_since(
            case["now"].as_i64().unwrap(),
            case["syncedAt"].as_i64(),
            SYNC_LOOKBACKS,
        );
        assert_eq!(since, case["since"], "{case}");
    }
    for case in window["syncedAt"].as_array().unwrap() {
        let synced_at = retryable_synced_at(
            case["now"].as_i64().unwrap(),
            case["oldestRetryableAt"].as_i64(),
            SYNC_LOOKBACKS,
        );
        assert_eq!(synced_at, case["syncedAt"], "{case}");
    }
    for case in window["pastRetry"].as_array().unwrap() {
        // `now` is the unfloored clock the extension passes, fractional in one case.
        let past = is_wrap_past_retry(
            case["createdAt"].as_u64().unwrap(),
            case["now"].as_f64().unwrap(),
            SYNC_LOOKBACKS,
        );
        assert_eq!(past, case["pastRetry"], "{case}");
    }
}

fn contact(value: &Value) -> AddressBookEntry {
    AddressBookEntry {
        id: text(&value["id"]),
        network_id: text(&value["networkId"]),
        nostr_public_key: value["nostrPublicKey"].as_str().map(str::to_string),
        public_key_x: value["publicKeyX"].as_str().map(str::to_string),
    }
}

#[test]
fn recipients_and_the_sender_rule_match_the_extension() {
    let recipients = &part("rules")["recipients"];
    let guids = strings(&recipients["signerGuids"]);
    for signer in recipients["signers"].as_array().unwrap() {
        let guid = signer_guid_hex(signer["publicKeyX"].as_str().unwrap()).unwrap();
        assert_eq!(guid, signer["guid"]);
        assert!(guids.contains(&guid));
    }
    let contacts: Vec<AddressBookEntry> = recipients["contacts"]
        .as_array()
        .unwrap()
        .iter()
        .map(contact)
        .collect();
    let selected = coordination_recipients(
        &contacts,
        recipients["networkId"].as_str().unwrap(),
        recipients["ownPublicKey"].as_str().unwrap(),
        &guids,
    );
    let ids: Vec<&str> = selected.iter().map(|entry| entry.id.as_str()).collect();
    assert_eq!(ids, strings(&recipients["recipients"]));
    for case in recipients["senders"].as_array().unwrap() {
        let signer = is_coordination_signer(
            &contacts,
            case["senderPublicKey"].as_str().unwrap(),
            case["networkId"].as_str().unwrap(),
            &guids,
        );
        assert_eq!(signer, case["signer"], "{case}");
    }
}

/// Publishes through the core, replying to each wrap a relay was sent with `reply(relay, id)`, a
/// JSON `OK` frame or nothing. The counts, or the extension's error message.
fn publish(
    deliveries: &[NostrGiftWrapDelivery],
    relay_urls: &[String],
    reply: impl Fn(usize, &str) -> Option<String>,
) -> Result<Vec<u64>, String> {
    let mut publish = publish_many_nostr_gift_wraps(deliveries, relay_urls, RELAY_LIMITS)
        .map_err(|error| error.to_string())?;
    for relay in 0..relay_urls.len() {
        for frame in publish.event_frames() {
            let sent: Value = serde_json::from_str(&frame).unwrap();
            assert_eq!(sent[0], "EVENT");
            if let Some(ok) = reply(relay, sent[1]["id"].as_str().unwrap()) {
                publish.accept_frame(relay, &ok);
            }
        }
    }
    let counts = publish
        .acknowledgements()
        .map_err(|error| error.to_string())?;
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
    let cases = batches["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 16);
    for case in cases {
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
fn acknowledgements_match_the_extension() {
    let relays = part("relays");
    let (template, recipient) = (&relays["event"], text(&relays["recipient"]));
    let cases = relays["acknowledgements"].as_array().unwrap();
    assert_eq!(cases.len(), 11);
    for case in cases {
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
        assert_eq!(
            publish(&deliveries, &urls, reply),
            outcome(case),
            "{}",
            case["name"]
        );
    }
}

/// Replays one paging case through the core: each relay answers each recorded request with the
/// recorded events (or CLOSED when it refuses) and EOSE, and every REQ the core asks for must be
/// the recorded frame. The ordered result, or the extension's error.
fn replay(template: &Value, recipient: &str, case: &Value) -> Result<Vec<u64>, String> {
    let scripted = case["relays"].as_array().unwrap();
    let urls = scripted.iter().map(|relay| text(&relay["url"])).collect();
    let seen: HashSet<String> = numbers(&case["seen"]).into_iter().map(id_of).collect();
    let since = case["since"].as_i64().unwrap();
    let recipient = case["recipient"].as_str().unwrap_or(recipient);
    let mut sync = match sync_nostr_gift_wraps(recipient, since, urls, seen, RELAY_LIMITS) {
        Ok(sync) => sync,
        Err(error) => {
            assert!(scripted.iter().all(|relay| relay["requests"] == json!([])));
            return Err(error.to_string());
        }
    };
    for (index, relay) in scripted.iter().enumerate() {
        let name = format!("{} / {}", case["name"], relay["url"]);
        let stored = numbers(&relay["ids"]);
        let created = numbers(&relay["createdAt"]);
        let requests = relay["requests"].as_array().unwrap();
        let mut request = sync.open_frame();
        for (page, recorded) in requests.iter().enumerate() {
            assert_eq!(request, text(&recorded["frame"]), "{name}: request {page}");
            let subscription = serde_json::from_str::<Value>(&request).unwrap()[1].clone();
            let mut step = RelayStep::Continue;
            if relay.get("refuses").is_some() {
                let closed = json!(["CLOSED", subscription, "restricted: parity"]);
                step = sync.accept_frame(index, &closed.to_string());
            } else {
                for n in numbers(&recorded["answer"]) {
                    let at = created[stored.iter().position(|id| *id == n).unwrap()];
                    let frame = json!(["EVENT", subscription, event(template, n, at)]);
                    if step == RelayStep::Continue {
                        step = sync.accept_frame(index, &frame.to_string());
                    }
                }
                if step == RelayStep::Continue {
                    let eose = json!(["EOSE", subscription]).to_string();
                    step = sync.accept_frame(index, &eose);
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
    let candidates = sync.candidates().map_err(|error| error.to_string())?;
    Ok(candidates
        .iter()
        .map(|event| number_of(&event.id))
        .collect())
}

#[test]
fn sync_paging_matches_the_extension() {
    let (paging, relays) = (part("paging"), part("relays"));
    let (template, recipient) = (&relays["event"], text(&relays["recipient"]));
    let cases = paging.as_array().unwrap();
    assert_eq!(cases.len(), 14);
    for case in cases {
        let expected = match case.get("result") {
            Some(result) => Ok(numbers(result)),
            None => Err(text(&case["error"])),
        };
        assert_eq!(
            replay(template, &recipient, case),
            expected,
            "{}",
            case["name"]
        );
    }
}
