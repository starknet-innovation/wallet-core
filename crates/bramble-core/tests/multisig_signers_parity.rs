//! Changing an Argent multisig's signers and reviewing a multisig's calls, against the extension:
//! `extension-multisig-transactions-signers.json` (`argentMultisigConfigChangeCall`,
//! `parseArgentMultisigConfigChange`, `nextArgentMultisigPublicConfig` and
//! `reviewArgentMultisigCalls` on the fixture's 2-of-3 Sepolia account), the `calls` refusals of
//! `extension-multisig-transactions-refusals.json` (`requireSafeArgentMultisigCalls`), and the
//! proposal's transfer in `extension-multisig-transactions.json`, whose summary is the stored
//! intent's. Each signer change is also prepared through `ProposePlan` over the case's recorded
//! node exchanges, reproducing the extension's frozen transaction. The generator reviews with a fresh wallet: no contacts, no other accounts, no paid
//! recipients, so `trusted` is empty and the known tokens are the network's catalog tokens.
//!
//! Every refusal's `name` is the extension's `WalletError`; the typed error stands in for it and
//! its `Display` is the fixture's `message`.

mod multisig_support;

use bramble_core::catalogs::networks;
use bramble_core::MULTISIG_FEE_OVERHEAD;
use multisig_support::{class, current_config, decode, drive, summary_length};
use serde_json::Value;
use starknet_wallet_core::accounts::multisig_change::{
    config_change_call, next_public_config, parse_config_change, ConfigChange,
};
use starknet_wallet_core::accounts::multisig_records::{PublicConfig, Record};
use starknet_wallet_core::send::call_decode::Known;
use starknet_wallet_core::send::multisig_invoke::{MultisigCall, PreparedInvoke};
use starknet_wallet_core::send::multisig_invoke_plan::{MultisigStep, ProposePlan};
use starknet_wallet_core::send::multisig_review::{require_safe_calls, review_calls};
use starknet_wallet_core::send::types::StepReply;
use starknet_wallet_core::NetworkId;

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn of<T: serde::de::DeserializeOwned>(value: &Value) -> T {
    serde_json::from_value(value.clone()).unwrap()
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
    let record: Record = of(account);
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
        let call: MultisigCall = of(&case["call"]);
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
        assert_eq!(
            prepared(&record, &call, case),
            decode(&case["prepared"]),
            "{name}"
        );
    }
}

/// `ProposePlan` over the case's recorded exchanges, the self-call as its one call.
fn prepared(record: &Record, call: &MultisigCall, case: &Value) -> Value {
    let class = class();
    let config = current_config(record);
    let calls = std::slice::from_ref(call);
    let mut plan = ProposePlan::new(record, calls, &config, &class, MULTISIG_FEE_OVERHEAD).unwrap();
    let first = plan.advance(StepReply::Start).unwrap();
    let exchanges: Vec<Value> = case["prepare"]
        .as_array()
        .unwrap()
        .iter()
        .map(decode)
        .collect();
    let last = drive(first, &exchanges, |reply| plan.advance(reply).unwrap());
    let MultisigStep::Prepared(prepared) = last else {
        panic!("{last:?}");
    };
    let prepared: PreparedInvoke = *prepared;
    assert!(prepared.is_valid());
    serde_json::to_value(&prepared).unwrap()
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
        let call: MultisigCall = of(&case["input"]);
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
        let calls: Vec<MultisigCall> = of(&case["input"]);
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
    let entries = flows["networks"].as_array().unwrap();
    assert_eq!(entries.len(), 2);
    for entry in entries {
        let network = NetworkId::from_catalog_id(entry["networkId"].as_str().unwrap()).unwrap();
        let account = &entry["account"];
        let address = account["address"].as_str().unwrap();
        let current: PublicConfig = of(&account["currentConfig"]);
        let calls: Vec<MultisigCall> = of(&entry["proposal"]["calls"]);
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
