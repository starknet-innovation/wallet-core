//! The mechanism tests' canned node (mobile's): fixed replies per method, and the driver that
//! answers a plan from it until a chosen request.

use serde_json::{json, Value};
use starknet_wallet_core::send::plan::{Account, Authorization, Kind, Plan};
use starknet_wallet_core::send::sign::Deployment;
use starknet_wallet_core::send::types::{SendFailure, SendStep, StepReply};
use starknet_wallet_core::{felt, Felt, NetworkId};

use super::*;

pub const ACCOUNT_0: &str = "0x208361e3911b1d93a704400b88e7e53a89171fce64dbb48f8479e4f44b398d9";

/// The published phrase's OpenZeppelin account 0 on Sepolia.
pub fn oz_account(host: &Host) -> Account {
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

pub fn transfer_plan(host: &Host, recipient: &str) -> Plan {
    let account = oz_account(host);
    let transfer = transfer(&account, recipient, &[]).unwrap();
    Plan::new(
        account,
        Kind::Transfer(Box::new(transfer)),
        bramble_core::SEND_POLICY,
    )
}

/// Mobile's canned node: deployment state, nonce, and the estimate it returns per transaction.
pub struct Node {
    pub deployed: bool,
    pub nonce: String,
    pub invoke_estimate: Value,
    pub deploy_estimate: Value,
    pub status: Value,
}

impl Node {
    pub fn sepolia(deployed: bool) -> Self {
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
    pub fn reply(&mut self, method: &str, params: &Value, signed: Option<String>) -> Value {
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

pub fn parts(step: &SendStep) -> (u64, String, Value, bool) {
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

pub fn latest_hash(host: &Host) -> String {
    felt::canonical(&host.records.borrow().last().expect("a pending record").hash)
}

/// Answers every step from `node` until `stop(method, is_write)` holds or a non-RPC step
/// arrives; returns that step unanswered.
pub fn run_until(
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

pub fn never(_: &str, _: bool) -> bool {
    false
}

pub fn reviewed(host: &Host, node: &mut Node, recipient: &str) -> Plan {
    let mut plan = transfer_plan(host, recipient);
    let first = plan.advance(host, StepReply::Start).unwrap();
    assert_eq!(
        run_until(&mut plan, host, node, first, never),
        SendStep::Review
    );
    plan
}

pub fn failure(step: SendStep) -> SendFailure {
    match step {
        SendStep::Failed { failure } => failure,
        other => panic!("expected a failure, got {other:?}"),
    }
}
