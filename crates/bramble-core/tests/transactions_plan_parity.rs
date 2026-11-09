//! The send plan must put on the wire exactly what the extension's real send path puts there.
//! `extension-transactions-{wire,argent,argent-upgraded,braavos}.json` record that path for an
//! OpenZeppelin, Ready (Argent) and Braavos account against a canned node; this drives
//! `wallet_core::send::plan::Plan` under `SEND_POLICY` with the same replies and requires the same
//! request bodies in the same order, the same number of signed hashes and the same transaction
//! hash. Moved from mobile's `transactions_{,argent_,braavos_}parity.rs` in Phase 2.3, with a
//! test host in place of the mobile vault (`plan_support`); the refusals are in
//! `transactions_refusal_parity.rs`, and mobile's lifecycle tests (locks, persistence) stay with
//! its wallet.

mod plan_support;

use plan_support::*;
use serde_json::json;
use wallet_core::send::judge::TxStatus;
use wallet_core::send::types::{SendError, SendStep, StepReply};

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
