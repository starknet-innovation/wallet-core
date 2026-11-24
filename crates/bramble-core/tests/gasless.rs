//! A transfer's fee paid in USDC through mobile's exports, end to end against a canned node and a
//! canned paymaster: the offer from a reviewed transfer whose STRK falls short, the choice, the
//! gateway's build checked and signed by the core, the execute, and the row with the signed fee.
//! Then each refusal: one per gate the export adds over wallet-core's (whose guards its own unit
//! tests and `gasless_parity.rs` pin).
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

mod gasless_export_support;
mod send_support;

use bramble_core::{
    activity_fee_text, activity_needs_observation, ActivityKind, GaslessFeeError, GaslessOffer,
    GaslessSubmitStep, NetworkId, PriceView, SendError, StepReply, TransferDirection,
};
use gasless_export_support::*;
use serde_json::{json, Value};
use starknet_wallet_core::{felt, kms, Felt};

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
    // The journal row filed before the execute took the hash: one row, no relay left on it.
    let rows = core.activity(account_id(&core)).unwrap();
    assert_eq!(rows.len(), 1);
    let row = rows.iter().find(|row| row.hash == "0x7a1ce5").unwrap();
    assert!(row.relay.is_none());
    assert!(activity_needs_observation(row.clone(), NOW));
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
    // An answer as old as the price book's TTL is no longer current at the choice.
    let old = PriceView {
        as_of_ms: Some((NOW - 5 * 60 * 1000) as f64),
        ..prices()
    };
    assert_eq!(
        refused(fee.choose(offer, Some(old), NOW)),
        "This token's network fee could not be checked."
    );
    let plan = transfer(&core, ACCOUNT_1, "0");
    let (fee, offer) = offered(&plan);
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
