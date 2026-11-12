//! The core's privacy read against the extension's, from `extension-privacy-wire.json` (written
//! by `bun run mobile:parity` from the extension's own code): viewing keys, routing, the exact
//! discovery bodies and every judged reply, over Bramble's routing (`privacy.json`). The gates
//! are `privacy_gates_parity.rs`.

mod portfolio_support;
mod privacy_support;

use privacy_support::{
    read_routing, routes, run_mainnet, steady, wire, Script, PHRASE, ROUTING_JSON,
};
use serde_json::Value;
use starknet_wallet_core::accounts::Ladder;
use starknet_wallet_core::privacy::config::route_for;
use starknet_wallet_core::privacy::types::{PrivacyPurpose, PrivacyRegistration};
use starknet_wallet_core::{account_key, derive_strk20_viewing_key, Felt, NetworkId, SecretFelt};

fn text(value: &Value) -> &str {
    value.as_str().unwrap()
}

/// The scoped viewing key of the parity phrase's account `index`, as canonical hex (public test
/// keys only).
fn strk20_viewing_key(index: u32, chain: &str, pool: &str) -> Option<String> {
    let key = account_key(PHRASE, None, Ladder::Account, index).ok()?;
    let viewing_key = derive_strk20_viewing_key(&key, chain, pool).ok()?;
    Some(viewing_key.expose_secret_hex().to_string())
}

#[test]
fn viewing_keys_match_the_extension_krusty_path() {
    let wire = wire();
    let keys = &wire["viewingKeys"];
    let (chain, pool) = (text(&keys["chainId"]), text(&keys["pool"]));
    for account in keys["accounts"].as_array().unwrap() {
        let index = account["accountIndex"].as_u64().unwrap() as u32;
        let derived = strk20_viewing_key(index, chain, pool).unwrap();
        assert_eq!(derived, text(&account["viewingKey"]), "account {index}");
    }
    // Krusty's own vectors, through the core's wrapper (this crate never names Krusty).
    for vector in keys["crate"].as_array().unwrap() {
        let private_key = SecretFelt::new(Felt::from_hex(text(&vector["privateKey"])).unwrap());
        let key = derive_strk20_viewing_key(
            &private_key,
            text(&vector["chainId"]),
            text(&vector["pool"]),
        )
        .unwrap();
        assert_eq!(
            key.expose_secret_hex().as_str(),
            text(&vector["viewingKey"])
        );
    }
    for refusal in keys["refusals"].as_array().unwrap() {
        assert!(refusal["refused"].as_bool().unwrap());
        assert!(strk20_viewing_key(0, chain, text(&refusal["pool"])).is_none());
    }
}

#[test]
fn routing_matches_and_ohttp_is_never_routed() {
    let wire = wire();
    let routing = &wire["routing"];
    assert_eq!(text(&routing["bundled"]), ROUTING_JSON);
    let routed = |text: &str| -> Vec<NetworkId> {
        let (routes, _) = read_routing(text);
        [NetworkId::Mainnet, NetworkId::Sepolia]
            .into_iter()
            .filter(|network| route_for(&routes, *network).is_some())
            .collect()
    };
    assert_eq!(routed(text(&routing["bundled"])), vec![NetworkId::Mainnet]);
    for variant in ["ohttpEnabled", "ohttpRelay"] {
        assert_eq!(routed(text(&routing[variant])), vec![], "{variant}");
        assert!(
            !text(&routing[variant]).contains("discoveryUrl"),
            "{variant}"
        );
    }
    assert!(route_for(&routes(), NetworkId::Sepolia).is_none());
}

fn case(wire: &Value, id: &str) -> Value {
    wire["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == id)
        .unwrap()
        .clone()
}

#[test]
fn discovery_bodies_are_byte_identical_to_the_sdk() {
    let wire = wire();
    let run = run_mainnet(
        Script::from_case(&case(&wire, "three-pages")["script"]),
        &mut steady,
    );
    let capture = wire["capture"].as_array().unwrap();
    let posts: Vec<_> = capture.iter().filter(|r| r["kind"] != "rpc").collect();
    assert_eq!(run.posts.len(), posts.len());
    for ((url, body, _), expected) in run.posts.iter().zip(&posts) {
        assert_eq!(url, text(&expected["url"]));
        assert_eq!(std::str::from_utf8(body).unwrap(), text(&expected["body"]));
    }
    let fee = capture.iter().find(|r| r["kind"] == "rpc").unwrap();
    let (body, _) = run
        .rpcs
        .iter()
        .find(|(_, p)| *p == PrivacyPurpose::PoolFee)
        .unwrap();
    let sent: Value = serde_json::from_str(body).unwrap();
    assert_eq!(sent["method"], "starknet_call");
    assert_eq!(sent["params"], fee["params"]);
}

#[test]
fn every_judged_reply_reaches_the_extension_outcome() {
    let wire = wire();
    for entry in wire["cases"].as_array().unwrap() {
        let id = text(&entry["id"]);
        let run = run_mainnet(Script::from_case(&entry["script"]), &mut steady);
        let outcome = run.outcome.clone().unwrap();
        if let Some(deviation) = entry.get("mobile") {
            assert_eq!(
                format!("{:?}", outcome.unwrap_err()),
                text(deviation),
                "{id}"
            );
            continue;
        }
        let expected = &entry["outcome"];
        if expected.get("error").is_some() {
            assert!(
                outcome.is_err(),
                "{id}: the extension failed, the core gave {outcome:?}"
            );
        } else {
            let state = outcome.unwrap_or_else(|f| panic!("{id}: the core failed {f:?}"));
            let registered = text(&expected["registration"]) == "registered";
            assert_eq!(
                state.registration == PrivacyRegistration::Registered,
                registered,
                "{id}"
            );
            assert_eq!(state.pool_fee_fri, text(&expected["poolFeeFri"]), "{id}");
            let balances = expected["balances"].as_array().unwrap();
            assert_eq!(state.balances.len(), balances.len(), "{id}");
            for (mine, theirs) in state.balances.iter().zip(balances) {
                assert_eq!(mine.token_address, text(&theirs["token"]), "{id}");
                assert_eq!(mine.balance, text(&theirs["balance"]), "{id}");
            }
        }
        let requests = &entry["requests"];
        assert_eq!(
            run.count(PrivacyPurpose::PreflightCheck) as u64,
            requests["preflight"].as_u64().unwrap(),
            "{id}"
        );
        assert_eq!(
            run.count(PrivacyPurpose::IncomingState) as u64,
            requests["incoming"].as_u64().unwrap(),
            "{id}"
        );
        assert_eq!(
            run.count(PrivacyPurpose::PoolFee) as u64,
            requests["fee"].as_u64().unwrap(),
            "{id}"
        );
        let pauses: Vec<u32> = entry["pauses"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| {
                assert!(
                    p["retryAfterMs"].is_null(),
                    "{id}: the SDK error carries no Retry-After"
                );
                p["attempt"].as_u64().unwrap() as u32 * 1_000
            })
            .collect();
        assert_eq!(run.waits, pauses, "{id}");
    }
}
