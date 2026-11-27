//! A proof-bearing batch's plan beyond the fixture's transcripts: a proving-block wait that
//! fails, and an account read undeployed, each ending the plan before any estimate or signature.
//! The batch and its node replies are `extension-strk20-invoke.jsonl`'s (`strk20_invoke_parity`).

use serde_json::{json, Value};
use starknet_wallet_core::send::plan::{Invoke, Kind, Plan};
use starknet_wallet_core::send::proof_invoke::{PriorWrite, ProofSubmission};
use starknet_wallet_core::send::review_flags::ReviewContext;
use starknet_wallet_core::send::rpc_bodies;
use starknet_wallet_core::send::types::{SendFailureCode as Code, SendStep, StepReply};
use starknet_wallet_core::Felt;

use crate::plan_support::Host;
use crate::strk20_invoke_parity::{account, calls, network, of_kind, proof, reply, rpc, MATURITY};

/// A prior write that reverted stops the plan at `Confirm`'s wait: nothing is estimated again or
/// signed ("strk20 services unavailable" is the core's `PreparationUnavailable`).
#[test]
fn a_failed_wait_stops_the_plan_before_signing() {
    let line = &of_kind("rpc")[1];
    assert!(proof(line).is_some());
    let host = Host::new();
    let account = account(&host, network(line));
    let calls = calls(line);
    let pool = calls.last().unwrap().contract_address;
    let prior = Felt::from(0x5afe_u32);
    let submission = ProofSubmission {
        proof: proof(line).unwrap(),
        prior: Some(PriorWrite::Submitted(prior)),
        maturity: MATURITY,
    };
    let context = ReviewContext::default();
    let invoke = Invoke::reviewed(&account, calls, &bramble_core::SEND_POLICY, &context)
        .unwrap()
        .with_proof(&pool, submission)
        .unwrap();
    let mut plan = Plan::new(
        account,
        Kind::Invoke(Box::new(invoke)),
        bramble_core::SEND_POLICY,
    );
    let review = line["review"].as_array().unwrap();
    let mut step = plan.advance(&host, StepReply::Start).unwrap();
    while let SendStep::Rpc { id, body, .. } = &step {
        let method = serde_json::from_str::<Value>(body).unwrap()["method"].clone();
        let recorded = review
            .iter()
            .find(|e| e["body"]["method"] == method)
            .unwrap();
        step = plan.advance(&host, reply(*id, &recorded["reply"])).unwrap();
    }
    let step = plan.advance(&host, StepReply::Confirm).unwrap();
    let (id, body) = rpc(&step);
    assert_eq!(body, rpc_bodies::transaction_status(id, &prior));
    let reverted = json!({"jsonrpc": "2.0", "result": {"execution_status": "REVERTED", "finality_status": "ACCEPTED_ON_L2"}});
    let step = plan.advance(&host, reply(id, &reverted)).unwrap();
    let SendStep::Failed { failure } = step else {
        panic!("{step:?}");
    };
    assert_eq!(failure.code, Code::PreparationUnavailable);
    assert_eq!(failure.hash, None);
    assert_eq!(plan.signatures_made(), 0);
}

/// A proof exists only for a deployed account, so a proof plan that finds the account undeployed
/// (at review, or again at submit) fails closed before any estimate or signature, where an
/// ordinary batch would bundle the deployment.
#[test]
fn an_undeployed_account_never_carries_a_proof() {
    let line = &of_kind("rpc")[1];
    let not_found =
        json!({"jsonrpc": "2.0", "error": {"code": 20, "message": "Contract not found"}});
    for undeployed_at_submit in [false, true] {
        let host = Host::new();
        let account = account(&host, network(line));
        let calls = calls(line);
        let pool = calls.last().unwrap().contract_address;
        let submission = ProofSubmission {
            proof: proof(line).unwrap(),
            prior: None,
            maturity: MATURITY,
        };
        let context = ReviewContext::default();
        let invoke = Invoke::reviewed(&account, calls, &bramble_core::SEND_POLICY, &context)
            .unwrap()
            .with_proof(&pool, submission)
            .unwrap();
        let mut plan = Plan::new(
            account,
            Kind::Invoke(Box::new(invoke)),
            bramble_core::SEND_POLICY,
        );
        let answer = |method: &Value, phase: &str| -> Value {
            if method == "starknet_getClassHashAt" && (phase == "submit") == undeployed_at_submit {
                return not_found.clone();
            }
            let exchanges = line[phase].as_array().unwrap();
            let recorded = exchanges.iter().find(|e| e["body"]["method"] == *method);
            recorded.unwrap()["reply"].clone()
        };
        let mut step = plan.advance(&host, StepReply::Start).unwrap();
        let mut phase = "review";
        loop {
            match &step {
                SendStep::Rpc { id, body, .. } => {
                    let method = serde_json::from_str::<Value>(body).unwrap()["method"].clone();
                    // Once the account read undeployed, nothing is estimated.
                    let refused = phase == "submit" || !undeployed_at_submit;
                    assert!(!refused || method != "starknet_estimateFee", "{phase}");
                    step = plan
                        .advance(&host, reply(*id, &answer(&method, phase)))
                        .unwrap();
                }
                SendStep::Review => {
                    assert!(undeployed_at_submit);
                    phase = "submit";
                    // Past the review: from here no estimate may be sent either.
                    step = plan.advance(&host, StepReply::Confirm).unwrap();
                    // The proving-block wait answers before the submit's reads.
                    let (id, _) = rpc(&step);
                    let block = &line["submit"][0]["reply"];
                    step = plan.advance(&host, reply(id, block)).unwrap();
                    let (id, body) = rpc(&step);
                    let method = serde_json::from_str::<Value>(body).unwrap()["method"].clone();
                    step = plan
                        .advance(&host, reply(id, &answer(&method, phase)))
                        .unwrap();
                }
                SendStep::Failed { failure } => {
                    assert_eq!(failure.code, Code::PreparationUnavailable);
                    assert_eq!(failure.hash, None);
                    break;
                }
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(plan.signatures_made(), 0);
        assert_eq!(phase == "submit", undeployed_at_submit);
    }
}
