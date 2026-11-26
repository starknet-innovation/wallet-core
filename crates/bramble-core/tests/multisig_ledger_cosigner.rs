//! P8: a Ledger co-signer signs an Argent multisig intent on its device, as the extension's
//! `signIntent` does through `accountHashSigner`. The fixture's last co-signer key is added as a
//! Ledger Argent account (same key, another address), and a scripted device holding that key
//! answers the APDUs. Krusty's signatures are deterministic, so the device's signature is the one
//! the extension's co-signer made and every package is the recorded one.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

use crate::ledger_cosign_support;
use crate::multisig_tx_support;

use crate::common::ledger_device::OK;
use bramble_core::{CoreError, IntentState, MultisigDeviceStep, MultisigTxStep, StepReply};
use ledger_cosign_support::*;
use multisig_tx_support::*;
use starknet_wallet_core::{Felt, SecretFelt};

#[test]
fn a_ledger_cosigner_reaches_the_threshold_and_the_intent_submits() {
    for net in networks() {
        let setup = setup(&net);
        let signing = setup.on_device();
        let mut device = setup.device();
        let step = drive_device(&signing, &mut device, signing.start().unwrap()).unwrap();
        assert_eq!(
            step,
            MultisigDeviceStep::Signed {
                intent_id: setup.intent.id.clone()
            }
        );
        // version, key, sign path, sign hash
        assert_eq!(device.apdus, 4);
        let intent = setup.stored();
        assert_eq!(intent.state, IntentState::Ready);
        assert_eq!(
            intent.package,
            net["proposal"]["readyPackage"].as_str().unwrap()
        );
        // Spent: nothing more reaches the device.
        assert!(signing.start().is_err());
        assert!(signing.reply(OK.to_vec()).is_err());

        let wallet = &setup.wallet;
        let submission = wallet
            .core
            .clone()
            .submit_multisig_intent(intent.id.clone(), wallet.current_state(&intent))
            .unwrap();
        let step = submission.advance(StepReply::Start).unwrap();
        let step = multisig_tx_support::drive(step, &phase(&net["proposal"]["submit"]), |reply| {
            submission.advance(reply).unwrap()
        });
        let hash = net["proposal"]["transactionHash"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(step, MultisigTxStep::Submitted { hash });
    }
}

#[test]
fn each_record_signs_only_its_own_way() {
    let net = &networks()[0];
    let setup = setup(net);
    let state = || setup.wallet.current_state(&setup.intent);
    // A Ledger record has no key to sign with.
    assert!(setup
        .wallet
        .core
        .sign_multisig_intent(setup.intent.id.clone(), setup.ledger_id.clone(), state())
        .is_err());
    // A software record keeps its key path.
    let refused = setup
        .wallet
        .core
        .clone()
        .sign_multisig_intent_on_device(setup.intent.id.clone(), setup.software_id.clone(), state())
        .err()
        .unwrap();
    assert!(
        matches!(refused, CoreError::InvalidInput { reason } if reason == "selected signer is not a Ledger account")
    );
    assert_eq!(setup.stored(), setup.intent);
}

#[test]
fn a_wrong_device_parks_and_the_right_one_signs_on_retry() {
    let net = &networks()[0];
    let setup = setup(net);
    let signing = setup.on_device();
    let mut wrong = Device::holding(SecretFelt::new(Felt::from(0xabcu64)));
    let step = drive_device(&signing, &mut wrong, signing.start().unwrap()).unwrap();
    assert_eq!(
        step,
        MultisigDeviceStep::DeviceRetry {
            reason: "wrong-device".into()
        }
    );
    // Refused before the person is asked: no signHash APDU went out.
    assert_eq!(wrong.apdus, 2);
    assert_eq!(setup.stored(), setup.intent);
    let mut right = setup.device();
    let step = drive_device(&signing, &mut right, signing.retry().unwrap()).unwrap();
    assert!(
        matches!(step, MultisigDeviceStep::Signed { .. }),
        "{step:?}"
    );
    assert_eq!(setup.stored().state, IntentState::Ready);
}

#[test]
fn a_rejection_on_the_device_parks_until_retried() {
    let net = &networks()[0];
    let setup = setup(net);
    // Parked: a stray answer ends it rather than being read (and releases its claim).
    let parked = setup.on_device();
    let mut again = setup.device();
    again.reject = 1;
    drive_device(&parked, &mut again, parked.start().unwrap()).unwrap();
    assert!(parked.reply(OK.to_vec()).is_err());
    assert!(parked.retry().is_err());

    let signing = setup.on_device();
    let mut device = setup.device();
    device.reject = 1;
    let step = drive_device(&signing, &mut device, signing.start().unwrap()).unwrap();
    assert_eq!(
        step,
        MultisigDeviceStep::DeviceRetry {
            reason: "rejected-on-device".into()
        }
    );
    assert_eq!(setup.stored(), setup.intent);
    let step = drive_device(&signing, &mut device, signing.retry().unwrap()).unwrap();
    assert!(
        matches!(step, MultisigDeviceStep::Signed { .. }),
        "{step:?}"
    );
}

#[test]
fn a_lock_mid_signing_stops_the_device() {
    let net = &networks()[0];
    let setup = setup(net);
    let signing = setup.on_device();
    let mut device = setup.device();
    let MultisigDeviceStep::Device { apdu } = signing.start().unwrap() else {
        panic!("expected an APDU");
    };
    let answer = device.answer(&apdu);
    setup.wallet.relock();
    assert!(matches!(signing.reply(answer), Err(CoreError::Locked)));
    // Ended for good, a new session included.
    assert!(signing.reply(OK.to_vec()).is_err());
    assert!(signing.retry().is_err());
    assert_eq!(device.apdus, 1);
    assert_eq!(setup.stored(), setup.intent);
}

#[test]
fn a_signature_that_does_not_verify_is_refused() {
    let net = &networks()[0];
    let setup = setup(net);
    let signing = setup.on_device();
    let mut device = setup.device();
    device.tamper = true;
    let refused = drive_device(&signing, &mut device, signing.start().unwrap())
        .err()
        .unwrap();
    assert!(
        matches!(refused, CoreError::InvalidInput { reason } if reason == "signature-verify-failed")
    );
    assert!(signing.retry().is_err());
    assert_eq!(setup.stored(), setup.intent);
}
