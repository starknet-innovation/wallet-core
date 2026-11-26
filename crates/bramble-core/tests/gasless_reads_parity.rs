//! G7h's wire and reader blocks (`fixtures/extension-gasless.jsonl`): how `postGasless` judges each
//! paymaster reply as a quote and as an execute, with its screening repeats; the build params;
//! the readers of each result; the offer and account gates; and the fee ceiling's conversion.
//! Cases the core's types cannot express (a negative or fractional number where it takes an
//! unsigned integer, an extra key on a record) are asserted as such, never skipped silently.

use crate::gasless_support;

use bramble_core::gasless::{gasless_v2_classes, GASLESS_POLICY};
use gasless_support::*;
use serde_json::{json, Value};
use starknet_wallet_core::accounts::AccountFamily;
use starknet_wallet_core::gasless::{
    convert_strk_fee_to_token, read_available, read_quote, read_supported_tokens,
    read_transaction_hash, signs_gasless_v2, Exchange, Exchanged, GaslessAccount, GaslessCode,
    GaslessKind, GaslessSession, SubmitRound, SubmitStep,
};
use starknet_wallet_core::js::json as js;
use starknet_wallet_core::portfolio::wire::{HttpMethod, HttpRequest};
use starknet_wallet_core::send::u256::U256;
use starknet_wallet_core::{felt, NetworkId};

fn js_value(value: &Value) -> js::JsValue {
    js::parse(&value.to_string()).unwrap()
}

fn plain(value: &js::JsValue) -> Value {
    serde_json::from_str(&value.to_json()).unwrap()
}

#[test]
fn each_reply_is_judged_as_the_extension_judged_it() {
    let policy = &GASLESS_POLICY;
    for case in cases("wire", 61) {
        let executes = case["executes"] == true;
        let request = HttpRequest {
            method: HttpMethod::Post,
            url: "https://wallet.nodes.starknet.org/api/paymaster/mainnet".into(),
            headers: Vec::new(),
            body: None,
            max_body_bytes: policy.max_body_bytes,
            deadline_ms: policy.deadline_ms,
        };
        let mut exchange = Exchange::new(request, executes);
        let (mut posted, mut slept) = (0, Vec::new());
        let replies = case["replies"].as_array().unwrap();
        let answer = loop {
            posted += 1;
            match exchange.judge(
                &outcome(&replies[posted - 1], policy.max_body_bytes),
                policy,
            ) {
                Exchanged::Repeat { delay_ms } => {
                    slept.push(delay_ms);
                    // The host's lock check before an execute's repeat (`isAuthorized`).
                    if executes && name(&case).contains("the wallet locks during the pause") {
                        break json!({"refused": locked()});
                    }
                }
                Exchanged::Settled(Ok(result)) => break json!({"result": plain(&result)}),
                Exchanged::Settled(Err(error)) => break json!({"refused": thrown(&error)}),
            }
        };
        assert_eq!(answer, case["answer"], "{}", name(&case));
        assert_eq!(json!(slept), case["slept"], "{}", name(&case));
        assert_eq!(
            posted,
            case["sent"].as_array().unwrap().len(),
            "{}",
            name(&case)
        );
    }
}

const ARGENT_04: &str = "0x036078334509b514626504edc9fb252328d1a240e4e948bef8d0c08dff45927f";

/// `gaslessBuildParams` as the core posts it: the `params` of the build a submission starts with,
/// for a deployed Argent 0.4 account on Mainnet.
fn build_params(input: &Value) -> Result<Value, Value> {
    let refused = |code: GaslessCode| thrown(&code.into());
    let account = GaslessAccount {
        network: NetworkId::Mainnet,
        address: felt_at(&input["userAddress"]),
        family: AccountFamily::Argent,
        class_hash: felt::parse(ARGENT_04).unwrap(),
        deployed: true,
        deployment: None,
    };
    let calls = calls(&input["calls"]).unwrap();
    let session = GaslessSession::open(&GASLESS_POLICY, &account, &calls, &gasless_v2_classes())
        .map_err(refused)?;
    let reviewed = offer(&json!({
        "amount": "1", "decimals": 6, "label": "T", "tokenAddress": input["gasToken"],
    }));
    let now = input["nowSeconds"].as_u64().unwrap();
    let (_, step) = SubmitRound::start(session, &reviewed, Some(U256::from_u128(1)), now)
        .map_err(|error| thrown(&error))?;
    let SubmitStep::Post { request, .. } = step else {
        panic!("a build is posted first");
    };
    Ok(sent(&request)["body"]["params"].clone())
}

#[test]
fn the_build_params_are_the_extensions() {
    for case in cases("buildParams", 10) {
        let input = &case["input"];
        match name(&case).as_str() {
            // The core takes the clock as an unsigned integer: neither can reach it.
            "a negative clock" | "a fractional clock" => {
                assert!(input["nowSeconds"].as_u64().is_none());
                let code = GaslessCode::Unavailable;
                assert_eq!(case["refused"], thrown(&code.into()));
            }
            // knownExtensionBug: the extension refuses with starknet.js's range error. The core
            // cannot hold the felt at all, so the call never reaches a build.
            "a calldata felt above the field" => {
                assert!(case["knownExtensionBug"].is_string() && case["refused"].is_object());
                assert!(calls(&input["calls"]).is_none());
            }
            // The core derives a deployment's address from its class, salt and calldata, where
            // the extension compares the stored `contractAddress`; neither made-up deployment
            // derives the account, so the core refuses both (Mainnet refuses any deployment
            // before that: `screening_subject_unverifiable`).
            "deploy and invoke" | "a deployment for another address" => {
                // The first is given the built body (`gaslessDeploymentBody`), the second the
                // record it is built from; both are the generator's one `DEPLOYMENT`.
                let given = &input["deployment"];
                let deployment = match given.get("addressSalt") {
                    Some(_) => deployment(given).unwrap(),
                    None => deployment(&json!({
                        "addressSalt": given["salt"],
                        "classHash": given["class_hash"],
                        "constructorCalldata": given["calldata"],
                    }))
                    .unwrap(),
                };
                assert_ne!(deployment.address(), Some(felt_at(&input["userAddress"])));
                let route = json!({
                    "chainId": "SN_SEPOLIA", "enabled": true, "forwarder": "0x1",
                    "url": "https://wallet.example/api/paymaster/sepolia",
                });
                let policy = policy(&route);
                let account = GaslessAccount {
                    network: NetworkId::Sepolia,
                    address: felt_at(&input["userAddress"]),
                    family: AccountFamily::OpenZeppelin,
                    class_hash: deployment.class_hash,
                    deployed: false,
                    deployment: Some(&deployment),
                };
                let calls = [];
                let opened = GaslessSession::open(&policy, &account, &calls, &[]);
                assert!(opened.is_err(), "{}", name(&case));
            }
            _ => {
                let expected = match case.get("refused") {
                    Some(refused) => Err(refused.clone()),
                    None => Ok(case["output"].clone()),
                };
                assert_eq!(build_params(input), expected, "{}", name(&case));
            }
        }
    }
}

#[test]
fn each_result_is_read_as_the_extension_read_it() {
    for case in cases("supportedTokens", 9) {
        let read = read_supported_tokens(&js_value(&case["input"])).map(|tokens| {
            let rows: Vec<Value> = tokens
                .iter()
                .map(|t| json!({"address": felt::canonical(&t.address), "decimals": t.decimals}))
                .collect();
            Value::Array(rows)
        });
        assert_eq!(json!(read), case["output"], "{}", name(&case));
    }
    for case in cases("available", 4) {
        let read = read_available(&js_value(&case["input"]));
        assert_eq!(json!(read), case["output"], "{}", name(&case));
    }
    for case in cases("quote", 9) {
        let kind = match name(&case).as_str() {
            "a deploy and invoke, asked as one" => GaslessKind::DeployAndInvoke,
            _ => GaslessKind::Invoke,
        };
        let input = js_value(&case["input"]);
        let read = read_quote(&input, kind)
            .map(|(max, typed)| json!({"quotedMax": max.to_decimal(), "typedData": plain(typed)}));
        assert_eq!(json!(read), case["output"], "{}", name(&case));
    }
    for case in cases("transactionHash", 7) {
        let read = read_transaction_hash(&js_value(&case["input"]));
        let expected = case["output"].as_str().and_then(felt::parse);
        assert_eq!(read, expected, "{}", name(&case));
    }
}

#[test]
fn an_offer_and_an_account_pass_the_extensions_gates() {
    let strk = felt::parse(STRK).unwrap();
    for case in cases("offerShape", 6) {
        let input = &case["input"];
        if name(&case) == "an extra key" {
            // The core's offer has no room for a fourth field, so no echo can carry one.
            assert_eq!(case["output"], false);
            assert!(input.as_object().unwrap().contains_key("price"));
            continue;
        }
        let valid = offer(input).is_valid(&strk);
        assert_eq!(json!(valid), case["output"], "{}", name(&case));
    }
    let classes = gasless_v2_classes();
    for case in cases("account", 9) {
        let input = &case["input"];
        // A multisig or read-only account is not an `AccountFamily`: never offered.
        let signs = family(input).is_some_and(|family| {
            signs_gasless_v2(family, &felt_at(&input["classHash"]), &classes)
        });
        assert_eq!(json!(signs), case["output"], "{}", name(&case));
    }
}

#[test]
fn the_fee_ceiling_converts_and_rounds_as_the_extension_does() {
    for case in cases("ceiling", 13) {
        let input = &case["input"];
        let label = name(&case);
        if let Some(reference) = input.as_str() {
            let Some(reference) = U256::from_decimal(reference) else {
                // `gaslessFeeCeiling(-5n)`: the core's reference is unsigned, and the
                // extension's non-positive answer is the core's zero, which refuses every quote.
                assert_eq!(
                    (label.as_str(), &case["output"]),
                    ("the ceiling over -5", &json!("0"))
                );
                continue;
            };
            let ceiling = GASLESS_POLICY.fee_ceiling(reference).to_decimal();
            assert_eq!(json!(ceiling), case["output"], "{label}");
            continue;
        }
        let number = |key: &str| U256::from_decimal(input[key].as_str().unwrap()).unwrap();
        let price = |key: &str| u64::try_from(number(key).low).unwrap();
        // A decimal count is a `u8`: a fraction cannot be one, so it converts to nothing.
        let decimals = |key: &str| {
            let count = input[key].as_f64().unwrap();
            (count.fract() == 0.0 && count <= 255.0).then_some(count as u8)
        };
        let converted = decimals("strkDecimals")
            .zip(decimals("tokenDecimals"))
            .and_then(|(strk_decimals, token_decimals)| {
                convert_strk_fee_to_token(
                    number("strkFee"),
                    price("strkPrice"),
                    price("tokenPrice"),
                    strk_decimals,
                    token_decimals,
                )
            });
        let converted = converted.map(U256::to_decimal);
        assert_eq!(json!(converted), case["output"], "{label}");
    }
}

/// Every line of the fixture is one of the blocks the two parity files read, at the counts they
/// assert.
#[test]
fn every_line_belongs_to_a_block_that_is_read() {
    let blocks = [
        ("hash", 14),
        ("review", 57),
        ("offers", 14),
        ("wire", 61),
        ("buildParams", 10),
        ("supportedTokens", 9),
        ("available", 4),
        ("quote", 9),
        ("transactionHash", 7),
        ("offerShape", 6),
        ("account", 9),
        ("ceiling", 13),
    ];
    let lines = include_str!("fixtures/extension-gasless.jsonl")
        .lines()
        .count();
    assert_eq!(blocks.iter().map(|(_, n)| n).sum::<usize>(), lines);
    for (block, count) in blocks {
        cases(block, count);
    }
}
