//! The send plan must put on the wire exactly what the extension's real send path puts there.
//! `extension-transactions-{wire,argent,argent-upgraded,braavos}.json` record that path for an
//! OpenZeppelin, Ready (Argent) and Braavos account against a canned node; this drives
//! `starknet_wallet_core::send::plan::Plan` under `SEND_POLICY` with the same replies and
//! requires the same request bodies in the same order, the same number of signed hashes and the
//! same transaction hash. Moved from mobile's `transactions_{,argent_,braavos_}parity.rs` in
//! Phase 2.3, with a test host in place of the mobile vault (`plan_support`); the refusals are in
//! `transactions_refusal_parity.rs`, and mobile's lifecycle tests (locks, persistence) stay with
//! its wallet.

mod plan_support;

use plan_support::*;
use serde_json::json;
use starknet_wallet_core::send::judge::TxStatus;
use starknet_wallet_core::send::types::{SendError, SendStep, StepReply};

#[test]
fn plans_match_the_extension_wire_captures() {
    let mut count = 0;
    for (name, family) in [
        ("wire", "openzeppelin"),
        ("argent", "argent"),
        ("argent-upgraded", "argent"),
        ("braavos", "braavos"),
    ] {
        for scenario in capture(name)["scenarios"].as_array().unwrap() {
            run(scenario, family);
            count += 1;
        }
    }
    assert_eq!(count, 20);
}

#[test]
fn a_broadcast_is_confirmed_at_the_checkpoints() {
    let wire = capture("wire");
    let (mut plan, host) = run(&wire["scenarios"][0], "openzeppelin");
    let mut step = plan.advance(&host, StepReply::Proceed).unwrap();
    assert_eq!(step, SendStep::Wait { delay_ms: 0 });
    step = plan.advance(&host, StepReply::WaitElapsed).unwrap();
    let SendStep::Rpc { id, body, .. } = step else {
        panic!("{step:?}");
    };
    assert!(body.contains("starknet_getTransactionStatus"));
    let accepted =
        json!({"result": {"finality_status": "ACCEPTED_ON_L2", "execution_status": "SUCCEEDED"}});
    let step = plan.advance(&host, reply(id, &accepted)).unwrap();
    let hash = wire["scenarios"][0]["transactionHash"].as_str().unwrap();
    assert_eq!(
        step,
        SendStep::Finished {
            status: TxStatus::Accepted,
            hash: hash.into(),
            explorer_url: Some(format!("https://explorer.test/tx/{hash}")),
        }
    );
    assert_eq!(host.observed.borrow().len(), 1);
    assert_eq!(
        plan.advance(&host, StepReply::Confirm),
        Err(SendError::OutOfOrder)
    );
    assert_eq!(
        plan.advance(&host, StepReply::Start),
        Err(SendError::PlanClosed)
    );
}

#[test]
fn a_lock_after_review_refuses_confirm_before_signing() {
    let wire = capture("wire");
    let scenario = &wire["scenarios"][0];
    let host = Host::new();
    let mut plan = plan(
        scenario,
        account(scenario, "openzeppelin", host.public_key()),
    );
    let step = plan.advance(&host, StepReply::Start).unwrap();
    let step = drive(&mut plan, &host, step, &review_calls(scenario), scenario);
    assert_eq!(step, SendStep::Review);
    host.session.set(false);
    let SendStep::Failed { failure } = plan.advance(&host, StepReply::Confirm).unwrap() else {
        panic!("confirmed after a lock");
    };
    assert_eq!(
        failure.code,
        starknet_wallet_core::send::types::SendFailureCode::Locked
    );
    assert!(host.signed.borrow().is_empty() && host.records.borrow().is_empty());
    assert_eq!(
        plan.advance(&host, StepReply::Confirm),
        Err(SendError::PlanClosed)
    );
}

#[test]
fn a_fee_past_the_reviewed_headroom_is_refused_before_signing() {
    let wire = capture("wire");
    let scenario = &wire["scenarios"][0];
    let host = Host::new();
    let mut plan = plan(
        scenario,
        account(scenario, "openzeppelin", host.public_key()),
    );
    let step = plan.advance(&host, StepReply::Start).unwrap();
    let step = drive(&mut plan, &host, step, &review_calls(scenario), scenario);
    assert_eq!(step, SendStep::Review);
    // The submit re-estimate quotes every price doubled: past 3/2, so nothing is signed.
    let mut submit = phase(scenario, "submit");
    let at = submit
        .iter()
        .position(|e| e["method"] == "starknet_estimateFee")
        .unwrap();
    submit.truncate(at + 1);
    for entry in submit[at]["reply"]["result"].as_array_mut().unwrap() {
        for key in ["l1_gas_price", "l2_gas_price", "l1_data_gas_price"] {
            let price = u128::from_str_radix(&entry[key].as_str().unwrap()[2..], 16).unwrap();
            entry[key] = format!("{:#x}", price * 2).into();
        }
    }
    let step = plan.advance(&host, StepReply::Confirm).unwrap();
    let SendStep::Failed { failure } = drive(&mut plan, &host, step, &submit, scenario) else {
        panic!("signed past the reviewed fee");
    };
    assert_eq!(
        failure.code,
        starknet_wallet_core::send::types::SendFailureCode::ReviewedFeeExceeded
    );
    assert!(host.signed.borrow().is_empty() && host.records.borrow().is_empty());
}
