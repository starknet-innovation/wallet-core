//! Sync judge tests, moved from mobile's `nostr_relay_tests.rs`. Each mirrors a case in the
//! extension's `tests/background/multisig/nostr-relay.test.ts` (named in the comment above it),
//! plus the ignore-and-continue cases that pin `parseFrame` and the early-end rules. Paging itself
//! is held to the extension's recorded frames in `bramble-core`'s
//! `tests/nostr_coordination_parity.rs`.

use super::*;
use serde_json::json;

const RECIPIENT: &str = "4444444444444444444444444444444444444444444444444444444444444444";
const DEFAULTS: [&str; 2] = ["wss://relay.primal.net", "wss://nos.lol"];
const SUBSCRIPTION_ID: &str = "mcw-1";

fn event(id: &str, recipient: &str) -> Value {
    json!({"content":"ciphertext","created_at":1,"id":id,"kind":1059,
           "pubkey":"22".repeat(32),"sig":"33".repeat(64),"tags":[["p",recipient]]})
}

fn frame(kind: &str, payload: Option<Value>) -> String {
    match payload {
        Some(payload) => json!([kind, SUBSCRIPTION_ID, payload]).to_string(),
        None => json!([kind, SUBSCRIPTION_ID]).to_string(),
    }
}

const LIMITS: RelayLimits = RelayLimits {
    max_relays: 2,
    max_sync_pages: 4,
};

fn judge(relays: &[&str]) -> SyncJudge {
    let relays = relays.iter().map(|r| r.to_string()).collect();
    sync_nostr_gift_wraps(RECIPIENT, 1, relays, HashSet::new(), LIMITS).unwrap()
}

fn close() -> RelayStep {
    RelayStep::Finish {
        close_frame: Some(json!(["CLOSE", SUBSCRIPTION_ID]).to_string()),
    }
}

// "syncs only matching gift wraps from each relay"
#[test]
fn syncs_only_matching_gift_wraps_from_each_relay() {
    let mut sync = judge(&DEFAULTS);
    let request: Value = serde_json::from_str(&sync.open_frame()).unwrap();
    assert_eq!(
        request,
        json!(["REQ", "mcw-1", {"#p":[RECIPIENT],"kinds":[1059],"limit":50,"since":1}])
    );
    assert!(sync.open_frame().contains(r#""kinds":[1059]"#));
    for relay in 0..2 {
        let other = event(&"22".repeat(32), &"55".repeat(32));
        assert_eq!(
            sync.accept_frame(relay, &frame("EVENT", Some(other))),
            RelayStep::Continue
        );
        let wrap = event(&"11".repeat(32), RECIPIENT);
        assert_eq!(
            sync.accept_frame(relay, &frame("EVENT", Some(wrap))),
            RelayStep::Continue
        );
        assert_eq!(sync.accept_frame(relay, &frame("EOSE", None)), close());
    }
    let candidates = sync.candidates().unwrap();
    assert_eq!(candidates.len(), 2);
    assert!(candidates.iter().all(|c| c.id == "11".repeat(32)));
    let bad = sync_nostr_gift_wraps("0x44", 1, vec![DEFAULTS[0].into()], HashSet::new(), LIMITS);
    assert_eq!(bad.err(), Some(RelayError::InvalidSyncRequest));
}

// "preserves same-id relay candidates until Krusty authenticates them"
#[test]
fn preserves_same_id_candidates_across_relays() {
    let mut sync = judge(&DEFAULTS);
    for (relay, url) in DEFAULTS.iter().enumerate() {
        let mut wrap = event(&"ab".repeat(32), RECIPIENT);
        wrap["content"] = json!(url);
        sync.accept_frame(relay, &frame("EVENT", Some(wrap)));
        sync.accept_frame(relay, &frame("EOSE", None));
    }
    let mut contents: Vec<String> = sync
        .candidates()
        .unwrap()
        .into_iter()
        .map(|c| c.content)
        .collect();
    contents.sort();
    assert_eq!(contents, ["wss://nos.lol", "wss://relay.primal.net"]);
}

// "gives each relay space in the pre-authentication batch"
#[test]
fn gives_each_relay_space_in_the_batch() {
    let mut sync = judge(&DEFAULTS);
    let mut honest = event(&"fe".repeat(32), RECIPIENT);
    honest["content"] = json!("honest");
    sync.accept_frame(1, &frame("EVENT", Some(honest)));
    sync.accept_frame(1, &frame("EOSE", None));
    for index in 0..49u64 {
        let mut flood = event(&format!("{index:064x}"), RECIPIENT);
        flood["created_at"] = json!(9_007_199_254_740_991u64 - index);
        sync.accept_frame(0, &frame("EVENT", Some(flood)));
    }
    sync.accept_frame(0, &frame("EOSE", None));
    let candidates = sync.candidates().unwrap();
    assert_eq!(candidates.len(), 50);
    assert!(candidates.iter().any(|c| c.content == "honest"));
}

// "closes a subscription at the accepted frame budget". With paging, a full first page asks for
// the next one below its oldest wrap; here every wrap is from one second, so it steps below it.
#[test]
fn closes_a_subscription_at_the_frame_budget() {
    let mut sync = judge(&["wss://relay.team.example"]);
    for index in 0..49u64 {
        let wrap = event(&format!("{index:064x}"), RECIPIENT);
        assert_eq!(
            sync.accept_frame(0, &frame("EVENT", Some(wrap))),
            RelayStep::Continue
        );
    }
    // All fifty wraps are unseen, so the batch is full and no further page is read.
    assert_eq!(
        sync.accept_frame(0, &frame("EVENT", Some(event(&"ff".repeat(32), RECIPIENT)))),
        close()
    );
    assert_eq!(sync.candidates().unwrap().len(), 50);
}

// Every EVENT frame counts toward the budget, accepted or not (`pending.frames`).
#[test]
fn rejected_event_frames_count_toward_the_budget() {
    let mut sync = judge(&["wss://relay.team.example"]);
    let foreign = event(&"22".repeat(32), &"55".repeat(32));
    for _ in 0..49 {
        assert_eq!(
            sync.accept_frame(0, &frame("EVENT", Some(foreign.clone()))),
            RelayStep::Continue
        );
    }
    assert_eq!(
        sync.accept_frame(0, &frame("EVENT", Some(json!({"not": "an event"})))),
        close()
    );
    // As `#finishSubscription` without an error: the relay succeeded with nothing to offer.
    assert!(sync.candidates().unwrap().is_empty());
}

// "fails a refused subscription without waiting for the relay timeout"
#[test]
fn fails_a_refused_subscription() {
    let mut sync = judge(&["wss://relay.team.example"]);
    let closed = json!(["CLOSED", SUBSCRIPTION_ID, "restricted: test"]).to_string();
    assert_eq!(
        sync.accept_frame(0, &closed),
        RelayStep::Finish { close_frame: None }
    );
    assert_eq!(sync.candidates(), Err(RelayError::RelaysUnavailable));
}

// "retains authenticated candidates received before a relay disconnect"
#[test]
fn retains_candidates_received_before_a_disconnect() {
    let mut sync = judge(&["wss://relay.team.example"]);
    sync.accept_frame(0, &frame("EVENT", Some(event(&"88".repeat(32), RECIPIENT))));
    sync.relay_ended(0);
    assert_eq!(sync.candidates().unwrap().len(), 1);
}

// Must-fix: a timeout after two kept events keeps both.
#[test]
fn a_timeout_after_two_events_keeps_both() {
    let mut sync = judge(&["wss://relay.team.example"]);
    sync.accept_frame(0, &frame("EVENT", Some(event(&"01".repeat(32), RECIPIENT))));
    sync.accept_frame(0, &frame("EVENT", Some(event(&"02".repeat(32), RECIPIENT))));
    sync.relay_ended(0);
    assert_eq!(sync.candidates().unwrap().len(), 2);
}

// Must-fix: CLOSED after events keeps them.
#[test]
fn closed_after_events_keeps_them() {
    let mut sync = judge(&["wss://relay.team.example"]);
    sync.accept_frame(0, &frame("EVENT", Some(event(&"01".repeat(32), RECIPIENT))));
    sync.accept_frame(
        0,
        &json!(["CLOSED", SUBSCRIPTION_ID, "error: shutting down"]).to_string(),
    );
    assert_eq!(sync.candidates().unwrap().len(), 1);
}

// Must-fix: oversized, non-JSON, non-array and five-element frames are ignored, not fatal.
#[test]
fn malformed_frames_are_ignored_and_the_subscription_continues() {
    let mut sync = judge(&["wss://relay.team.example"]);
    let oversized = format!(r#"["NOTICE","{}"]"#, "x".repeat(140_001));
    for ignored in [
        oversized.as_str(),
        "not json",
        r#"{"EVENT":1}"#,
        r#"["EVENT","mcw-1",{},1,2]"#,
        r#"["EVENT","mcw-2",{}]"#,
        r#"["OK","mcw-1",true,""]"#,
    ] {
        assert_eq!(sync.accept_frame(0, ignored), RelayStep::Continue);
    }
    sync.accept_frame(0, &frame("EVENT", Some(event(&"03".repeat(32), RECIPIENT))));
    assert_eq!(sync.accept_frame(0, &frame("EOSE", None)), close());
    assert_eq!(sync.candidates().unwrap().len(), 1);
}

// The frame limit is strictly greater than 140,000 bytes: exactly 140,000 is read.
#[test]
fn a_frame_of_exactly_the_limit_is_read_and_one_byte_more_is_ignored() {
    let sized = |length: usize, id: &str| {
        let mut wrap = event(&id.repeat(32), RECIPIENT);
        let mut tags = vec![json!(["p", RECIPIENT])];
        tags.extend((0..18).map(|_| json!(["x", "y".repeat(500)])));
        wrap["tags"] = json!(tags);
        wrap["content"] = json!("");
        let overhead = frame("EVENT", Some(wrap.clone())).len();
        assert!(length - overhead <= 131_070);
        wrap["content"] = json!("c".repeat(length - overhead));
        let text = frame("EVENT", Some(wrap));
        assert_eq!(text.len(), length);
        text
    };
    let mut sync = judge(&["wss://relay.team.example"]);
    sync.accept_frame(0, &sized(140_001, "01"));
    sync.accept_frame(0, &sized(140_000, "02"));
    sync.accept_frame(0, &frame("EOSE", None));
    let ids: Vec<String> = sync
        .candidates()
        .unwrap()
        .into_iter()
        .map(|c| c.id)
        .collect();
    assert_eq!(ids, ["02".repeat(32)]);
}

#[test]
fn every_relay_failing_is_unavailable_and_order_is_newest_first() {
    let mut sync = judge(&DEFAULTS);
    sync.relay_ended(0);
    sync.relay_ended(1);
    assert!(sync.candidates().is_err());

    let mut sync = judge(&["wss://relay.team.example"]);
    for (id, at) in [("0b", 5), ("0a", 5), ("0c", 9)] {
        let mut wrap = event(&id.repeat(32), RECIPIENT);
        wrap["created_at"] = json!(at);
        sync.accept_frame(0, &frame("EVENT", Some(wrap)));
    }
    sync.accept_frame(0, &frame("EOSE", None));
    let ids: Vec<String> = sync
        .candidates()
        .unwrap()
        .into_iter()
        .map(|c| c.id[..2].to_string())
        .collect();
    assert_eq!(ids, ["0c", "0a", "0b"]);
}

// A later page that fails before delivering anything fails the relay, earlier pages included:
// `unseenRelayEvents` rejects, and `Promise.allSettled` drops the relay.
#[test]
fn a_later_page_failing_empty_fails_the_whole_relay() {
    let seen: HashSet<String> = (0..50u64).map(|n| format!("{n:064x}")).collect();
    let relays = vec!["wss://relay.team.example".to_string()];
    let mut sync = sync_nostr_gift_wraps(RECIPIENT, 1, relays, seen, LIMITS).unwrap();
    let mut step = RelayStep::Continue;
    for n in 0..50u64 {
        let mut wrap = event(&format!("{n:064x}"), RECIPIENT);
        wrap["created_at"] = json!(100 - n);
        step = sync.accept_frame(0, &frame("EVENT", Some(wrap)));
    }
    let RelayStep::NextPage { request, .. } = step else {
        panic!("a full page of seen wraps asks for the next one: {step:?}");
    };
    let request: Value = serde_json::from_str(&request).unwrap();
    assert_eq!(request[1], "mcw-2");
    assert_eq!(request[2]["until"], 51);
    // The first page's id is closed; its frames are now ignored.
    assert_eq!(
        sync.accept_frame(0, &frame("EOSE", None)),
        RelayStep::Continue
    );
    sync.relay_ended(0);
    assert_eq!(sync.candidates(), Err(RelayError::RelaysUnavailable));
}

#[test]
fn subscription_ids_count_in_base_36() {
    assert_eq!(subscription_id(1), "mcw-1");
    assert_eq!(subscription_id(35), "mcw-z");
    assert_eq!(subscription_id(36), "mcw-10");
}
