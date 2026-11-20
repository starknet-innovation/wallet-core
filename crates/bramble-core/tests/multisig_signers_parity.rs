//! Changing an Argent multisig's signers and reviewing a multisig's calls, against the extension:
//! `extension-multisig-transactions-signers.json` (`argentMultisigConfigChangeCall`,
//! `parseArgentMultisigConfigChange`, `nextArgentMultisigPublicConfig` and
//! `reviewArgentMultisigCalls` on the fixture's 2-of-3 Sepolia account), the `calls` refusals of
//! `extension-multisig-transactions-refusals.json` (`requireSafeArgentMultisigCalls`), and the
//! proposal's transfer in `extension-multisig-transactions.json`, whose summary is the stored
//! intent's. The generator reviews with a fresh wallet: no contacts, no other accounts, no paid
//! recipients, so `trusted` is empty and the known tokens are the network's catalog tokens.
//!
//! Every refusal's `name` is the extension's `WalletError`; the typed error stands in for it and
//! its `Display` is the fixture's `message`.

use bramble_core::catalogs::{multisig, networks};
use serde_json::Value;
use starknet_wallet_core::accounts::multisig::ArgentMultisigClass;
use starknet_wallet_core::accounts::multisig_call::ArgentMultisigCall;
use starknet_wallet_core::accounts::multisig_change::{
    config_change_call, next_public_config, parse_config_change, ConfigChange,
};
use starknet_wallet_core::accounts::multisig_records::PublicConfig;
use starknet_wallet_core::send::call_decode::Known;
use starknet_wallet_core::send::multisig_review::{require_safe_calls, review_calls};
use starknet_wallet_core::NetworkId;

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn of<T: serde::de::DeserializeOwned>(value: &Value) -> T {
    serde_json::from_value(value.clone()).unwrap()
}

fn class() -> ArgentMultisigClass {
    multisig::argent_multisig_class().clone()
}

fn summary_length() -> usize {
    multisig::get().limits.summary_length
}

/// The extension's `WalletError` is the only refusal these paths throw.
fn assert_refusal(refusal: &Value, message: String, name: &str) {
    assert_eq!(refusal["name"], "WalletError", "{name}");
    assert_eq!(refusal["message"].as_str().unwrap(), message, "{name}");
}

#[test]
fn signer_changes_build_parse_and_review_as_the_extension_does() {
    let signers = fixture("extension-multisig-transactions-signers.json");
    let account = &signers["account"];
    let address = account["address"].as_str().unwrap();
    let current: PublicConfig = of(&account["currentConfig"]);
    let network = NetworkId::from_catalog_id(account["networkId"].as_str().unwrap()).unwrap();
    let known = Known {
        tokens: &networks::info(network).tokens,
        nft_contracts: &[],
    };
    let cases = signers["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 4);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let change: ConfigChange = of(&case["change"]);
        let call: ArgentMultisigCall = of(&case["call"]);
        assert_eq!(
            config_change_call(address, &change, &class()).unwrap(),
            call,
            "{name}"
        );
        assert_eq!(
            parse_config_change(address, &call, &class()).unwrap(),
            change,
            "{name}"
        );
        let review = review_calls(
            address,
            std::slice::from_ref(&call),
            Some(&current),
            &class(),
            &known,
            &[],
            summary_length(),
        )
        .unwrap();
        assert_eq!(
            serde_json::to_value(&review).unwrap(),
            case["review"],
            "{name}"
        );
        // P1b follow-up: `case["prepare"]` (the RPC exchanges) and `case["prepared"]` (the
        // prepared invoke) belong to the propose machinery (`send/multisig_invoke*`); wire them
        // here once that branch is stacked under this one.
    }
}

#[test]
fn malformed_self_calls_and_invalid_transitions_are_refused_with_the_extensions_message() {
    let signers = fixture("extension-multisig-transactions-signers.json");
    let address = signers["account"]["address"].as_str().unwrap();
    let refusals = &signers["refusals"];
    let current: PublicConfig = of(&refusals["current"]);
    let parse = refusals["parse"].as_array().unwrap();
    assert_eq!(parse.len(), 7);
    for case in parse {
        let call: ArgentMultisigCall = of(&case["input"]);
        let error = parse_config_change(address, &call, &class()).unwrap_err();
        assert_refusal(
            &case["refusal"],
            error.to_string(),
            case["name"].as_str().unwrap(),
        );
    }
    let transitions = refusals["transitions"].as_array().unwrap();
    assert_eq!(transitions.len(), 8);
    for case in transitions {
        let change: ConfigChange = of(&case["input"]);
        let error = next_public_config(&current, &change, &class()).unwrap_err();
        assert_refusal(
            &case["refusal"],
            error.to_string(),
            case["name"].as_str().unwrap(),
        );
    }
}

#[test]
fn unsafe_call_sets_are_refused_with_the_extensions_message() {
    let refusals = fixture("extension-multisig-transactions-refusals.json");
    let address = refusals["account"]["address"].as_str().unwrap();
    let cases = refusals["calls"].as_array().unwrap();
    assert_eq!(cases.len(), 6);
    for case in cases {
        let calls: Vec<ArgentMultisigCall> = of(&case["input"]);
        let error = require_safe_calls(address, &calls, &class()).unwrap_err();
        assert_refusal(
            &case["refusal"],
            error.to_string(),
            case["name"].as_str().unwrap(),
        );
    }
}

#[test]
fn the_proposals_transfer_is_summarised_as_the_stored_intent() {
    let flows = fixture("extension-multisig-transactions.json");
    for entry in flows["networks"].as_array().unwrap() {
        let network = NetworkId::from_catalog_id(entry["networkId"].as_str().unwrap()).unwrap();
        let account = &entry["account"];
        let address = account["address"].as_str().unwrap();
        let current: PublicConfig = of(&account["currentConfig"]);
        let calls: Vec<ArgentMultisigCall> = of(&entry["proposal"]["calls"]);
        let intent: Value =
            serde_json::from_str(entry["proposal"]["stages"][0]["intent"].as_str().unwrap())
                .unwrap();
        let known = Known {
            tokens: &networks::info(network).tokens,
            nft_contracts: &[],
        };
        let review = review_calls(
            address,
            &calls,
            Some(&current),
            &class(),
            &known,
            &[],
            summary_length(),
        )
        .unwrap();
        assert_eq!(review.config, None);
        assert_eq!(review.summary, intent["summary"].as_str().unwrap());
    }
}
