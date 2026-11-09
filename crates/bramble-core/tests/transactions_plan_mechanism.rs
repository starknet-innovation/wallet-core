//! The plan's own behaviour against a canned node: refusals, ordering, fee ceilings, the
//! deployment wait. Moved from mobile's `tests/transactions_plan.rs` in Phase 2.3; its wallet
//! cases (contacts, tracked tokens, a locked wallet preparing) stay with mobile's wallet, and the
//! request checks `prepare_transfer` made are `Transfer::new`'s here.

mod plan_support;

use plan_support::*;
use serde_json::{json, Value};
use starknet_wallet_core::send::calls::ReviewFlag;
use starknet_wallet_core::send::judge::TxStatus;
use starknet_wallet_core::send::plan::{Account, Authorization, Kind, Plan, TransferRefusal};
use starknet_wallet_core::send::sign::Deployment;
use starknet_wallet_core::send::types::{
    SendError, SendFailure, SendFailureCode as Code, SendStep, StepReply,
};
use starknet_wallet_core::{felt, Felt, NetworkId};

const ACCOUNT_0: &str = "0x208361e3911b1d93a704400b88e7e53a89171fce64dbb48f8479e4f44b398d9";

/// The published phrase's OpenZeppelin account 0 on Sepolia.
fn oz_account(host: &Host) -> Account {
    let public_key = host.public_key();
    let deployment = Deployment {
        class_hash: f(OZ_CLASS),
        salt: Felt::ZERO,
        constructor_calldata: vec![public_key],
    };
    let address = deployment.address().unwrap();
    assert_eq!(address, f(ACCOUNT_0));
    Account {
        network: NetworkId::Sepolia,
        address,
        public_key,
        authorization: Authorization::StarkPair,
        deployment: Some(deployment),
    }
}

fn transfer_plan(host: &Host, recipient: &str) -> Plan {
    let account = oz_account(host);
    let transfer = transfer(&account, recipient, &[]).unwrap();
    Plan::new(
        account,
        Kind::Transfer(Box::new(transfer)),
        bramble_core::SEND_POLICY,
    )
}

/// Mobile's canned node: deployment state, nonce, and the estimate it returns per transaction.
struct Node {
    deployed: bool,
    nonce: String,
    invoke_estimate: Value,
    deploy_estimate: Value,
    status: Value,
}

impl Node {
    fn sepolia(deployed: bool) -> Self {
        Self {
            deployed,
            nonce: "0x5".into(),
            invoke_estimate: json!({"l1_data_gas_consumed":"0x180","l1_data_gas_price":"0x5a0",
                "l1_gas_consumed":"0x0","l1_gas_price":"0x2d5c6b1e1ef","l2_gas_consumed":"0x1234a0",
                "l2_gas_price":"0xb2d05e00","overall_fee":"0x1","unit":"FRI"}),
            deploy_estimate: json!({"l1_data_gas_consumed":"0x1e0","l1_data_gas_price":"0x5a0",
                "l1_gas_consumed":"0x0","l1_gas_price":"0x2d5c6b1e1ef","l2_gas_consumed":"0xf4240",
                "l2_gas_price":"0xb2d05e00","overall_fee":"0xabc","unit":"FRI"}),
            status: json!({"execution_status":"SUCCEEDED","finality_status":"ACCEPTED_ON_L2"}),
        }
    }

    /// The reply to a request; a write echoes the hash the host last recorded.
    fn reply(&mut self, method: &str, params: &Value, signed: Option<String>) -> Value {
        let not_found = json!({"error": {"code": 20, "message": "Contract not found"}});
        match method {
            "starknet_chainId" => json!({"result": "0x534e5f5345504f4c4941"}),
            "starknet_getClassHashAt" if self.deployed => json!({"result": OZ_CLASS}),
            "starknet_getClassHashAt" => not_found,
            "starknet_getNonce" if self.deployed => json!({"result": self.nonce}),
            "starknet_getNonce" => not_found,
            "starknet_estimateFee" => {
                let entries: Vec<Value> = params["request"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|tx| match tx["type"].as_str() {
                        Some("DEPLOY_ACCOUNT") => self.deploy_estimate.clone(),
                        _ => self.invoke_estimate.clone(),
                    })
                    .collect();
                json!({ "result": entries })
            }
            "starknet_addDeployAccountTransaction" => {
                self.deployed = true;
                self.nonce = "0x1".into();
                json!({"result": {"transaction_hash": signed.unwrap(), "contract_address": ACCOUNT_0}})
            }
            "starknet_addInvokeTransaction" => {
                json!({"result": {"transaction_hash": signed.unwrap()}})
            }
            "starknet_getTransactionStatus" => json!({ "result": self.status }),
            "starknet_getTransactionReceipt" => json!({"result": {"type": "DEPLOY_ACCOUNT"}}),
            other => panic!("unexpected method {other}"),
        }
    }
}

fn parts(step: &SendStep) -> (u64, String, Value, bool) {
    let SendStep::Rpc {
        id, body, is_write, ..
    } = step
    else {
        panic!("expected an RPC step, got {step:?}");
    };
    let value: Value = serde_json::from_str(body).unwrap();
    let method = value["method"].as_str().unwrap().to_string();
    (*id, method, value["params"].clone(), *is_write)
}

fn latest_hash(host: &Host) -> String {
    felt::canonical(&host.records.borrow().last().expect("a pending record").hash)
}

/// Answers every step from `node` until `stop(method, is_write)` holds or a non-RPC step
/// arrives; returns that step unanswered.
fn run_until(
    plan: &mut Plan,
    host: &Host,
    node: &mut Node,
    mut step: SendStep,
    stop: impl Fn(&str, bool) -> bool,
) -> SendStep {
    loop {
        match &step {
            SendStep::Rpc { .. } => {
                let (id, method, params, is_write) = parts(&step);
                if stop(&method, is_write) {
                    return step;
                }
                let signed = is_write.then(|| latest_hash(host));
                step = plan
                    .advance(host, reply(id, &node.reply(&method, &params, signed)))
                    .unwrap();
            }
            SendStep::Wait { .. } => step = plan.advance(host, StepReply::WaitElapsed).unwrap(),
            _ => return step,
        }
    }
}

fn never(_: &str, _: bool) -> bool {
    false
}

fn reviewed(host: &Host, node: &mut Node, recipient: &str) -> Plan {
    let mut plan = transfer_plan(host, recipient);
    let first = plan.advance(host, StepReply::Start).unwrap();
    assert_eq!(
        run_until(&mut plan, host, node, first, never),
        SendStep::Review
    );
    plan
}

fn failure(step: SendStep) -> SendFailure {
    match step {
        SendStep::Failed { failure } => failure,
        other => panic!("expected a failure, got {other:?}"),
    }
}

#[test]
fn deployed_send_confirms_and_records() {
    let host = Host::new();
    let mut node = Node::sepolia(true);
    let mut plan = reviewed(&host, &mut node, ACCOUNT_1);
    let review = plan.current_review().unwrap().clone();
    assert!(review.can_confirm && !review.requires_deployment);
    assert_eq!(review.fee.unwrap().display, "0.008053 STRK");
    assert_eq!(review.recipient_short, "0x3fa7…41a3");
    let step = plan.advance(&host, StepReply::Confirm).unwrap();
    let SendStep::Broadcast { hash } = run_until(&mut plan, &host, &mut node, step, never) else {
        panic!("no broadcast");
    };
    let step = plan.advance(&host, StepReply::Proceed).unwrap();
    assert_eq!(
        run_until(&mut plan, &host, &mut node, step, never),
        SendStep::Finished {
            status: TxStatus::Accepted,
            explorer_url: Some(format!("https://explorer.test/tx/{hash}")),
            hash: hash.clone(),
        }
    );
    assert_eq!(latest_hash(&host), hash);
}

#[test]
fn a_zero_or_malformed_recipient_or_amount_never_becomes_a_transfer() {
    let host = Host::new();
    let account = oz_account(&host);
    for zero in [
        "0x0",
        "0x0000000000000000000000000000000000000000000000000000000000000000",
        " 0X0 ",
        "not an address",
    ] {
        assert_eq!(
            transfer(&account, zero, &[]).err(),
            Some(TransferRefusal::InvalidRecipient),
            "{zero}"
        );
    }
    // The call signed is the one derived here: recipient and amount in the calldata.
    let built = transfer(&account, ACCOUNT_1, &[]).unwrap();
    assert!(built.flags().is_empty());
}

#[test]
fn estimate_node_error_at_confirm_is_a_node_error_not_a_fee_rise() {
    let host = Host::new();
    let mut node = Node::sepolia(true);
    let mut plan = reviewed(&host, &mut node, ACCOUNT_1);
    let step = plan.advance(&host, StepReply::Confirm).unwrap();
    let estimate = run_until(&mut plan, &host, &mut node, step, |m, _| {
        m == "starknet_estimateFee"
    });
    let (id, ..) = parts(&estimate);
    let error = json!({"error": {"code": 41, "message": "Transaction execution error"}});
    let failed = failure(plan.advance(&host, reply(id, &error)).unwrap());
    assert_eq!(failed.code, Code::NodeError);
}

#[test]
fn write_transport_failure_is_indeterminate_and_never_resent() {
    let host = Host::new();
    let mut node = Node::sepolia(true);
    let mut plan = reviewed(&host, &mut node, ACCOUNT_1);
    let step = plan.advance(&host, StepReply::Confirm).unwrap();
    let (id, ..) = parts(&run_until(&mut plan, &host, &mut node, step, |_, w| w));
    let hash = latest_hash(&host);
    let failed = failure(
        plan.advance(&host, StepReply::TransportFailure { id, now_ms: NOW })
            .unwrap(),
    );
    assert_eq!(
        failed,
        SendFailure {
            code: Code::SubmissionIndeterminate,
            hash: Some(hash.clone()),
            detail: None,
        }
    );
    assert_eq!(plan.interrupted_failure().unwrap().hash, Some(hash));
    assert_eq!(
        plan.advance(&host, StepReply::WaitElapsed),
        Err(SendError::PlanClosed)
    );
}

#[test]
fn a_different_broadcast_hash_is_a_mismatch() {
    let host = Host::new();
    let mut node = Node::sepolia(true);
    let mut plan = reviewed(&host, &mut node, ACCOUNT_1);
    let step = plan.advance(&host, StepReply::Confirm).unwrap();
    let (id, ..) = parts(&run_until(&mut plan, &host, &mut node, step, |_, w| w));
    let other = json!({"result": {"transaction_hash": "0x1"}});
    assert_eq!(
        failure(plan.advance(&host, reply(id, &other)).unwrap()).code,
        Code::HashMismatch
    );
}

#[test]
fn a_poisoned_recipient_is_flagged_and_refused_at_confirm() {
    let host = Host::new();
    let mut node = Node::sepolia(true);
    let lookalike = "0x2083610000000000000000000000000000000000000000000000000000098d9";
    let mut plan = reviewed(&host, &mut node, lookalike);
    let review = plan.current_review().unwrap();
    assert!(review.flags.contains(&ReviewFlag::AddressPoisoning) && !review.can_confirm);
    let failed = failure(plan.advance(&host, StepReply::Confirm).unwrap());
    assert_eq!(failed.code, Code::PoisonedRecipient);
    assert_eq!(plan.signatures_made(), 0);
}

#[test]
fn self_transfer_is_flagged_but_allowed() {
    let host = Host::new();
    let mut node = Node::sepolia(true);
    let plan = reviewed(&host, &mut node, ACCOUNT_0);
    let review = plan.current_review().unwrap();
    assert_eq!(review.flags, vec![ReviewFlag::SelfTransfer]);
    assert!(review.can_confirm);
}

#[test]
fn deploy_wait_gives_up_after_two_hundred_attempts() {
    let host = Host::new();
    let mut node = Node::sepolia(false);
    let mut plan = reviewed(&host, &mut node, ACCOUNT_1);
    node.status = json!({"finality_status": "RECEIVED"});
    let step = plan.advance(&host, StepReply::Confirm).unwrap();
    let step = run_until(&mut plan, &host, &mut node, step, |_, w| w);
    let (id, method, params, _) = parts(&step);
    let deploy_hash = latest_hash(&host);
    let mut step = plan
        .advance(
            &host,
            reply(id, &node.reply(&method, &params, Some(deploy_hash.clone()))),
        )
        .unwrap();
    let mut polls = 0;
    loop {
        match &step {
            SendStep::Wait { delay_ms } => {
                assert_eq!(*delay_ms, 5_000);
                step = plan.advance(&host, StepReply::WaitElapsed).unwrap();
            }
            SendStep::Rpc { .. } => {
                let (id, method, params, _) = parts(&step);
                assert_eq!(method, "starknet_getTransactionStatus");
                polls += 1;
                step = plan
                    .advance(&host, reply(id, &node.reply(&method, &params, None)))
                    .unwrap();
            }
            _ => break,
        }
    }
    assert_eq!(polls, 200);
    assert_eq!(
        failure(step),
        SendFailure {
            code: Code::DeploymentUnconfirmed,
            hash: Some(deploy_hash),
            detail: None,
        }
    );
    assert_eq!(plan.signatures_made(), 1, "the invoke was never signed");
}

#[test]
fn unknown_deployment_state_fails_the_review_closed() {
    let host = Host::new();
    let mut plan = transfer_plan(&host, ACCOUNT_1);
    let (id, ..) = parts(&plan.advance(&host, StepReply::Start).unwrap());
    let chain = json!({"result": "0x534e5f5345504f4c4941"});
    let (id2, method, ..) = parts(&plan.advance(&host, reply(id, &chain)).unwrap());
    assert_eq!(method, "starknet_getClassHashAt");
    let odd = json!({"error": {"code": 1, "message": "internal"}});
    assert_eq!(
        plan.advance(&host, reply(id2, &odd)).unwrap(),
        SendStep::Review
    );
    let review = plan.current_review().unwrap();
    assert!(
        review.requires_deployment
            && review.fee_estimate_failed
            && review.deployment_fee_estimate_failed
            && !review.can_confirm
    );
    assert_eq!(
        plan.advance(&host, StepReply::Confirm),
        Err(SendError::OutOfOrder)
    );
}

#[test]
fn bulk_estimate_without_the_invoke_entry_fails_the_fee() {
    let host = Host::new();
    let mut node = Node::sepolia(false);
    let mut plan = transfer_plan(&host, ACCOUNT_1);
    let first = plan.advance(&host, StepReply::Start).unwrap();
    let step = run_until(&mut plan, &host, &mut node, first, |m, _| {
        m == "starknet_estimateFee"
    });
    let (id, ..) = parts(&step);
    let only_deploy = json!({"result": [node.deploy_estimate.clone()]});
    let step = plan.advance(&host, reply(id, &only_deploy)).unwrap();
    assert_eq!(
        run_until(&mut plan, &host, &mut node, step, never),
        SendStep::Review
    );
    let review = plan.current_review().unwrap();
    assert!(review.fee_estimate_failed && review.deployment_fee.is_some() && !review.can_confirm);
}

#[test]
fn wrong_chain_and_out_of_order_replies_close_the_plan() {
    let host = Host::new();
    let mainnet = json!({"result": "0x534e5f4d41494e"});
    let sepolia = json!({"result": "0x534e5f5345504f4c4941"});

    let mut plan = transfer_plan(&host, ACCOUNT_1);
    let (id, ..) = parts(&plan.advance(&host, StepReply::Start).unwrap());
    assert_eq!(
        failure(plan.advance(&host, reply(id, &mainnet)).unwrap()).code,
        Code::WrongChain
    );

    let mut plan = transfer_plan(&host, ACCOUNT_1);
    assert_eq!(
        plan.advance(&host, StepReply::Confirm),
        Err(SendError::OutOfOrder)
    );
    assert_eq!(
        plan.advance(&host, StepReply::Start),
        Err(SendError::PlanClosed)
    );

    let mut plan = transfer_plan(&host, ACCOUNT_1);
    let (id, ..) = parts(&plan.advance(&host, StepReply::Start).unwrap());
    assert_eq!(
        plan.advance(&host, reply(id + 1, &sepolia)),
        Err(SendError::OutOfOrder)
    );

    // A wrong chain at Confirm fails before anything is signed.
    let mut node = Node::sepolia(true);
    let mut plan = reviewed(&host, &mut node, ACCOUNT_1);
    let (_, method, ..) = parts(&plan.advance(&host, StepReply::Confirm).unwrap());
    assert_eq!(method, "starknet_chainId");
    assert_eq!(
        plan.advance(&host, StepReply::Confirm),
        Err(SendError::OutOfOrder)
    );
    let mut plan = reviewed(&host, &mut node, ACCOUNT_1);
    let (id2, ..) = parts(&plan.advance(&host, StepReply::Confirm).unwrap());
    assert_eq!(
        failure(plan.advance(&host, reply(id2, &mainnet)).unwrap()).code,
        Code::WrongChain
    );
    assert_eq!(plan.signatures_made(), 0);
}

#[test]
fn a_deployment_that_does_not_reproduce_the_address_is_never_offered() {
    let host = Host::new();
    let mut account = oz_account(&host);
    // A wrong salt would deploy a different contract than the one the invoke targets.
    account.deployment.as_mut().unwrap().salt = Felt::ONE;
    let transfer = transfer(&account, ACCOUNT_1, &[]).unwrap();
    let mut plan = Plan::new(
        account,
        Kind::Transfer(Box::new(transfer)),
        bramble_core::SEND_POLICY,
    );
    let mut node = Node::sepolia(false);
    let first = plan.advance(&host, StepReply::Start).unwrap();
    assert_eq!(
        run_until(&mut plan, &host, &mut node, first, never),
        SendStep::Review
    );
    let review = plan.current_review().unwrap();
    assert!(review.requires_deployment && review.deployment_fee_estimate_failed);
    assert!(!review.can_confirm);
}
