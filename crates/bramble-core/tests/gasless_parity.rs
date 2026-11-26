//! P14 against what the extension's own gasless code did (G7h, `fixtures/extension-gasless.jsonl`):
//! the SNIP-12 hashes Krusty gives beside starknet.js's, each gateway body checked and submitted
//! (`validateGaslessTypedData`, `submitReviewedGaslessInvoke`: every body posted, the hash
//! signed, the submission or the refusal), and the offer round (`quoteGaslessOffers`). The wire
//! judgements and the pure readers are `gasless_reads_parity.rs`.
//!
//! The core departs from the fixture where it pins an extension defect (`knownExtensionBug`) or
//! where the core is stricter; each departure is asserted as one, never skipped silently.

use crate::gasless_support;

use bramble_core::gasless::{gasless_v2_classes, GASLESS_POLICY};
use gasless_support::*;
use serde_json::{json, Value};
use starknet_wallet_core::accounts::AccountFamily;
use starknet_wallet_core::gasless::{
    validate_typed_data, GaslessAccount, GaslessCode, GaslessHeldToken, GaslessSession, OfferRound,
    RoundStep, SubmitRound, SubmitStep, TypedDataCheck,
};
use starknet_wallet_core::js::json as js;
use starknet_wallet_core::send::u256::U256;
use starknet_wallet_core::{felt, kms, Felt};

fn js_value(value: &Value) -> js::JsValue {
    js::parse(&value.to_string()).unwrap()
}

#[test]
fn krusty_hashes_every_outside_execution_as_starknet_js_does() {
    let mut compared = 0;
    for case in cases("hash", 14) {
        let typed = case["typedData"].to_string();
        for row in case["hashes"].as_array().unwrap() {
            let account = felt_at(&row["accountAddress"]);
            let ours = kms::typed_data_message_hash(&typed, &account);
            assert_eq!(row["agree"], true, "{}", name(&case));
            assert_eq!(
                ours.map(|hash| felt::canonical(&hash)).ok().as_deref(),
                row["starknetJs"].as_str(),
                "{}: {}",
                name(&case),
                row["account"]
            );
            compared += 1;
        }
    }
    assert_eq!(compared, 42);
}

/// The case's account as the gate reads it; `None` for a multisig or read-only account.
fn gasless_account<'d>(
    case: &Value,
    deployment: Option<&'d starknet_wallet_core::send::sign::Deployment>,
) -> Option<GaslessAccount<'d>> {
    let account = &case["account"];
    Some(GaslessAccount {
        network: network(&case["chainId"]),
        address: felt_at(&account["address"]),
        family: family(account)?,
        class_hash: felt_at(&account["classHash"]),
        deployed: account["deployed"] == true,
        deployment,
    })
}

fn reference(case: &Value) -> Option<U256> {
    case["reference"]
        .as_str()
        .map(|text| U256::from_decimal(text).unwrap())
}

/// `validateGaslessTypedData` with the case's inputs, as the generator's `check` records it.
fn check(case: &Value) -> Value {
    let calls = calls(&case["calls"]).unwrap();
    let ceiling = reference(case).map_or(U256::ZERO, |r| GASLESS_POLICY.fee_ceiling(r));
    let quoted = case["quotedMax"].as_str().unwrap().trim_start_matches("0x");
    let input = TypedDataCheck {
        account: felt_at(&case["account"]["address"]),
        network: network(&case["chainId"]),
        calls: &calls,
        forwarder: felt_at(&case["checkForwarder"]),
        gas_token: felt_at(&case["reviewed"]["tokenAddress"]),
        quoted_max: U256::from_hex(quoted).unwrap(),
        ceiling,
        now: case["nowSeconds"].as_u64().unwrap(),
        minimum_remaining: GASLESS_POLICY.sign_window_seconds,
    };
    match validate_typed_data(&js_value(&case["typedData"]), &input, &GASLESS_POLICY) {
        Ok(accepted) => json!({
            "caller": felt::canonical(&accepted.caller),
            "executeAfter": format!("{:#x}", accepted.execute_after),
            "executeBefore": format!("{:#x}", accepted.execute_before),
            "feeAmount": accepted.fee_amount.to_decimal(),
            "ok": true,
        }),
        Err(reason) => json!({"ok": false, "reason": format!("{reason:?}").to_lowercase()}),
    }
}

/// `serializeAccountSignature` of the generator's fixed signer `(r, s) = (1, 2)`, key `0x3`.
fn signature(family: AccountFamily) -> Vec<Felt> {
    let felts: &[u8] = match family {
        AccountFamily::Argent => &[1, 0, 3, 1, 2],
        _ => &[1, 2],
    };
    felts.iter().map(|felt| Felt::from(*felt)).collect()
}

/// `submitReviewedGaslessInvoke` through the core's machine, the host's half played as the
/// generator scripted it: every body posted, the hash asked to be signed, and the outcome.
fn submit(case: &Value) -> Value {
    let policy = policy(&case["route"]);
    let deployment = (!case["deployment"].is_null())
        .then(|| deployment(&case["deployment"]))
        .flatten();
    let account = gasless_account(case, deployment.as_ref()).unwrap();
    let calls = calls(&case["calls"]).unwrap();
    let replies = case["submit"]["replies"].as_array().cloned();
    let replies = replies.unwrap_or_else(|| case["replies"].as_array().unwrap().clone());
    let mut sent = Vec::new();
    let mut signed = Value::Null;
    let refused = |error| json!({"refused": thrown(&error)});
    let session = match GaslessSession::open(&policy, &account, &calls, &gasless_v2_classes()) {
        Ok(session) => session,
        Err(code) => return json!({"outcome": refused(code.into()), "sent": sent, "signed": null}),
    };
    let reviewed = offer(&case["reviewed"]);
    let now = case["nowSeconds"].as_u64().unwrap();
    let (mut round, mut step) = match SubmitRound::start(session, &reviewed, reference(case), now) {
        Ok(started) => started,
        Err(error) => return json!({"outcome": refused(error), "sent": sent, "signed": null}),
    };
    let outcome = loop {
        let next = match step {
            SubmitStep::Post { request, .. } => {
                sent.push(gasless_support::sent(&request));
                let reply = &replies[sent.len() - 1];
                round.accept(&outcome(reply, policy.max_body_bytes))
            }
            SubmitStep::Sign { hash } => {
                signed = json!(felt::canonical(&hash));
                if case["locked"] == true {
                    break json!({"refused": locked()});
                }
                let read_now = case["readNowSeconds"].as_u64().unwrap();
                round.signed(&signature(account.family), read_now)
            }
            SubmitStep::Submitted(submission) => {
                break json!({
                    "transactionHash": felt::canonical(&submission.transaction_hash),
                    "userFee": user_fee_json(&submission.user_fee),
                })
            }
        };
        step = match next {
            Ok(next) => next,
            Err(error) => break refused(error),
        };
    };
    json!({"outcome": outcome, "sent": sent, "signed": signed})
}

const SELECTOR_FELT_CASE: &str =
    "calls: the user's entrypoint is given as its selector felt (an honest body)";
const SEPOLIA_DEPLOY_CASE: &str =
    "accepted: deploy and invoke on Sepolia over a test route with a made-up forwarder";

#[test]
fn every_gateway_body_is_checked_and_submitted_as_the_extension_did() {
    let honest = cases("review", 57)
        .into_iter()
        .find(|case| name(case) == "accepted: one USDC transfer and a fee inside every bound")
        .unwrap();
    let honest = submit(&honest);
    for case in cases("review", 57) {
        let submitted = submit(&case);
        let expected = json!({
            "outcome": case["submit"]["outcome"],
            "sent": case["submit"]["sent"],
            "signed": case["submit"]["signed"],
        });
        match name(&case).as_str() {
            // knownExtensionBug: starknet.js hashes a felt entrypoint's text as a name, so the
            // extension refuses this honest body. The core takes the felt as the selector
            // (Bramble #1039's fix): it accepts the body, signs the same hash as the call spelled
            // by name, and submits.
            SELECTOR_FELT_CASE => {
                assert!(case["knownExtensionBug"].is_string());
                assert_eq!(case["check"], json!({"ok": false, "reason": "calls"}));
                assert_eq!(check(&case), check(&expected_honest(&case)));
                assert_eq!(submitted["signed"], honest["signed"]);
                assert_eq!(submitted["outcome"], honest["outcome"]);
            }
            // The core derives a deployment's address from its class, salt and calldata; the
            // extension compares the stored `contractAddress`. This made-up deployment does not
            // derive the made-up address, so the core refuses it before any request.
            SEPOLIA_DEPLOY_CASE => {
                assert_eq!(check(&case), case["check"], "{}", name(&case));
                let code = GaslessCode::Unavailable;
                assert_eq!(
                    submitted["outcome"],
                    json!({"refused": thrown(&code.into())})
                );
                assert_eq!(submitted["sent"], json!([]));
                assert_eq!(expected["outcome"]["transactionHash"], "0x7a1ce5");
            }
            _ => {
                assert_eq!(check(&case), case["check"], "{}: check", name(&case));
                assert_eq!(submitted, expected, "{}: submit", name(&case));
            }
        }
    }
}

/// The selector-felt case with its call spelled by name: the check the fix must agree with.
fn expected_honest(case: &Value) -> Value {
    let mut honest = case.clone();
    honest["calls"][0]["entrypoint"] = json!("transfer");
    honest
}

fn held(case: &Value) -> Vec<(GaslessHeldToken, Option<U256>)> {
    let unpriced: Vec<Felt> = case["unpriced"]
        .as_array()
        .unwrap()
        .iter()
        .map(felt_at)
        .collect();
    let reference = U256::from_decimal(case["reference"].as_str().unwrap()).unwrap();
    case["held"]
        .as_array()
        .unwrap()
        .iter()
        .map(|token| {
            let address = felt_at(&token["address"]);
            let held = GaslessHeldToken {
                address,
                balance: token["balance"]
                    .as_str()
                    .map(|b| U256::from_decimal(b).unwrap()),
                decimals: token["decimals"].as_u64().unwrap() as u8,
                label: token["label"].as_str().unwrap().into(),
            };
            (held, (!unpriced.contains(&address)).then_some(reference))
        })
        .collect()
}

/// `quoteGaslessOffers` through the core: its gates, then the round over the scripted replies.
fn offers(case: &Value) -> (Value, Vec<Value>) {
    let route = json!({
        "chainId": "SN_MAIN",
        "enabled": true,
        "forwarder": felt::canonical(&GASLESS_POLICY.ready_route(starknet_wallet_core::NetworkId::Mainnet).unwrap().1),
        "url": "https://wallet.nodes.starknet.org/api/paymaster/mainnet",
    });
    let policy = policy(&route);
    let mut sent = Vec::new();
    let mut with_chain = case.clone();
    with_chain["chainId"] = json!("SN_MAIN");
    let calls = calls(&case["calls"]).unwrap();
    let opened = gasless_account(&with_chain, None)
        .filter(|_| case["strkShort"] == true)
        .and_then(|account| {
            GaslessSession::open(&policy, &account, &calls, &gasless_v2_classes()).ok()
        });
    let Some(session) = opened else {
        return (json!([]), sent);
    };
    let strk = felt::parse(STRK).unwrap();
    let now = case["nowSeconds"].as_u64().unwrap();
    let (mut round, mut step) = OfferRound::start(session, held(case), strk, now);
    let replies = case["replies"].as_array().unwrap();
    loop {
        match step {
            RoundStep::Post { request, delay_ms } => {
                assert_eq!(delay_ms, 0, "{}", name(case));
                sent.push(gasless_support::sent(&request));
                step = round.accept(&outcome(&replies[sent.len() - 1], policy.max_body_bytes));
            }
            RoundStep::Done(offers) => {
                return (offers.iter().map(offer_json).collect(), sent);
            }
        }
    }
}

#[test]
fn the_offer_round_asks_and_offers_what_the_extension_did() {
    for case in cases("offers", 14) {
        let (offers, sent) = offers(&case);
        assert_eq!(offers, case["offers"], "{}: offers", name(&case));
        assert_eq!(Value::Array(sent), case["sent"], "{}: sent", name(&case));
    }
}
