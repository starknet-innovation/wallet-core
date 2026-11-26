//! A Ledger account's send (P8): the plan hands its hashes to a scripted device, never to
//! `PlanHost::sign`, and only a signature that verified against the account's key is written.

use crate::plan_support;

use bramble_core::ledger::{ledger_account_path, LEDGER_APP_MIN};
use plan_support::node::*;
use plan_support::*;
use starknet_wallet_core::ledger::TransportFailure;
use starknet_wallet_core::send::plan::{Kind, Plan, Token, Transfer};
use starknet_wallet_core::send::plan_device::DeviceKey;
use starknet_wallet_core::send::types::{SendError, SendFailureCode as Code, SendStep, StepReply};
use starknet_wallet_core::send::u256::U256;
use starknet_wallet_core::{
    derive_starknet_keypair, felt, sign_stark_hash, stark_signature_is_valid, Felt, SecretFelt,
};

/// A scripted Starknet app holding `key`.
struct Device {
    key: SecretFelt,
    /// Every hash it signed.
    signed: Vec<Felt>,
    /// From this many signatures on, it signs another hash than the one sent (a faulty device).
    tamper_from: usize,
}

impl Device {
    /// The device holding the published phrase's account 0 key, the test `Host`'s.
    fn same_as_host() -> Self {
        let phrase = capture("wire")["mnemonic"].as_str().unwrap().to_string();
        Self::holding(
            derive_starknet_keypair(&phrase, 0, 0, None)
                .unwrap()
                .private_key,
        )
    }

    fn holding(key: SecretFelt) -> Self {
        Self {
            key,
            signed: Vec::new(),
            tamper_from: usize::MAX,
        }
    }

    fn answer(&mut self, apdu: &[u8]) -> Vec<u8> {
        const OK: [u8; 2] = [0x90, 0x00];
        match (apdu[1], apdu[2]) {
            (0x00, _) => [&[2, 4, 0][..], &OK].concat(),
            (0x01, _) => {
                let x = f(&sign_stark_hash(&self.key, "0x1").unwrap().public_key);
                [&[0x04][..], &x.to_bytes_be(), &[0x22; 32], &OK].concat()
            }
            (0x02, 0) => OK.to_vec(),
            (0x02, 1) => {
                let mut hash = Felt::from_bytes_be_slice(&apdu[5..37]);
                if self.signed.len() >= self.tamper_from {
                    hash += Felt::ONE;
                }
                self.signed.push(hash);
                let signature = sign_stark_hash(&self.key, &felt::canonical(&hash)).unwrap();
                let (r, s) = (f(&signature.r), f(&signature.s));
                [
                    &[0x41][..],
                    &r.to_bytes_be(),
                    &s.to_bytes_be(),
                    &[0x00],
                    &OK,
                ]
                .concat()
            }
            other => panic!("unexpected APDU {other:?}"),
        }
    }
}

fn device_key() -> DeviceKey {
    DeviceKey {
        path: ledger_account_path("LedgerW", 0).unwrap(),
        app_minimum: LEDGER_APP_MIN,
    }
}

/// A reviewed transfer from account 0, signed on a device.
fn reviewed_on_device(host: &Host, node: &mut Node) -> Plan {
    let mut plan = transfer_plan(host, ACCOUNT_1).with_device(device_key());
    let first = plan.advance(host, StepReply::Start).unwrap();
    assert_eq!(
        run_until(&mut plan, host, node, first, never),
        SendStep::Review
    );
    plan
}

/// Answers RPCs from `node` and APDUs from `device` until `stop` holds for an RPC or a step
/// neither answers.
fn run_to(
    plan: &mut Plan,
    host: &Host,
    node: &mut Node,
    device: &mut Device,
    mut step: SendStep,
    stop: impl Fn(&str, bool) -> bool + Copy,
) -> SendStep {
    loop {
        step = match run_until(plan, host, node, step, stop) {
            SendStep::Device { apdu } => plan
                .advance(
                    host,
                    StepReply::Device {
                        reply: device.answer(&apdu),
                    },
                )
                .unwrap(),
            other => return other,
        };
    }
}

fn run(
    plan: &mut Plan,
    host: &Host,
    node: &mut Node,
    device: &mut Device,
    step: SendStep,
) -> SendStep {
    run_to(plan, host, node, device, step, never)
}

/// The signature a write carried: `(r, s)` for an OpenZeppelin account.
fn written_signature(params: &serde_json::Value) -> (Felt, Felt) {
    let tx = params
        .get("invoke_transaction")
        .or_else(|| params.get("deploy_account_transaction"))
        .unwrap();
    let signature = tx["signature"].as_array().unwrap();
    assert_eq!(signature.len(), 2);
    (
        f(signature[0].as_str().unwrap()),
        f(signature[1].as_str().unwrap()),
    )
}

#[test]
fn a_deployed_ledger_account_signs_on_its_device_only() {
    let host = Host::new();
    let mut node = Node::sepolia(true);
    let mut device = Device::same_as_host();
    let mut plan = reviewed_on_device(&host, &mut node);
    let step = plan.advance(&host, StepReply::Confirm).unwrap();
    // Stop at the write to read what it carries.
    let step = run_to(&mut plan, &host, &mut node, &mut device, step, |_, w| w);
    let (_, method, params, _) = parts(&step);
    assert_eq!(method, "starknet_addInvokeTransaction");
    assert!(
        host.signed.borrow().is_empty(),
        "the host's key never signs"
    );
    assert_eq!(plan.signatures_made(), 1);
    let [hash] = device.signed[..] else {
        panic!("one hash signed on the device");
    };
    assert_eq!(felt::canonical(&hash), latest_hash(&host));
    let (r, s) = written_signature(&params);
    assert!(stark_signature_is_valid(&host.public_key(), &hash, &r, &s));
}

#[test]
fn an_undeployed_ledger_account_signs_its_deployment_and_invoke_on_the_device() {
    let host = Host::new();
    let mut node = Node::sepolia(false);
    let mut device = Device::same_as_host();
    let mut plan = reviewed_on_device(&host, &mut node);
    assert!(plan.current_review().unwrap().requires_deployment);
    let step = plan.advance(&host, StepReply::Confirm).unwrap();
    let SendStep::Broadcast { hash } = run(&mut plan, &host, &mut node, &mut device, step) else {
        panic!("expected the invoke's broadcast");
    };
    assert!(host.signed.borrow().is_empty());
    assert_eq!(plan.signatures_made(), 2);
    let records = host.records.borrow();
    assert_eq!(device.signed, [records[0].hash, records[1].hash]);
    assert_eq!(felt::canonical(&records[1].hash), hash);
}

/// Confirms, then answers the device with `device` until the plan ends or writes.
fn confirm_with(device: &mut Device) -> (SendStep, Host, Plan) {
    let host = Host::new();
    let mut node = Node::sepolia(true);
    let mut plan = reviewed_on_device(&host, &mut node);
    let step = plan.advance(&host, StepReply::Confirm).unwrap();
    let step = run(&mut plan, &host, &mut node, device, step);
    (step, host, plan)
}

#[test]
fn another_device_parks_the_send_until_the_right_one_signs() {
    let mut other = Device::holding(SecretFelt::new(Felt::from(0x1234_u64)));
    let host = Host::new();
    let mut node = Node::sepolia(true);
    let mut plan = reviewed_on_device(&host, &mut node);
    let step = plan.advance(&host, StepReply::Confirm).unwrap();
    let step = run(&mut plan, &host, &mut node, &mut other, step);
    assert_eq!(
        step,
        SendStep::DeviceRetry {
            reason: "wrong-device".into()
        }
    );
    assert!(other.signed.is_empty());
    assert!(host.records.borrow().is_empty() && host.signed.borrow().is_empty());
    assert_eq!(plan.interrupted_failure(), None);
    // The person plugs in the account's own device and retries.
    let mut right = Device::same_as_host();
    let step = plan.advance(&host, StepReply::RetryDevice).unwrap();
    let SendStep::Broadcast { hash } = run(&mut plan, &host, &mut node, &mut right, step) else {
        panic!("expected the broadcast after the retry");
    };
    assert_eq!(felt::canonical(&right.signed[0]), hash);
    assert_eq!(plan.signatures_made(), 1);
}

#[test]
fn a_transport_failure_parks_the_send_and_a_retry_signs_from_the_start() {
    let host = Host::new();
    let mut node = Node::sepolia(true);
    let mut device = Device::same_as_host();
    let mut plan = reviewed_on_device(&host, &mut node);
    let step = plan.advance(&host, StepReply::Confirm).unwrap();
    let SendStep::Device { .. } = run_until(&mut plan, &host, &mut node, step, never) else {
        panic!("expected the device's first APDU");
    };
    let step = plan
        .advance(
            &host,
            StepReply::DeviceFailure {
                failure: TransportFailure::Locked,
            },
        )
        .unwrap();
    assert_eq!(
        step,
        SendStep::DeviceRetry {
            reason: "device-locked".into()
        }
    );
    assert!(host.records.borrow().is_empty());
    let step = plan.advance(&host, StepReply::RetryDevice).unwrap();
    let SendStep::Device { apdu } = &step else {
        panic!("expected the first APDU again, got {step:?}");
    };
    assert_eq!(apdu[1], 0x00, "a retry starts at the app version");
    let SendStep::Broadcast { .. } = run(&mut plan, &host, &mut node, &mut device, step) else {
        panic!("expected the broadcast after the retry");
    };
}

#[test]
fn a_signature_that_does_not_verify_ends_the_send() {
    let mut faulty = Device::same_as_host();
    faulty.tamper_from = 0;
    let (step, host, plan) = confirm_with(&mut faulty);
    let failure = failure(step);
    assert_eq!(failure.code, Code::DeviceRefused);
    assert_eq!(failure.detail.as_deref(), Some("signature-verify-failed"));
    assert_eq!(failure.hash, None);
    assert!(host.records.borrow().is_empty());
    assert_eq!(plan.signatures_made(), 0);
}

#[test]
fn a_refusal_after_the_automatic_deployment_still_names_the_device() {
    let host = Host::new();
    let mut node = Node::sepolia(false);
    let mut device = Device::same_as_host();
    device.tamper_from = 1;
    let mut plan = reviewed_on_device(&host, &mut node);
    let step = plan.advance(&host, StepReply::Confirm).unwrap();
    let failure = failure(run(&mut plan, &host, &mut node, &mut device, step));
    let deployment = host.records.borrow()[0].hash;
    assert_eq!(failure.code, Code::DeploymentThenInvokeUnconfirmed);
    assert_eq!(failure.hash, Some(felt::canonical(&deployment)));
    assert_eq!(failure.detail.as_deref(), Some("signature-verify-failed"));
    assert_eq!(
        host.records.borrow().len(),
        1,
        "the invoke was never written"
    );
}

#[test]
fn a_lock_stops_the_device_before_it_asks_the_person() {
    let host = Host::new();
    let mut node = Node::sepolia(true);
    let mut device = Device::same_as_host();
    let mut plan = reviewed_on_device(&host, &mut node);
    let step = plan.advance(&host, StepReply::Confirm).unwrap();
    let SendStep::Device { apdu } = run_until(&mut plan, &host, &mut node, step, never) else {
        panic!("expected the device's first APDU");
    };
    host.session.set(false);
    let reply = device.answer(&apdu);
    let step = plan.advance(&host, StepReply::Device { reply }).unwrap();
    assert_eq!(failure(step).code, Code::Locked);
    assert!(device.signed.is_empty(), "no signHash was sent");
    // A parked plan is refused a retry once locked as well.
    let host = Host::new();
    let mut other = Device::holding(SecretFelt::new(Felt::from(0x1234_u64)));
    let mut plan = reviewed_on_device(&host, &mut node);
    let step = plan.advance(&host, StepReply::Confirm).unwrap();
    run(&mut plan, &host, &mut node, &mut other, step);
    host.session.set(false);
    let step = plan.advance(&host, StepReply::RetryDevice).unwrap();
    assert_eq!(failure(step).code, Code::Locked);
}

#[test]
fn a_ledger_send_is_offered_gasless_as_any_other() {
    // A transfer whose STRK cannot pay its fee qualifies for gasless, on a device too: its outside
    // execution is signed there (`gasless_ledger.rs`).
    let host = Host::new();
    let account = oz_account(&host);
    let token = Token {
        address: f(STRK),
        symbol: "STRK",
        decimals: 18,
    };
    let short = |device: bool| {
        let transfer = Transfer::new(
            &account,
            token,
            ACCOUNT_1,
            "1.5",
            &[],
            &[f(STRK), f(ETH)],
            Some(U256::ZERO),
        )
        .unwrap();
        let mut plan = Plan::new(
            account.clone(),
            Kind::Transfer(Box::new(transfer)),
            bramble_core::SEND_POLICY,
        );
        if device {
            plan = plan.with_device(device_key());
        }
        let mut node = Node::sepolia(true);
        let first = plan.advance(&host, StepReply::Start).unwrap();
        assert_eq!(
            run_until(&mut plan, &host, &mut node, first, never),
            SendStep::Review
        );
        plan.gasless_basis().is_some()
    };
    assert!(short(false));
    assert!(short(true));
}

#[test]
fn a_signature_returned_after_a_lock_is_dropped() {
    let host = Host::new();
    let mut node = Node::sepolia(true);
    let mut device = Device::same_as_host();
    let mut plan = reviewed_on_device(&host, &mut node);
    let mut step = plan.advance(&host, StepReply::Confirm).unwrap();
    // Answer every APDU but the last, lock, then hand back the signature.
    step = run_until(&mut plan, &host, &mut node, step, never);
    loop {
        let SendStep::Device { apdu } = &step else {
            panic!("expected an APDU, got {step:?}");
        };
        let reply = device.answer(apdu);
        if apdu[1] == 0x02 && apdu[2] == 1 {
            host.session.set(false);
            step = plan.advance(&host, StepReply::Device { reply }).unwrap();
            break;
        }
        step = plan.advance(&host, StepReply::Device { reply }).unwrap();
    }
    assert_eq!(failure(step).code, Code::Locked);
    assert!(host.records.borrow().is_empty());
    assert_eq!(plan.signatures_made(), 0);
}

#[test]
fn a_device_step_takes_only_a_device_reply() {
    let host = Host::new();
    let mut node = Node::sepolia(true);
    let mut plan = reviewed_on_device(&host, &mut node);
    let step = plan.advance(&host, StepReply::Confirm).unwrap();
    let SendStep::Device { .. } = run_until(&mut plan, &host, &mut node, step, never) else {
        panic!("expected the device's first APDU");
    };
    assert_eq!(
        plan.advance(&host, StepReply::Confirm),
        Err(SendError::OutOfOrder)
    );
    // A device reply where none is expected is refused as well.
    let host = Host::new();
    let mut plan = transfer_plan(&host, ACCOUNT_1);
    plan.advance(&host, StepReply::Start).unwrap();
    assert_eq!(
        plan.advance(
            &host,
            StepReply::Device {
                reply: vec![0x90, 0x00]
            }
        ),
        Err(SendError::OutOfOrder)
    );
}

#[test]
fn a_braavos_ledger_account_never_reaches_the_hosts_key() {
    // Braavos's deployment review signs through the host; a device account is refused there.
    let braavos = capture("braavos");
    let scenario = braavos["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "braavos-standalone-deployment-SN_SEPOLIA")
        .unwrap();
    let host = Host::new();
    let account = account(scenario, "braavos", host.public_key());
    let mut plan = plan(scenario, account).with_device(device_key());
    let step = plan.advance(&host, StepReply::Start).unwrap();
    // It stops before the signed deployment estimate, the fourth read the extension made.
    let calls = review_calls(scenario);
    assert!(calls[3]["method"] == "starknet_estimateFee");
    let step = drive(&mut plan, &host, step, &calls[..3], scenario);
    assert_eq!(failure(step).code, Code::AccountClassUnsupported);
    assert!(host.signed.borrow().is_empty());
}
