//! A transfer's fee paid in USDC through mobile's exports, end to end against a canned node and a
//! canned paymaster: the offer from a reviewed transfer whose STRK falls short, the choice, the
//! gateway's build checked and signed by the core, the execute, and the row with the signed fee.
//! Then each refusal: one per gate the export adds over wallet-core's (whose guards its own unit
//! tests and `gasless_parity.rs` pin).
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

mod send_support;

use std::sync::Arc;

use bramble_core::gasless::GASLESS_POLICY;
use bramble_core::{
    activity_fee_text, ActivityKind, DisplayCurrency, GaslessBalance, GaslessFee, GaslessFeeError,
    GaslessOffer, GaslessOfferStep, GaslessSubmitStep, HttpOutcome, HttpRequest, NetworkId,
    PricePoint, PriceView, SendError, SendStep, StepReply, TransferDirection, TransferPlan,
    TransferRequest, WalletCore,
};
use send_support::*;
use serde_json::{json, Value};
use starknet_wallet_core::{felt, kms, Felt};

const ETH: &str = "0x49d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7";
const USDC: &str = "0x33068f6539f8e6e6b131e6b2b814e6c34a5224bc66947c47dab9dfee93b35fb";
const NOW_S: u64 = NOW / 1000;
const TYPES: &str = r#"{"StarknetDomain":[{"name":"name","type":"shortstring"},{"name":"version","type":"shortstring"},{"name":"chainId","type":"shortstring"},{"name":"revision","type":"shortstring"}],"OutsideExecution":[{"name":"Caller","type":"ContractAddress"},{"name":"Nonce","type":"felt"},{"name":"Execute After","type":"u128"},{"name":"Execute Before","type":"u128"},{"name":"Calls","type":"Call*"}],"Call":[{"name":"To","type":"ContractAddress"},{"name":"Selector","type":"selector"},{"name":"Calldata","type":"felt*"}]}"#;

fn forwarder() -> String {
    felt::canonical(&GASLESS_POLICY.ready_route(NetworkId::Mainnet).unwrap().1)
}

fn account_id(core: &WalletCore) -> String {
    oz_id(core, NetworkId::Mainnet, 0)
}

fn address(core: &WalletCore) -> Felt {
    felt::parse(account_id(core).split_once(':').unwrap().1).unwrap()
}

/// 0.001 ETH to account 1 from Mainnet account 0, with `strk` base units of STRK to pay with.
fn transfer(core: &Arc<WalletCore>, recipient: &str, strk: &str) -> Arc<TransferPlan> {
    transfer_from(core, recipient, strk, true)
}

fn transfer_from(
    core: &Arc<WalletCore>,
    recipient: &str,
    strk: &str,
    deployed: bool,
) -> Arc<TransferPlan> {
    let request = TransferRequest {
        network: NetworkId::Mainnet,
        account_id: account_id(core),
        token_address: ETH.into(),
        recipient: recipient.into(),
        amount: "0.001".into(),
        fee_balance: Some(strk.into()),
        tracked_tokens: vec![],
    };
    let plan = core.clone().prepare_transfer(request).unwrap();
    let mut node = Node::new(NetworkId::Mainnet, deployed);
    let first = plan.advance(StepReply::Start).unwrap();
    assert_eq!(
        run_until(&plan, core, &mut node, first, never),
        SendStep::Review
    );
    plan
}

fn prices() -> PriceView {
    PriceView {
        currency: DisplayCurrency::Usd,
        prices: vec![
            PricePoint {
                address: STRK.into(),
                value: 0.5,
            },
            PricePoint {
                address: USDC.into(),
                value: 1.0,
            },
        ],
        changes: vec![],
        as_of_ms: Some(NOW as f64),
        stale: false,
    }
}

fn balances() -> Vec<GaslessBalance> {
    vec![GaslessBalance {
        token_address: USDC.into(),
        balance: Some("5000".into()),
    }]
}

fn result(value: Value) -> HttpOutcome {
    let body = json!({"id": 1, "jsonrpc": "2.0", "result": value}).to_string();
    HttpOutcome {
        status: 200,
        retry_after: None,
        body: Some(body.into_bytes()),
        timed_out: false,
    }
}

fn selector(name: &str) -> String {
    felt::canonical(&kms::selector(name))
}

/// The gateway's honest body: the reviewed transfer, then `fee` USDC base units to the forwarder.
fn typed_data(amount: &str, fee: &str) -> Value {
    let user = json!({"To": ETH, "Selector": selector("transfer"),
        "Calldata": [ACCOUNT_1, amount, "0x0"]});
    let fee = json!({"To": USDC, "Selector": selector("transfer"),
        "Calldata": [forwarder(), fee, "0x0"]});
    json!({
        "types": serde_json::from_str::<Value>(TYPES).unwrap(),
        "primaryType": "OutsideExecution",
        "domain": {"name": "Account.execute_from_outside", "version": "2", "chainId": "SN_MAIN",
            "revision": "1"},
        "message": {"Caller": forwarder(), "Nonce": "0x5", "Execute After": "0x1",
            "Execute Before": format!("{:#x}", NOW_S + 600), "Calls": [user, fee]},
    })
}

fn built(typed: &Value) -> HttpOutcome {
    result(json!({
        "fee": {
            "estimated_fee_in_gas_token": "0x3e8",
            "estimated_fee_in_strk": "0x1",
            "gas_token_price_in_strk": "0x1",
            "suggested_max_fee_in_gas_token": "0x7d0",
            "suggested_max_fee_in_strk": "0x1",
        },
        "parameters": {"fee_mode": {"gas_token": USDC, "mode": "default"}, "version": "0x1"},
        "type": "invoke",
        "typed_data": typed,
    }))
}

/// 0.001 ETH in base units, as the transfer's calldata carries it.
const AMOUNT: &str = "0x38d7ea4c68000";

fn method(request: &HttpRequest) -> Value {
    let body: Value = serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
    body["method"].clone()
}

/// The round over a paymaster that offers USDC: the fee object and its one offer.
fn offered(plan: &Arc<TransferPlan>) -> (Arc<GaslessFee>, GaslessOffer) {
    let started = plan
        .clone()
        .gasless_offers(Some(prices()), balances(), NOW)
        .expect("a gasless choice");
    let replies = [
        result(json!(true)),
        result(json!([{"decimals": 6, "token_address": USDC}])),
        built(&typed_data(AMOUNT, "0x3e8")),
    ];
    let mut step = started.step;
    for (reply, expected) in replies.into_iter().zip([
        "paymaster_isAvailable",
        "paymaster_getSupportedTokens",
        "paymaster_buildTransaction",
    ]) {
        let GaslessOfferStep::Post { post } = step else {
            panic!("expected {expected}, got {step:?}");
        };
        assert_eq!(method(&post.request), expected);
        step = started.fee.accept_offer_reply(reply).unwrap();
    }
    let GaslessOfferStep::Done { offers } = step else {
        panic!("the round did not end: {step:?}");
    };
    assert_eq!(offers.len(), 1);
    (started.fee, offers[0].clone())
}

fn post(step: GaslessSubmitStep) -> HttpRequest {
    match step {
        GaslessSubmitStep::Post { post } => post.request,
        other => panic!("expected a post, got {other:?}"),
    }
}

#[test]
fn a_short_transfer_pays_its_fee_in_usdc_signed_once_and_files_the_signed_fee() {
    let (_dir, core) = wallet();
    let plan = transfer(&core, ACCOUNT_1, "0");
    let (fee, offer) = offered(&plan);
    assert_eq!(
        (offer.amount.as_str(), offer.display.as_str()),
        ("1000", "0.001 USDC")
    );
    let build = post(fee.choose(offer.clone(), Some(prices()), NOW).unwrap());
    assert_eq!(method(&build), "paymaster_buildTransaction");
    // The plan's STRK path is gone: a Confirm now finds it closed.
    assert_eq!(plan.advance(StepReply::Confirm), Err(SendError::PlanClosed));
    let typed = typed_data(AMOUNT, "0x3e8");
    let execute = post(fee.accept_submit_reply(built(&typed), NOW).unwrap());
    let body: Value = serde_json::from_str(execute.body.as_deref().unwrap()).unwrap();
    assert_eq!(body["method"], "paymaster_executeTransaction");
    let invoke = &body["params"]["transaction"]["invoke"];
    assert_eq!(invoke["typed_data"], typed, "the gateway's body, unchanged");
    // The core signed the checked hash with the account's key: an OpenZeppelin `[r, s]`.
    let hash = kms::typed_data_message_hash(&typed.to_string(), &address(&core)).unwrap();
    let record = core
        .account_records(NetworkId::Mainnet)
        .unwrap()
        .into_iter()
        .find(|record| record.id == account_id(&core))
        .unwrap();
    let signature: Vec<Felt> = invoke["signature"]
        .as_array()
        .unwrap()
        .iter()
        .map(|felt| felt::parse(felt.as_str().unwrap()).unwrap())
        .collect();
    let public_key = felt::parse(&record.public_key).unwrap();
    assert_eq!(signature.len(), 2);
    assert!(kms::stark_signature_is_valid(
        &public_key,
        &hash,
        &signature[0],
        &signature[1]
    ));
    let submitted = fee
        .accept_submit_reply(result(json!({"transaction_hash": "0x7a1ce5"})), NOW)
        .unwrap();
    let GaslessSubmitStep::Submitted {
        transaction_hash, ..
    } = submitted
    else {
        panic!("not submitted: {submitted:?}");
    };
    assert_eq!(transaction_hash, "0x7a1ce5");
    let rows = core.activity(account_id(&core)).unwrap();
    let row = rows.iter().find(|row| row.hash == "0x7a1ce5").unwrap();
    assert_eq!(row.kind, ActivityKind::Transfer);
    assert_eq!(row.direction, Some(TransferDirection::Out));
    assert_eq!(row.amount.as_deref(), Some("0.001"));
    assert!(
        row.summary.ends_with(" (network fee 0.001 USDC)"),
        "{}",
        row.summary
    );
    let signed = row.user_fee.as_ref().unwrap();
    assert_eq!(
        (signed.amount.as_str(), signed.token.as_str()),
        ("1000", "USDC")
    );
    assert_eq!(
        activity_fee_text(row.clone()).as_deref(),
        Some("0.001 USDC")
    );
    // Spent: nothing more is taken.
    assert_eq!(
        fee.choose(offer, Some(prices()), NOW),
        Err(GaslessFeeError::NotFound)
    );
}

#[test]
fn no_choice_is_offered_when_strk_covers_the_fee_or_the_recipient_is_poisoned() {
    let (_dir, core) = wallet();
    let covered = transfer(&core, ACCOUNT_1, "100000000000000000000");
    assert!(covered
        .gasless_offers(Some(prices()), balances(), NOW)
        .is_none());
    // A twin of the account's own address: the review refuses it, so gasless must too.
    let own = felt::canonical(&address(&core));
    let twin = format!(
        "{}{}{}",
        &own[..6],
        "0".repeat(own.len() - 10),
        &own[own.len() - 4..]
    );
    let poisoned = transfer(&core, &twin, "0");
    assert!(!poisoned.current_review().unwrap().can_confirm);
    assert!(poisoned
        .gasless_offers(Some(prices()), balances(), NOW)
        .is_none());
}

// Mainnet screens the subject before relaying, so an undeployed account pays in STRK.
#[test]
fn an_undeployed_account_has_no_gasless_choice() {
    let (_dir, core) = wallet();
    let undeployed = transfer_from(&core, ACCOUNT_1, "0", false);
    assert!(undeployed.current_review().unwrap().requires_deployment);
    assert!(undeployed
        .gasless_offers(Some(prices()), balances(), NOW)
        .is_none());
}

#[test]
fn a_confirmed_or_closed_plan_has_no_gasless_choice() {
    let (_dir, core) = wallet();
    let plan = transfer(&core, ACCOUNT_1, "0");
    plan.close();
    assert!(plan
        .gasless_offers(Some(prices()), balances(), NOW)
        .is_none());
}

#[test]
fn only_an_offer_the_round_made_is_chosen_and_only_once_per_plan() {
    let (_dir, core) = wallet();
    let plan = transfer(&core, ACCOUNT_1, "0");
    let (fee, offer) = offered(&plan);
    let (twin, _) = offered(&plan);
    let raised = GaslessOffer {
        amount: "999".into(),
        ..offer.clone()
    };
    assert_eq!(
        fee.choose(raised, Some(prices()), NOW),
        Err(GaslessFeeError::NotFound)
    );
    assert!(fee.choose(offer.clone(), Some(prices()), NOW).is_ok());
    // A second fee object for the same plan finds the transfer already taken.
    assert_eq!(
        twin.choose(offer, Some(prices()), NOW),
        Err(GaslessFeeError::NotFound)
    );
    assert_eq!(
        fee.accept_offer_reply(result(json!(true))),
        Err(GaslessFeeError::NotFound)
    );
}

fn refused(error: Result<GaslessSubmitStep, GaslessFeeError>) -> String {
    match error {
        Err(GaslessFeeError::Refused { reason, .. }) => reason,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn a_choice_without_a_fresh_price_is_refused_before_any_request() {
    let (_dir, core) = wallet();
    let plan = transfer(&core, ACCOUNT_1, "0");
    let (fee, offer) = offered(&plan);
    let stale = PriceView {
        stale: true,
        ..prices()
    };
    assert_eq!(
        refused(fee.choose(offer, Some(stale), NOW)),
        "This token's network fee could not be checked."
    );
}

#[test]
fn a_build_that_moves_the_users_amount_is_refused_and_never_signed() {
    let (_dir, core) = wallet();
    let plan = transfer(&core, ACCOUNT_1, "0");
    let (fee, offer) = offered(&plan);
    post(fee.choose(offer, Some(prices()), NOW).unwrap());
    let moved = typed_data("0x38d7ea4c68001", "0x3e8");
    assert_eq!(
        refused(fee.accept_submit_reply(built(&moved), NOW)),
        "This network fee quote was refused. Nothing was submitted."
    );
    let honest = built(&typed_data(AMOUNT, "0x3e8"));
    assert_eq!(
        fee.accept_submit_reply(honest, NOW),
        Err(GaslessFeeError::NotFound)
    );
}

#[test]
fn a_lock_before_signing_refuses_and_nothing_is_executed() {
    let (_dir, core) = wallet();
    let plan = transfer(&core, ACCOUNT_1, "0");
    let (fee, offer) = offered(&plan);
    post(fee.choose(offer, Some(prices()), NOW).unwrap());
    core.lock();
    let typed = typed_data(AMOUNT, "0x3e8");
    assert_eq!(
        fee.accept_submit_reply(built(&typed), NOW),
        Err(GaslessFeeError::Locked)
    );
}

#[test]
fn a_lost_execute_files_no_row() {
    let (_dir, core) = wallet();
    let plan = transfer(&core, ACCOUNT_1, "0");
    let (fee, offer) = offered(&plan);
    post(fee.choose(offer, Some(prices()), NOW).unwrap());
    let typed = typed_data(AMOUNT, "0x3e8");
    post(fee.accept_submit_reply(built(&typed), NOW).unwrap());
    let lost = HttpOutcome {
        status: 502,
        retry_after: None,
        body: None,
        timed_out: false,
    };
    assert!(matches!(
        fee.accept_submit_reply(lost, NOW),
        Err(GaslessFeeError::Lost { .. })
    ));
    assert!(core.activity(account_id(&core)).unwrap().is_empty());
}

// `postGasless`: an execute whose screening was unavailable is posted again after a pause, and
// only if the wallet is still unlocked when the pause ends.
#[test]
fn an_execute_repeat_waits_and_is_refused_if_the_wallet_locked_meanwhile() {
    let unavailable = || HttpOutcome {
        status: 503,
        retry_after: None,
        body: Some(br#"{"error":"screening_unavailable"}"#.to_vec()),
        timed_out: false,
    };
    for lock in [false, true] {
        let (_dir, core) = wallet();
        let plan = transfer(&core, ACCOUNT_1, "0");
        let (fee, offer) = offered(&plan);
        post(fee.choose(offer, Some(prices()), NOW).unwrap());
        assert_eq!(fee.resume(), Err(GaslessFeeError::NotFound));
        let typed = typed_data(AMOUNT, "0x3e8");
        let execute = post(fee.accept_submit_reply(built(&typed), NOW).unwrap());
        assert_eq!(
            fee.accept_submit_reply(unavailable(), NOW),
            Ok(GaslessSubmitStep::Wait { delay_ms: 250 })
        );
        if lock {
            core.lock();
            assert_eq!(fee.resume(), Err(GaslessFeeError::Locked));
            assert_eq!(fee.resume(), Err(GaslessFeeError::NotFound));
        } else {
            assert_eq!(post(fee.resume().unwrap()), execute, "the same signed body");
        }
    }
}
