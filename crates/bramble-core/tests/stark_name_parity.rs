//! Starknet name → address (P5) against what the extension's own code produced
//! (`fixtures/extension-stark-name.jsonl`, `bun run mobile:parity`): which typed names it looks
//! up and how it normalises them, the `domain_to_address` call it sends through starknet.js for
//! each on Mainnet, and how it reads one endpoint's reply. The extension's per-endpoint outcome
//! maps onto `StarkNameAnswer` with one endpoint configured: `unreachable` (the reply failed over
//! and none was left) is `TryNextEndpoint`, and `threw` (the extension fails closed) is `Failed`.
#![cfg(feature = "mobile")]

use bramble_core::{
    stark_name_address_request, stark_name_judge_address, stark_name_normalize, NetworkId,
    StarkNameAnswer,
};
use serde_json::Value;

fn cases(block: &str) -> Vec<Value> {
    let found: Vec<Value> = include_str!("fixtures/extension-stark-name.jsonl")
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

#[test]
fn names_normalise_and_send_the_extensions_call() {
    for case in cases("name") {
        let name = text(&case["name"]);
        let input = text(&case["input"]).to_string();
        assert_eq!(
            stark_name_normalize(input.clone()).as_deref(),
            case["normalized"].as_str(),
            "{name}"
        );
        let request = &case["request"];
        // The id is starknet.js's own counter; the core takes the host's.
        let id = request["id"].as_u64().unwrap_or(2);
        let built = stark_name_address_request(id, NetworkId::Mainnet, input);
        if request.is_null() {
            // Refused before any call: by the background's input reader, by
            // `normalizeStarkName`, or by the encoder's field bound.
            assert!(built.is_err(), "{name}");
        } else {
            let body: Value = serde_json::from_str(&built.unwrap()).unwrap();
            assert_eq!(&body, request, "{name}");
        }
    }
}

#[test]
fn replies_are_judged_as_the_extension_reads_them() {
    for case in cases("reply") {
        let name = text(&case["name"]);
        let outcome = &case["outcome"];
        let expected = match text(&outcome["status"]) {
            "resolved" => StarkNameAnswer::Resolved {
                address: text(&outcome["address"]).into(),
            },
            "not-found" => StarkNameAnswer::NotFound,
            "unsupported-network" => StarkNameAnswer::UnsupportedNetwork,
            "unreachable" => StarkNameAnswer::TryNextEndpoint,
            "threw" => StarkNameAnswer::Failed,
            other => panic!("{name}: {other}"),
        };
        let id = case["id"].as_u64().unwrap();
        assert_eq!(
            stark_name_judge_address(text(&case["reply"]).into(), id),
            expected,
            "{name}"
        );
    }
}
