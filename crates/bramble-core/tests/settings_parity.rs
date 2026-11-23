//! Custom RPC endpoints (P4) against what the extension's own code produced
//! (`fixtures/extension-settings-parity.jsonl`, `bun run mobile:parity`): which typed endpoints
//! `setRpcOverride` stores or refuses, the endpoint order and explorer fallback that follow, how a
//! stored override reads back, and `redactEndpoints`.
//!
//! Deliberate departures (`docs/PLAN.md` P4), asserted as mobile's own below: mobile refuses
//! every `http` endpoint, loopback too, with its own reason; the core has no URL parser, so a
//! stored override the extension would respell to its canonical form is dropped instead (with
//! that network's explorer fallback kept off, the same answer the extension gives with the
//! override); an override under a network mobile does not know fails the read; and a text
//! override no rule takes is dropped (fallback kept off) where the extension fails the read.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

mod common;

use bramble_core::{CoreError, NetworkId, TokenPreferences, WalletCore};
use common::{imported, open, PASSWORD};
use serde_json::{json, Map, Value};
use starknet_wallet_core::nostr::relay_url::redact_endpoints;
use std::sync::Arc;

fn cases(block: &str) -> Vec<Value> {
    let found: Vec<Value> = include_str!("fixtures/extension-settings-parity.jsonl")
        .lines()
        .map(|line| serde_json::from_str(line).expect("a JSON line"))
        .filter(|case: &Value| case["block"] == block)
        .collect();
    assert!(!found.is_empty(), "no {block} cases");
    found
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap()
}

const NETWORKS: [(NetworkId, &str); 2] = [
    (NetworkId::Mainnet, "SN_MAIN"),
    (NetworkId::Sepolia, "SN_SEPOLIA"),
];

fn network(id: &str) -> NetworkId {
    NETWORKS.iter().find(|(_, name)| *name == id).unwrap().0
}

/// A token-preferences record holding `choice` as its stored `explorerFallback`.
fn fallback_choice(choice: &Value) -> Arc<TokenPreferences> {
    if choice.is_null() {
        return TokenPreferences::new(None);
    }
    TokenPreferences::new(Some(
        json!({ "schema": "bramble.mobile.portfolio.v1", "tokens": [], "hiddenTokens": [],
                "displayCurrency": "usd", "explorerFallback": choice })
        .to_string(),
    ))
}

/// Every override the core holds, as the extension's `rpcOverrides` map.
fn overrides(core: &WalletCore) -> Value {
    let mut map = Map::new();
    for (id, name) in NETWORKS {
        if let Some(url) = core.rpc_override(id).unwrap() {
            map.insert(name.into(), url.into());
        }
    }
    Value::Object(map)
}

/// `http:` the extension accepts on loopback; mobile refuses cleartext.
fn is_cleartext(url: &str) -> bool {
    url.trim_start().starts_with("http://")
}

#[test]
fn typed_endpoints_are_stored_or_refused_as_the_extension_does() {
    let (_dir, _clock, core) = imported();
    for case in cases("set") {
        let name = text(&case["name"]);
        let id = network(text(&case["networkId"]));
        let rpc_url = text(&case["rpcUrl"]);
        for (other, _) in NETWORKS {
            core.set_rpc_override(other, String::new()).unwrap();
        }
        if let Some(before) = case["before"].as_object() {
            for (network_id, url) in before {
                core.set_rpc_override(network(network_id), text(url).into())
                    .unwrap();
            }
        }
        let saved = core.set_rpc_override(id, rpc_url.into());
        if name == "an empty fragment" {
            // Left out: `isServiceUrl` refuses a fragment by `url.hash === ""`, which a bare `#`
            // passes, so the extension stores `https://rpc.example/#` against its own rule. The
            // core refuses it; reported rather than mirrored.
            assert!(saved.is_err(), "{name}");
            continue;
        }
        if is_cleartext(rpc_url) {
            // Refused whether or not the extension takes it, with mobile's own reason.
            assert!(
                matches!(&saved, Err(CoreError::InvalidInput { reason }) if reason == "mobile RPC endpoints must use https"),
                "{name}"
            );
            continue;
        }
        match &saved {
            Ok(_) => assert!(case["error"].is_null(), "{name}: stored"),
            Err(CoreError::InvalidInput { reason }) => {
                assert_eq!(Some(reason.as_str()), case["error"].as_str(), "{name}")
            }
            Err(other) => panic!("{name}: {other:?}"),
        }
        assert_eq!(overrides(&core), case["rpcOverrides"], "{name}");
        let listed = core.networks().unwrap();
        let info = listed.iter().find(|info| info.network == id).unwrap();
        assert_eq!(json!(info.rpc_urls), case["rpcUrls"], "{name}");
        assert_eq!(
            core.explorer_fallback_allowed(id, TokenPreferences::new(None)),
            case["explorerFallbackAllowed"] == true,
            "{name}"
        );
    }
}

#[test]
fn the_explorer_fallback_follows_the_extensions_rule() {
    for case in cases("fallback") {
        let name = text(&case["name"]);
        let (_dir, _clock, core) = imported();
        for (network_id, url) in case["rpcOverrides"].as_object().unwrap() {
            core.set_rpc_override(network(network_id), text(url).into())
                .unwrap();
        }
        let id = network(text(&case["networkId"]));
        assert_eq!(
            core.explorer_fallback_allowed(id, fallback_choice(&case["explorerFallback"])),
            case["allowed"] == true,
            "{name}"
        );
    }
}

#[test]
fn a_stored_override_reads_back_as_the_extension_reads_it() {
    for case in cases("stored") {
        let name = text(&case["name"]);
        let stored = &case["stored"]["rpcOverrides"];
        let (dir, clock, core) = imported();
        core.lock();
        let path = dir.path().join("preferences.json");
        let mut preferences: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        preferences["rpcOverrides"] = stored.clone();
        std::fs::write(&path, preferences.to_string()).unwrap();
        let core = open(dir.path(), &clock);
        let unlocked = core.unlock(PASSWORD.into());
        if stored.get("SN_DEVNET").is_some() {
            // The extension keeps an override under a network it does not ship; mobile, which
            // knows only Mainnet and Sepolia, fails closed on one (P4).
            assert!(matches!(unlocked, Err(CoreError::Storage { .. })), "{name}");
            continue;
        }
        if !case["error"].is_null() {
            if stored["SN_MAIN"].is_string() {
                // A text endpoint no rule takes: the extension fails the whole preferences read;
                // mobile, whose unlock reads its preferences, drops it with the explorer fallback
                // kept off rather than lock the person out (P4).
                unlocked.unwrap_or_else(|error| panic!("{name}: {error:?}"));
                assert_eq!(
                    core.rpc_override(NetworkId::Mainnet).unwrap(),
                    None,
                    "{name}"
                );
                let unset = TokenPreferences::new(None);
                assert!(
                    !core.explorer_fallback_allowed(NetworkId::Mainnet, unset),
                    "{name}"
                );
            } else {
                assert!(matches!(unlocked, Err(CoreError::Storage { .. })), "{name}");
            }
            continue;
        }
        unlocked.unwrap_or_else(|error| panic!("{name}: {error:?}"));
        let choice = &case["explorerFallback"];
        for (id, network_id) in NETWORKS {
            let read = &case["rpcOverrides"][network_id];
            let held = core.rpc_override(id).unwrap();
            let kept_as_stored = read.is_string() && *read == stored[network_id];
            if read.is_string() && !(kept_as_stored && !is_cleartext(text(read))) {
                // Respelled or cleartext: dropped here (see the module comment).
                assert_eq!(held, None, "{name}: {network_id}");
            } else {
                assert_eq!(held.as_deref(), read.as_str(), "{name}: {network_id}");
            }
            // `explorerFallbackAllowed` over what the extension read.
            let allowed = choice[network_id]
                .as_bool()
                .unwrap_or_else(|| !read.is_string());
            let tokens = fallback_choice(&case["stored"]["explorerFallback"]);
            assert_eq!(
                core.explorer_fallback_allowed(id, tokens),
                allowed,
                "{name}: {network_id}"
            );
        }
    }
}

#[test]
fn failure_text_is_redacted_as_the_extension_redacts_it() {
    for case in cases("redact") {
        assert_eq!(
            redact_endpoints(text(&case["text"])),
            text(&case["redacted"]),
            "{}",
            text(&case["name"])
        );
    }
}
