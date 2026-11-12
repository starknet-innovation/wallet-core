//! Publish and relay-selection tests, moved from mobile's `nostr_relay_tests.rs`. Each mirrors a
//! case in the extension's `tests/background/multisig/nostr-relay.test.ts` (named above it).

use super::*;
use serde_json::json;

const RECIPIENT: &str = "4444444444444444444444444444444444444444444444444444444444444444";
const DEFAULTS: [&str; 2] = ["wss://relay.primal.net", "wss://nos.lol"];

fn event(id: &str, recipient: &str) -> Value {
    json!({"content":"ciphertext","created_at":1,"id":id,"kind":1059,
           "pubkey":"22".repeat(32),"sig":"33".repeat(64),"tags":[["p",recipient]]})
}

fn urls(relays: &[&str]) -> Vec<String> {
    relays.iter().map(|relay| relay.to_string()).collect()
}

// "rejects insecure, duplicate, or empty relay selections"
#[test]
fn rejects_insecure_duplicate_or_empty_relay_selections() {
    for relays in [
        urls(&["ws://relay.example"]),
        urls(&[]),
        urls(&[DEFAULTS[0], DEFAULTS[0]]),
        urls(&[DEFAULTS[0], DEFAULTS[1], "wss://relay.team.example"]),
    ] {
        assert_eq!(
            valid_relay_urls(&relays, 2),
            Err(RelayError::InvalidRelayConfiguration)
        );
    }
    assert_eq!(valid_relay_urls(&urls(&DEFAULTS), 2), Ok(()));
}

// "returns after one durable relay copy and accepts idempotent duplicate acks"
#[test]
fn publish_counts_duplicates_as_accepted_and_fails_when_nothing_landed() {
    let wrap = super::super::event_from_value(&event(&"66".repeat(32), RECIPIENT)).unwrap();
    let mut judge = PublishJudge::new(vec![wrap.clone()]);
    assert!(judge.event_frames()[0].starts_with(r#"["EVENT",{"#));
    let duplicate = json!(["OK", wrap.id, false, "duplicate: already have it"]).to_string();
    assert_eq!(
        judge.accept_frame(&duplicate),
        RelayStep::Finish { close_frame: None }
    );
    assert_eq!(judge.accepted(), [true]);
    let mut refused = PublishJudge::new(vec![wrap.clone()]);
    refused.accept_frame(&json!(["OK", wrap.id, false, "blocked: no"]).to_string());
    assert_eq!(refused.accepted(), [false]);
    assert_eq!(publish_counts(&[vec![true], vec![false]]).unwrap(), [1]);
    assert_eq!(
        publish_counts(&[vec![false], vec![false]]),
        Err(RelayError::RelaysUnavailable)
    );
}

// The first OK for an event decides it; malformed and unrelated frames are ignored.
#[test]
fn the_first_ok_decides_and_other_frames_are_ignored() {
    let wraps: Vec<NostrGiftWrapDelivery> = ["01", "02"]
        .iter()
        .map(|id| NostrGiftWrapDelivery {
            event_json: event(&id.repeat(32), RECIPIENT).to_string(),
            recipient_public_key: RECIPIENT.into(),
        })
        .collect();
    let limits = RelayLimits {
        max_relays: 2,
        max_sync_pages: 4,
    };
    let mut publish = publish_many_nostr_gift_wraps(&wraps, &urls(&DEFAULTS), limits).unwrap();
    assert_eq!(publish.event_frames().len(), 2);
    let ok = |id: &str, accepted: bool| json!(["OK", id.repeat(32), accepted, ""]).to_string();
    for ignored in ["not json", r#"["OK","x",true,"",1]"#, r#"["OK",1,true]"#] {
        assert_eq!(publish.accept_frame(0, ignored), RelayStep::Continue);
    }
    assert_eq!(
        publish.accept_frame(0, &ok("01", true)),
        RelayStep::Continue
    );
    assert_eq!(
        publish.accept_frame(0, &ok("01", false)),
        RelayStep::Continue
    );
    assert_eq!(
        publish.accept_frame(0, &ok("02", false)),
        RelayStep::Finish { close_frame: None }
    );
    // The second relay never answered (it failed or timed out).
    assert_eq!(publish.acknowledgements(), Ok(vec![1, 0]));
    assert_eq!(
        publish.accept_frame(2, &ok("01", true)),
        RelayStep::Finish { close_frame: None }
    );
}
