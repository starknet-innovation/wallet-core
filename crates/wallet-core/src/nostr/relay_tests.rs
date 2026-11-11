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
