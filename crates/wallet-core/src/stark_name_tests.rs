//! The extension's own cases for `stark-name.ts`, taken verbatim from
//! `packages/platform/starknet/tests/rpc/stark-name.test.ts` (oracle `a01b240c`, cited by line),
//! then one case per refusal branch of `stark-name.ts` and the code it calls. No Bramble
//! generator writes stark-name fixtures yet, so these stand in for them.

use super::*;

// stark-name.test.ts:22-25, a registered name and the address mainnet answered for it.
const KNOWN_NAME: &str = "fricoben.stark";
const KNOWN_CALLDATA: [&str; 3] = ["1", "1499554868251", "0"];
const KNOWN_ADDRESS: &str = "0x61b6c0a78f9edf13cea17b50719f3344533fadd470b8cb29c2b4318014f52d3";
// stark-name.test.ts:89, the pinned mainnet naming contract.
const MAINNET_CONTRACT: &str = "0x6ac597f8116f886fa1c97a23fa4e08299975ecaf6b598873ca6792b9bbfb678";

fn contract() -> Felt {
    Felt::from_hex(MAINNET_CONTRACT).unwrap()
}

fn request(name: &str) -> Value {
    serde_json::from_str(&address_request(9, name, &contract()).unwrap()).unwrap()
}

/// The request's calldata in decimal, the spelling the oracle's tests compare.
fn decimal_calldata(name: &str) -> Vec<String> {
    request(name)["params"]["request"]["calldata"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| Felt::from_hex(value.as_str().unwrap()).unwrap().to_string())
        .collect()
}

fn reply(result: &str) -> String {
    format!(r#"{{"jsonrpc":"2.0","id":9,"result":{result}}}"#)
}

fn error_reply(error: &str) -> String {
    format!(r#"{{"jsonrpc":"2.0","id":9,"error":{error}}}"#)
}

fn resolved(address: &str) -> StarkNameAnswer {
    StarkNameAnswer::Resolved {
        address: address.into(),
    }
}

#[test]
fn normalize_lower_cases_and_trims_a_suffixed_name() {
    // stark-name.test.ts:50-53
    assert_eq!(
        normalize("Fricoben.stark").as_deref(),
        Some("fricoben.stark")
    );
    assert_eq!(
        normalize("  Fricoben.STARK ").as_deref(),
        Some("fricoben.stark")
    );
}

#[test]
fn normalize_requires_the_suffix_and_rejects_non_names() {
    // stark-name.test.ts:55-67
    assert_eq!(
        normalize("altaga.braavos.stark").as_deref(),
        Some("altaga.braavos.stark")
    );
    for refused in [
        "Fricoben",
        "mom",
        ".stark",
        "",
        "has space.stark",
        &format!("0x{}", "a".repeat(64)),
    ] {
        assert_eq!(normalize(refused), None, "{refused:?}");
    }
}

#[test]
fn normalize_follows_is_stark_domain_label_rules() {
    // isStarkDomain (starknet.js 10.8.0): every label 1-48 characters of a-z, 0-9, '-'.
    let longest = format!("{}.stark", "x".repeat(48));
    assert_eq!(normalize(&longest), Some(longest.clone()));
    assert_eq!(normalize(&format!("{}.stark", "x".repeat(49))), None);
    for accepted in ["-.stark", "a-.stark", "a.b.c.stark", "a.stark.stark"] {
        assert_eq!(normalize(accepted).as_deref(), Some(accepted));
    }
    for refused in ["a..stark", ".a.stark", "a_b.stark", "\u{e9}.stark", "stark"] {
        assert_eq!(normalize(refused), None, "{refused:?}");
    }
    // `trim` and `toLowerCase` as JavaScript has them: U+FEFF is trimmed, the Kelvin sign
    // lower-cases to `k`, and U+0130 to `i` plus a combining dot, which no label holds.
    assert_eq!(
        normalize("\u{feff}a.stark\u{2028}").as_deref(),
        Some("a.stark")
    );
    assert_eq!(normalize("\u{212a}.stark").as_deref(), Some("k.stark"));
    assert_eq!(normalize("\u{130}.stark"), None);
}

#[test]
fn request_sends_the_encoded_domain_to_domain_to_address() {
    // stark-name.test.ts:76-93
    let body = request(KNOWN_NAME);
    assert_eq!(body["method"], "starknet_call");
    assert_eq!(body["params"]["block_id"], "latest");
    assert_eq!(
        body["params"]["request"]["contract_address"],
        MAINNET_CONTRACT
    );
    assert_eq!(
        body["params"]["request"]["entry_point_selector"],
        DOMAIN_TO_ADDRESS_SELECTOR
    );
    assert_eq!(decimal_calldata(KNOWN_NAME), KNOWN_CALLDATA);
    // starknet.js sends the calldata as `CallData.toHex` spells it.
    assert_eq!(
        body["params"]["request"]["calldata"],
        json!(["0x1", "0x15d246f6c1b", "0x0"])
    );
}

#[test]
fn request_encodes_each_label_as_its_own_felt() {
    // stark-name.test.ts:95-116
    assert_eq!(
        decimal_calldata("altaga.braavos.stark"),
        ["2", "2944239886", "55349935767", "0"]
    );
}

#[test]
fn request_encodes_a_final_a_as_digit_37() {
    // useEncoded (starknet.js 10.8.0): "a" is 37, "aa" 0 + 37·38, "ba" 1 + 37·38.
    for (label, encoded) in [("a", "37"), ("aa", "1406"), ("ba", "1407"), ("b", "1")] {
        assert_eq!(
            decimal_calldata(&format!("{label}.stark")),
            ["1", encoded, "0"],
            "{label}"
        );
    }
}

#[test]
fn request_refuses_an_invalid_name_before_any_call() {
    // stark-name.test.ts:224-238 (stark-name.ts:78-81)
    assert!(address_request(9, "not a name", &contract()).is_err());
}

#[test]
fn request_refuses_typed_text_past_128_units() {
    // readStarkNameInput (message-inputs.ts:69,84): the bound is on the text as typed, so even a
    // valid name is refused past it, and padding counts.
    let labels = |first: usize| {
        format!(
            "{}.{}.{}.stark",
            "a".repeat(first),
            "b".repeat(40),
            "c".repeat(40)
        )
    };
    let name = labels(40);
    assert_eq!(utf16_len(&name), 128);
    assert!(address_request(9, &name, &contract()).is_ok());
    let longer = labels(41);
    assert!(normalize(&longer).is_some());
    assert!(address_request(9, &longer, &contract()).is_err());
    let padded = format!("{KNOWN_NAME}{}", " ".repeat(128 - KNOWN_NAME.len() + 1));
    assert!(address_request(9, &padded, &contract()).is_err());
}

#[test]
fn request_refuses_a_label_whose_encoding_passes_the_prime() {
    // stark-name.ts:83: CallData.compile throws "out of felt252 range" for a 48-character label
    // past the prime (values from starknet.js 10.8.0's useEncoded).
    assert_eq!(
        request(&format!("{}t.stark", "-".repeat(47)))["params"]["request"]["calldata"][1],
        "0x7d96dfa72404e05f7c2f72ac18dabc17d5cff4ef661d95189103acf914c1bac"
    );
    for label in [format!("{}u", "-".repeat(47)), "a".repeat(48)] {
        let name = format!("{label}.stark");
        assert!(normalize(&name).is_some());
        assert!(address_request(9, &name, &contract()).is_err(), "{label}");
    }
}

#[test]
fn judge_resolves_a_nonzero_address() {
    // stark-name.test.ts:71-74
    assert_eq!(
        judge_address(&reply(&format!(r#"["{KNOWN_ADDRESS}"]"#)), 9),
        resolved(KNOWN_ADDRESS)
    );
    // canonicalFelt respells what BigInt takes (stark-name.ts:120).
    assert_eq!(
        judge_address(&reply(r#"["0x00AB", "0x5"]"#), 9),
        resolved("0xab")
    );
    assert_eq!(judge_address(&reply(r#"["171"]"#), 9), resolved("0xab"));
}

#[test]
fn judge_maps_the_zero_answer_to_not_found() {
    // stark-name.test.ts:118-123
    for zero in [r#"["0x0"]"#, r#"["0x00"]"#, r#"["0"]"#] {
        assert_eq!(judge_address(&reply(zero), 9), StarkNameAnswer::NotFound);
    }
}

#[test]
fn judge_reads_a_number_that_is_not_a_field_element_as_not_found() {
    // stark-name.ts:124: canonicalFelt parses these, isNonZeroStarkFieldElement refuses them.
    let prime = "0x800000000000011000000000000000000000000000000000000000000000001";
    for answer in [
        format!(r#"["{prime}"]"#),
        r#"["-5"]"#.into(),
        r#"[""]"#.into(),
    ] {
        assert_eq!(
            judge_address(&reply(&answer), 9),
            StarkNameAnswer::NotFound,
            "{answer}"
        );
    }
}

#[test]
fn judge_fails_over_past_a_non_felt_answer() {
    // stark-name.test.ts:125-143: the first endpoint's non-felt answer fails over, the second
    // endpoint resolves.
    assert_eq!(
        judge_address(&reply(r#"["not-a-felt"]"#), 9),
        StarkNameAnswer::TryNextEndpoint
    );
    assert_eq!(
        judge_address(&reply(&format!(r#"["{KNOWN_ADDRESS}"]"#)), 9),
        resolved(KNOWN_ADDRESS)
    );
}

#[test]
fn judge_fails_over_past_an_unreadable_reply() {
    // stark-name.ts:119-122 (an empty result), and provider.ts:196-205: anything without a
    // numeric JSON-RPC code is a transport fault, so every endpoint failing it is unreachable
    // (stark-name.test.ts:183-192).
    for body in [
        reply("[]"),
        reply("null"),
        error_reply(r#"{"message":"Contract not found"}"#),
        "not json".into(),
        // Stricter than starknet.js, which reads these: another request's id, a result that
        // is not an array, an entry that is not a string.
        format!(r#"{{"jsonrpc":"2.0","id":8,"result":["{KNOWN_ADDRESS}"]}}"#),
        reply(r#""0x5""#),
        reply("[5]"),
    ] {
        assert_eq!(
            judge_address(&body, 9),
            StarkNameAnswer::TryNextEndpoint,
            "{body}"
        );
    }
}

#[test]
fn judge_maps_contract_not_found_to_unsupported_network() {
    // stark-name.test.ts:211-222 (code 20), and rpc-not-found.ts:19's message rule.
    for error in [
        r#"{"code":20,"message":"Contract not found"}"#,
        r#"{"code":-32603,"message":"Internal","data":"Contract not found"}"#,
    ] {
        assert_eq!(
            judge_address(&error_reply(error), 9),
            StarkNameAnswer::UnsupportedNetwork,
            "{error}"
        );
    }
}

#[test]
fn judge_fails_closed_on_another_node_error() {
    // stark-name.test.ts:240-254
    assert_eq!(
        judge_address(
            &error_reply(r#"{"code":99,"message":"unexpected node error"}"#),
            9
        ),
        StarkNameAnswer::Failed
    );
}

#[test]
fn selector_is_starknet_keccak() {
    assert_eq!(
        crate::felt::canonical(&crate::kms::selector("domain_to_address")),
        DOMAIN_TO_ADDRESS_SELECTOR
    );
}
