//! Custom RPC endpoints (`src/rpc_override.rs`), as the extension keeps them: `rpcOverrides` in
//! the preferences, one per network, tried before the bundled endpoints, which stay as failover.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

mod common;

use bramble_core::catalogs::networks;
use bramble_core::{supported_networks, CoreError, NetworkId, TokenPreferences};
use common::{core, imported, open, PASSWORD};
use serde_json::{json, Value};

const CUSTOM: &str = "https://rpc.example/v0_10/apiKEY?token=a%2Fb";

fn preferences(dir: &std::path::Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(dir.join("preferences.json")).unwrap()).unwrap()
}

#[test]
fn with_nothing_stored_the_bundled_networks_are_unchanged() {
    let (dir, _clock, core) = imported();
    assert_eq!(core.networks().unwrap(), supported_networks());
    assert_eq!(core.rpc_override(NetworkId::Mainnet).unwrap(), None);
    assert_eq!(core.rpc_override(NetworkId::Sepolia).unwrap(), None);
    assert_eq!(preferences(dir.path())["rpcOverrides"], json!({}));
}

#[test]
fn a_stored_override_is_the_first_endpoint_and_survives_a_restart() {
    let (dir, clock, core) = imported();
    let saved = core
        .set_rpc_override(NetworkId::Mainnet, format!("  {CUSTOM}\n"))
        .unwrap();
    assert_eq!(saved.as_deref(), Some(CUSTOM));
    assert_eq!(
        preferences(dir.path())["rpcOverrides"],
        json!({ "SN_MAIN": CUSTOM })
    );

    core.lock();
    let reopened = open(dir.path(), &clock);
    reopened.unlock(PASSWORD.into()).unwrap();
    assert_eq!(
        reopened
            .rpc_override(NetworkId::Mainnet)
            .unwrap()
            .as_deref(),
        Some(CUSTOM)
    );
    let listed = reopened.networks().unwrap();
    let mainnet = listed
        .iter()
        .find(|info| info.network == NetworkId::Mainnet)
        .unwrap();
    assert_eq!(
        *mainnet,
        networks::with_rpc_override(NetworkId::Mainnet, Some(CUSTOM))
    );
    assert_eq!(mainnet.rpc_urls[0], CUSTOM);
    assert_eq!(
        mainnet.rpc_urls[1..],
        networks::info(NetworkId::Mainnet).rpc_urls[..]
    );
    assert_eq!(
        mainnet.chain_id,
        networks::info(NetworkId::Mainnet).chain_id
    );
    // The other network keeps its defaults.
    let sepolia = listed
        .iter()
        .find(|info| info.network == NetworkId::Sepolia)
        .unwrap();
    assert_eq!(sepolia, networks::info(NetworkId::Sepolia));

    // Empty clears it, as the extension's reset.
    assert_eq!(
        reopened
            .set_rpc_override(NetworkId::Mainnet, " ".into())
            .unwrap(),
        None
    );
    assert_eq!(reopened.networks().unwrap(), supported_networks());
    assert_eq!(preferences(dir.path())["rpcOverrides"], json!({}));
}

#[test]
fn a_refused_endpoint_changes_nothing_and_names_no_url() {
    let (dir, _clock, core) = imported();
    core.set_rpc_override(NetworkId::Sepolia, CUSTOM.into())
        .unwrap();
    for refused in [
        "http://127.0.0.1:5050/rpc",
        "https://user:apiKEY@rpc.example",
        "https://RPC.example/apiKEY",
    ] {
        match core.set_rpc_override(NetworkId::Sepolia, refused.into()) {
            Err(CoreError::InvalidInput { reason }) => {
                assert!(!reason.contains("apiKEY") && !reason.contains("example"));
            }
            other => panic!("{refused}: {other:?}"),
        }
    }
    assert_eq!(
        core.rpc_override(NetworkId::Sepolia).unwrap().as_deref(),
        Some(CUSTOM)
    );
    assert_eq!(
        preferences(dir.path())["rpcOverrides"],
        json!({ "SN_SEPOLIA": CUSTOM })
    );
}

#[test]
fn overrides_need_an_unlocked_wallet() {
    let (_dir, _clock, core) = core();
    assert!(matches!(core.networks(), Err(CoreError::NoWallet)));
    let (_dir, _clock, core) = imported();
    core.lock();
    assert!(matches!(
        core.set_rpc_override(NetworkId::Mainnet, CUSTOM.into()),
        Err(CoreError::Locked)
    ));
    assert!(matches!(
        core.rpc_override(NetworkId::Mainnet),
        Err(CoreError::Locked)
    ));
}

// `explorerFallbackAllowed`: with no stored choice, off on a network with an override; the core
// reads the override itself, so a host cannot pass the wrong answer.
#[test]
fn the_explorer_fallback_follows_the_stored_override() {
    let (_dir, _clock, core) = imported();
    let unset = TokenPreferences::new(None);
    assert!(core.explorer_fallback_allowed(NetworkId::Mainnet, unset.clone()));
    core.set_rpc_override(NetworkId::Mainnet, CUSTOM.into())
        .unwrap();
    assert!(!core.explorer_fallback_allowed(NetworkId::Mainnet, unset.clone()));
    assert!(core.explorer_fallback_allowed(NetworkId::Sepolia, unset.clone()));
    let chosen = TokenPreferences::new(Some(
        json!({ "schema": "bramble.mobile.portfolio.v1", "tokens": [], "hiddenTokens": [],
                "displayCurrency": "usd", "explorerFallback": { "SN_MAIN": true } })
        .to_string(),
    ));
    assert!(core.explorer_fallback_allowed(NetworkId::Mainnet, chosen.clone()));
    core.lock();
    assert!(!core.explorer_fallback_allowed(NetworkId::Mainnet, chosen));
    assert!(!core.explorer_fallback_allowed(NetworkId::Sepolia, unset));
}

#[test]
fn an_invalid_stored_override_fails_closed() {
    for overrides in [
        json!({ "SN_MAIN": "http://127.0.0.1:5050/rpc" }),
        json!({ "SN_MAIN": "https://RPC.example" }),
        json!({ "SN_DEVNET": "https://rpc.example" }),
        json!({ "SN_MAIN": 1 }),
    ] {
        let (dir, clock, core) = imported();
        core.lock();
        let path = dir.path().join("preferences.json");
        let mut stored = preferences(dir.path());
        stored["rpcOverrides"] = overrides.clone();
        std::fs::write(&path, stored.to_string()).unwrap();
        let reopened = open(dir.path(), &clock);
        assert!(
            matches!(
                reopened.unlock(PASSWORD.into()),
                Err(CoreError::Storage { .. })
            ),
            "{overrides}"
        );
    }
}
