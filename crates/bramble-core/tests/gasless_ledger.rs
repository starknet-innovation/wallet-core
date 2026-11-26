//! P8 with P14: a Ledger account's transfer pays its fee in USDC, its outside execution signed on
//! the device inside its plan (the extension's `accountHashSigner` over `executeReviewedGasless`),
//! against a scripted device holding a software key. Only the hash of the build the core checked
//! against the reviewed transfer reaches the device, and only a signature that verified on the
//! account's stored key reaches the relay, after its journal row.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

mod gasless_export_support;
mod send_support;

use std::sync::Arc;

use bramble_core::ledger::LedgerScheme;
use bramble_core::{
    AccountFamily, GaslessFee, GaslessFeeError, GaslessSubmitStep, LedgerAccountRequest, NetworkId,
    SendStep, StepReply, TransferPlan, TransferRequest, WalletCore,
};
use gasless_export_support::*;
use serde_json::{json, Value};
use starknet_wallet_core::ledger::TransportFailure;
use starknet_wallet_core::{felt, kms, sign_stark_hash, Felt, SecretFelt};

const OK: [u8; 2] = [0x90, 0x00];

/// A scripted Starknet app holding `key`.
struct Device {
    key: SecretFelt,
    /// Every hash it signed.
    signed: Vec<Felt>,
    /// It signs another hash than the one sent (a faulty device).
    tamper: bool,
    /// It answers `signHash` with this refusal, once.
    refuse: Option<[u8; 2]>,
}

impl Device {
    fn holding(key: u64) -> Self {
        Self {
            key: SecretFelt::new(Felt::from(key)),
            signed: Vec::new(),
            tamper: false,
            refuse: None,
        }
    }

    fn public_key(&self) -> Felt {
        felt::parse(&sign_stark_hash(&self.key, "0x1").unwrap().public_key).unwrap()
    }

    fn answer(&mut self, apdu: &[u8]) -> Vec<u8> {
        match (apdu[1], apdu[2]) {
            (0x00, _) => [&[2, 4, 0][..], &OK].concat(),
            (0x01, _) => [
                &[0x04][..],
                &self.public_key().to_bytes_be(),
                &[0x22; 32],
                &OK,
            ]
            .concat(),
            (0x02, 0) => OK.to_vec(),
            (0x02, 1) => {
                if let Some(status) = self.refuse.take() {
                    return status.to_vec();
                }
                let mut hash = Felt::from_bytes_be_slice(&apdu[5..37]);
                if self.tamper {
                    hash += Felt::ONE;
                }
                self.signed.push(hash);
                let signature = sign_stark_hash(&self.key, &felt::canonical(&hash)).unwrap();
                let (r, s) = (
                    felt::parse(&signature.r).unwrap(),
                    felt::parse(&signature.s).unwrap(),
                );
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

const KEY: u64 = 0x5eed_1ed6;

/// A Mainnet Ledger OpenZeppelin account on `KEY`, with no STRK to pay its fee.
struct Ledger {
    _dir: tempfile::TempDir,
    core: Arc<WalletCore>,
    id: String,
    address: Felt,
    plan: Arc<TransferPlan>,
}

fn ledger() -> Ledger {
    let (dir, core) = wallet();
    let record = core
        .add_ledger_account(LedgerAccountRequest {
            network: NetworkId::Mainnet,
            family: AccountFamily::OpenZeppelin,
            scheme: LedgerScheme::Bramble,
            account_index: 0,
            public_key_x: felt::canonical(&Device::holding(KEY).public_key()),
        })
        .unwrap();
    assert!(record.ledger);
    let request = TransferRequest {
        network: NetworkId::Mainnet,
        account_id: record.id.clone(),
        token_address: ETH.into(),
        recipient: ACCOUNT_1.into(),
        amount: "0.001".into(),
        fee_balance: Some("0".into()),
        tracked_tokens: vec![],
    };
    let plan = core.clone().prepare_transfer(request).unwrap();
    let first = plan.advance(StepReply::Start).unwrap();
    let mut node = Node::new(NetworkId::Mainnet, true);
    assert_eq!(
        run_until(&plan, &core, &mut node, first, never),
        SendStep::Review
    );
    Ledger {
        address: felt::parse(record.id.split_once(':').unwrap().1).unwrap(),
        _dir: dir,
        core,
        id: record.id,
        plan,
    }
}

/// Chooses USDC and hands the gateway's honest build back: the first device step.
fn chosen(ledger: &Ledger) -> (Arc<GaslessFee>, GaslessSubmitStep) {
    let (fee, offer) = offered(&ledger.plan);
    post(fee.choose(offer, Some(prices()), NOW).unwrap());
    let step = fee.accept_submit_reply(built(&typed()), NOW).unwrap();
    (fee, step)
}

fn typed() -> Value {
    typed_data(AMOUNT, "0x3e8")
}

/// Answers device steps from `device` until another step (or an error) comes back.
fn run(
    fee: &GaslessFee,
    device: &mut Device,
    mut step: GaslessSubmitStep,
) -> Result<GaslessSubmitStep, GaslessFeeError> {
    while let GaslessSubmitStep::Device { apdu } = &step {
        let reply = device.answer(apdu);
        step = fee.advance_device(StepReply::Device { reply }, NOW)?;
    }
    Ok(step)
}

#[test]
fn a_ledger_account_pays_in_usdc_signed_on_its_device_and_journaled_first() {
    let ledger = ledger();
    let (fee, step) = chosen(&ledger);
    let mut device = Device::holding(KEY);
    let execute = post(run(&fee, &mut device, step).unwrap());
    let body: Value = serde_json::from_str(execute.body.as_deref().unwrap()).unwrap();
    assert_eq!(body["method"], "paymaster_executeTransaction");
    let invoke = &body["params"]["transaction"]["invoke"];
    assert_eq!(invoke["typed_data"], typed());
    // The device signed the checked execution's hash, once, and the relay gets that signature.
    let hash = kms::typed_data_message_hash(&typed().to_string(), &ledger.address).unwrap();
    assert_eq!(device.signed, [hash]);
    assert_eq!(ledger.plan.signatures_made(), 1);
    let signature: Vec<Felt> = invoke["signature"]
        .as_array()
        .unwrap()
        .iter()
        .map(|felt| felt::parse(felt.as_str().unwrap()).unwrap())
        .collect();
    assert_eq!(signature.len(), 2);
    assert!(kms::stark_signature_is_valid(
        &device.public_key(),
        &hash,
        &signature[0],
        &signature[1]
    ));
    // Journaled before the execute was handed out, then settled by the relay's answer.
    let rows = ledger.core.activity(ledger.id.clone()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].hash, felt::canonical(&hash));
    assert!(rows[0].relay.is_some());
    let submitted = fee
        .accept_submit_reply(result(json!({"transaction_hash": "0x7a1ce5"})), NOW)
        .unwrap();
    assert!(matches!(submitted, GaslessSubmitStep::Submitted { .. }));
    let rows = ledger.core.activity(ledger.id.clone()).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].hash, "0x7a1ce5");
    assert!(rows[0].relay.is_none());
    // The plan's STRK path stays shut, and its device steps belong to the gasless path alone.
    assert!(ledger.plan.advance(StepReply::Confirm).is_err());
}

#[test]
fn another_device_parks_the_signature_until_the_right_one_signs() {
    let ledger = ledger();
    let (fee, step) = chosen(&ledger);
    let mut other = Device::holding(0x1234);
    assert_eq!(
        run(&fee, &mut other, step),
        Ok(GaslessSubmitStep::DeviceRetry {
            reason: "wrong-device".into()
        })
    );
    assert!(other.signed.is_empty());
    assert!(ledger.core.activity(ledger.id.clone()).unwrap().is_empty());
    let step = fee.advance_device(StepReply::RetryDevice, NOW).unwrap();
    let mut right = Device::holding(KEY);
    let execute = post(run(&fee, &mut right, step).unwrap());
    assert_eq!(method(&execute), "paymaster_executeTransaction");
    assert_eq!(right.signed.len(), 1);
}

#[test]
fn a_refusal_on_the_device_or_a_lost_link_parks_it_and_a_retry_signs() {
    let ledger = ledger();
    let (fee, step) = chosen(&ledger);
    let mut device = Device::holding(KEY);
    device.refuse = Some([0x6e, 0x04]);
    assert_eq!(
        run(&fee, &mut device, step),
        Ok(GaslessSubmitStep::DeviceRetry {
            reason: "rejected-on-device".into()
        })
    );
    let step = fee.advance_device(StepReply::RetryDevice, NOW).unwrap();
    assert!(matches!(step, GaslessSubmitStep::Device { .. }));
    let step = fee
        .advance_device(
            StepReply::DeviceFailure {
                failure: TransportFailure::Locked,
            },
            NOW,
        )
        .unwrap();
    assert_eq!(
        step,
        GaslessSubmitStep::DeviceRetry {
            reason: "device-locked".into()
        }
    );
    let step = fee.advance_device(StepReply::RetryDevice, NOW).unwrap();
    post(run(&fee, &mut device, step).unwrap());
    assert_eq!(device.signed.len(), 1);
}

#[test]
fn a_lock_mid_signing_stops_the_device_and_nothing_is_journaled() {
    let ledger = ledger();
    let (fee, step) = chosen(&ledger);
    let mut device = Device::holding(KEY);
    let GaslessSubmitStep::Device { apdu } = step else {
        panic!("expected the device's first APDU, got {step:?}");
    };
    ledger.core.lock();
    let reply = device.answer(&apdu);
    assert_eq!(
        fee.advance_device(StepReply::Device { reply }, NOW),
        Err(GaslessFeeError::Locked)
    );
    assert!(device.signed.is_empty(), "no signHash was sent");
    assert_eq!(
        fee.advance_device(StepReply::RetryDevice, NOW),
        Err(GaslessFeeError::NotFound)
    );
}

#[test]
fn a_signature_that_does_not_verify_is_refused_and_never_posted() {
    let ledger = ledger();
    let (fee, step) = chosen(&ledger);
    let mut faulty = Device::holding(KEY);
    faulty.tamper = true;
    assert_eq!(
        run(&fee, &mut faulty, step),
        Err(GaslessFeeError::Refused {
            reason: "Ledger signature verification failed".into(),
            retryable: false
        })
    );
    assert_eq!(ledger.plan.signatures_made(), 0);
    assert!(ledger.core.activity(ledger.id.clone()).unwrap().is_empty());
    assert_eq!(
        fee.accept_submit_reply(result(json!({"transaction_hash": "0x1"})), NOW),
        Err(GaslessFeeError::NotFound)
    );
}

#[test]
fn a_tampered_build_is_refused_before_the_device_is_asked() {
    let ledger = ledger();
    let (fee, offer) = offered(&ledger.plan);
    post(fee.choose(offer, Some(prices()), NOW).unwrap());
    let moved = typed_data("0x38d7ea4c68001", "0x3e8");
    assert_eq!(
        fee.accept_submit_reply(built(&moved), NOW),
        Err(GaslessFeeError::Refused {
            reason: "This network fee quote was refused. Nothing was submitted.".into(),
            retryable: false
        })
    );
    // No device step was handed out, so none is taken.
    assert_eq!(
        fee.advance_device(StepReply::Device { reply: OK.to_vec() }, NOW),
        Err(GaslessFeeError::NotFound)
    );
    assert_eq!(ledger.plan.signatures_made(), 0);
}

#[test]
fn a_software_account_takes_no_device_reply() {
    let (_dir, core) = wallet();
    let plan = transfer(&core, ACCOUNT_1, "0");
    let (fee, offer) = offered(&plan);
    post(fee.choose(offer, Some(prices()), NOW).unwrap());
    let execute = post(fee.accept_submit_reply(built(&typed()), NOW).unwrap());
    assert_eq!(method(&execute), "paymaster_executeTransaction");
    assert_eq!(
        fee.advance_device(StepReply::RetryDevice, NOW),
        Err(GaslessFeeError::NotFound)
    );
}

#[test]
fn the_plans_own_advance_cannot_drive_the_relay_signature() {
    let ledger = ledger();
    let (fee, step) = chosen(&ledger);
    let GaslessSubmitStep::Device { apdu } = step else {
        panic!("expected the device's first APDU, got {step:?}");
    };
    let reply = Device::holding(KEY).answer(&apdu);
    assert_eq!(
        ledger.plan.advance(StepReply::Device {
            reply: reply.clone()
        }),
        Err(bramble_core::SendError::OutOfOrder)
    );
    // Refused there, the plan is closed: the relay signature is over too.
    assert_eq!(
        fee.advance_device(StepReply::Device { reply }, NOW),
        Err(GaslessFeeError::NotFound)
    );
    assert_eq!(ledger.plan.signatures_made(), 0);
}

/// Answers every APDU but the device's signature, which it returns unsent.
fn until_signature(fee: &GaslessFee, device: &mut Device, mut step: GaslessSubmitStep) -> Vec<u8> {
    loop {
        let GaslessSubmitStep::Device { apdu } = &step else {
            panic!("expected an APDU, got {step:?}");
        };
        let reply = device.answer(apdu);
        if apdu[1] == 0x02 && apdu[2] == 1 {
            return reply;
        }
        step = fee
            .advance_device(StepReply::Device { reply }, NOW)
            .unwrap();
    }
}

#[test]
fn a_window_that_lapses_on_the_device_is_refused_and_files_no_row() {
    let ledger = ledger();
    let (fee, step) = chosen(&ledger);
    let mut device = Device::holding(KEY);
    let reply = until_signature(&fee, &mut device, step);
    let late = NOW + 600_000;
    assert_eq!(
        fee.advance_device(StepReply::Device { reply }, late),
        Err(GaslessFeeError::Refused {
            reason: "This network fee quote was refused. Nothing was submitted.".into(),
            retryable: false
        })
    );
    assert!(ledger.core.activity(ledger.id.clone()).unwrap().is_empty());
    assert_eq!(
        fee.accept_submit_reply(result(json!({"transaction_hash": "0x1"})), NOW),
        Err(GaslessFeeError::NotFound)
    );
}

#[cfg(unix)]
#[test]
fn a_device_signature_that_cannot_be_journaled_is_never_posted() {
    use std::os::unix::fs::PermissionsExt;
    let ledger = ledger();
    let (fee, step) = chosen(&ledger);
    let mut device = Device::holding(KEY);
    let reply = until_signature(&fee, &mut device, step);
    let dir = ledger._dir.path();
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    let refused = fee.advance_device(StepReply::Device { reply }, NOW);
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        refused,
        Err(GaslessFeeError::Refused {
            reason: "The transaction could not be recorded before sending. Nothing was submitted — try again.".into(),
            retryable: false
        })
    );
    assert!(ledger.core.activity(ledger.id.clone()).unwrap().is_empty());
    assert_eq!(
        fee.accept_submit_reply(result(json!({"transaction_hash": "0x1"})), NOW),
        Err(GaslessFeeError::NotFound)
    );
}

#[test]
fn a_paymaster_reply_while_the_device_signs_ends_both() {
    let ledger = ledger();
    let (fee, step) = chosen(&ledger);
    assert!(matches!(step, GaslessSubmitStep::Device { .. }));
    assert!(fee
        .accept_submit_reply(result(json!({"transaction_hash": "0x1"})), NOW)
        .is_err());
    // The device's signature is no longer wanted: the plan closed with the submission.
    assert_eq!(
        fee.advance_device(StepReply::RetryDevice, NOW),
        Err(GaslessFeeError::NotFound)
    );
    assert_eq!(ledger.plan.signatures_made(), 0);
    assert!(ledger.core.activity(ledger.id.clone()).unwrap().is_empty());
}

#[test]
fn a_plan_cancelled_before_the_build_returns_never_reaches_a_signer() {
    let ledger = ledger();
    let (fee, offer) = offered(&ledger.plan);
    post(fee.choose(offer, Some(prices()), NOW).unwrap());
    ledger.plan.close();
    // Not the key path (which would read as a lock): the device plan refuses on its own.
    assert_eq!(
        fee.accept_submit_reply(built(&typed()), NOW),
        Err(GaslessFeeError::NotFound)
    );
    assert_eq!(ledger.plan.signatures_made(), 0);
    assert!(ledger.core.activity(ledger.id.clone()).unwrap().is_empty());
}

#[test]
fn a_signature_returned_after_a_lock_files_no_row() {
    let ledger = ledger();
    let (fee, step) = chosen(&ledger);
    let mut device = Device::holding(KEY);
    let reply = until_signature(&fee, &mut device, step);
    assert_eq!(device.signed.len(), 1, "the device signed");
    ledger.core.lock();
    assert_eq!(
        fee.advance_device(StepReply::Device { reply }, NOW),
        Err(GaslessFeeError::Locked)
    );
    assert_eq!(ledger.plan.signatures_made(), 0);
    ledger.core.unlock(PASSWORD.into()).unwrap();
    assert!(ledger.core.activity(ledger.id.clone()).unwrap().is_empty());
}
