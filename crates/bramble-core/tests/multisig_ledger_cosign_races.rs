//! P8: what can change while a Ledger co-signer confirms on its device. The intent is read again
//! before the device is asked, before a retry and before the signature is stored; one signer's
//! signing of one intent is claimed (`claimOperation("sign", ..)`); a tampered store refuses;
//! an idle lock stops the device.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

mod common;
mod ledger_cosign_support;
mod multisig_support;
mod multisig_tx_support;

use bramble_core::testing::IDLE_LOCK_MS;
use bramble_core::{CoreError, IntentState, MultisigDeviceStep};
use ledger_cosign_support::*;
use serde_json::Value;

const CLAIMED: &str = "this signer already signed or is signing the intent";

fn reason(error: CoreError) -> String {
    match error {
        CoreError::InvalidInput { reason } => reason,
        other => panic!("{other:?}"),
    }
}

/// The first APDU sent and answered: the person is about to be asked.
fn first_answer(
    setup: &Setup,
    device: &mut Device,
) -> (std::sync::Arc<bramble_core::MultisigDeviceSigning>, Vec<u8>) {
    let signing = setup.on_device();
    let MultisigDeviceStep::Device { apdu } = signing.start().unwrap() else {
        panic!("expected an APDU");
    };
    let answer = device.answer(&apdu);
    (signing, answer)
}

#[test]
fn another_signer_reaching_the_threshold_mid_confirmation_refuses_the_store() {
    let setup = setup(&networks()[0]);
    let mut device = setup.device();
    let (signing, answer) = first_answer(&setup, &mut device);
    // Signer 0 (not the Ledger key) completes the threshold meanwhile.
    let ready = setup.wallet.sign(&setup.intent, 0);
    assert_eq!(ready.state, IntentState::Ready);
    let step = signing.reply(answer).unwrap();
    let refused = drive_device(&signing, &mut device, step).err().unwrap();
    assert_eq!(
        reason(refused),
        "Argent multisig intent is not collecting signatures"
    );
    assert_eq!(setup.stored(), ready);
}

#[test]
fn an_intent_that_went_ready_is_not_sent_to_the_device() {
    let setup = setup(&networks()[0]);
    // Ready before the first APDU: nothing reaches the device.
    let signing = setup.on_device();
    setup.wallet.sign(&setup.intent, 0);
    assert!(signing.start().is_err());

    // Ready while parked: the retry refuses before the device is asked again.
    let setup = ledger_cosign_support::setup(&networks()[0]);
    let signing = setup.on_device();
    let mut device = setup.device();
    device.reject = 1;
    let step = drive_device(&signing, &mut device, signing.start().unwrap()).unwrap();
    assert!(matches!(step, MultisigDeviceStep::DeviceRetry { .. }));
    setup.wallet.sign(&setup.intent, 0);
    let apdus = device.apdus;
    assert_eq!(
        reason(signing.retry().err().unwrap()),
        "Argent multisig intent is not collecting signatures"
    );
    assert_eq!(device.apdus, apdus);
    assert!(signing.retry().is_err());
}

#[test]
fn an_intent_removed_mid_confirmation_is_not_stored() {
    let setup = setup(&networks()[0]);
    let mut device = setup.device();
    let (signing, answer) = first_answer(&setup, &mut device);
    setup
        .wallet
        .core
        .remove_multisig(setup.wallet.multisig_id())
        .unwrap();
    let step = signing.reply(answer).unwrap();
    assert_eq!(
        reason(drive_device(&signing, &mut device, step).err().unwrap()),
        "intent not found"
    );
    assert!(setup
        .wallet
        .core
        .multisig_intents(setup.wallet.multisig_id())
        .unwrap()
        .is_empty());
}

/// Rewrites the stored intent file with `edit` applied to the intent.
fn tamper(setup: &Setup, edit: impl Fn(&mut Value)) {
    let path = setup.wallet.dir.path().join("multisig-intents.json");
    let mut file: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    edit(&mut file["intents"][0]);
    std::fs::write(&path, file.to_string()).unwrap();
}

#[test]
fn a_tampered_store_is_refused_before_the_device() {
    for edit in [
        // Another hash for the same prepared fields.
        |intent: &mut Value| intent["prepared"]["transactionHash"] = "0x1234".into(),
        // Other prepared fields under the same hash.
        |intent: &mut Value| intent["prepared"]["nonce"] = "0x99".into(),
    ] {
        let setup = setup(&networks()[0]);
        let state = setup.wallet.current_state(&setup.intent);
        tamper(&setup, edit);
        let refused = setup
            .wallet
            .core
            .clone()
            .sign_multisig_intent_on_device(setup.intent.id.clone(), setup.ledger_id.clone(), state)
            .err()
            .unwrap();
        assert!(matches!(refused, CoreError::Storage { .. }), "{refused:?}");
    }
}

#[test]
fn an_idle_lock_during_signing_stops_the_device() {
    let setup = setup(&networks()[0]);
    let mut device = setup.device();
    let (signing, answer) = first_answer(&setup, &mut device);
    setup.wallet.clock.advance(IDLE_LOCK_MS);
    assert!(matches!(signing.reply(answer), Err(CoreError::Locked)));
    assert_eq!(device.apdus, 1);
    assert!(signing.retry().is_err());
}

#[test]
fn one_signer_signs_one_intent_once_at_a_time() {
    let setup = setup(&networks()[0]);
    let state = || setup.wallet.current_state(&setup.intent);
    let first = setup.on_device();
    // A second device signing by the same key is refused before any APDU.
    let second = setup.wallet.core.clone().sign_multisig_intent_on_device(
        setup.intent.id.clone(),
        setup.ledger_id.clone(),
        state(),
    );
    assert_eq!(reason(second.err().unwrap()), CLAIMED);
    // So is the same key's software record.
    let software = setup.wallet.core.sign_multisig_intent(
        setup.intent.id.clone(),
        setup.software_id.clone(),
        state(),
    );
    assert_eq!(reason(software.err().unwrap()), CLAIMED);
    // Another signer is not held up.
    assert_eq!(
        setup.wallet.sign(&setup.intent, 0).state,
        IntentState::Ready
    );

    drop(first);

    // Released when dropped, and when a signing ends.
    let setup = ledger_cosign_support::setup(&networks()[0]);
    drop(setup.on_device());
    let ended = setup.on_device();
    let mut tampering = setup.device();
    tampering.tamper = true;
    assert!(drive_device(&ended, &mut tampering, ended.start().unwrap()).is_err());
    let again = setup.on_device();
    let mut device = setup.device();
    let step = drive_device(&again, &mut device, again.start().unwrap()).unwrap();
    assert!(
        matches!(step, MultisigDeviceStep::Signed { .. }),
        "{step:?}"
    );
    // Signed: released, and the intent now refuses this key as already signed.
    drop(again);
    assert!(setup
        .wallet
        .core
        .clone()
        .sign_multisig_intent_on_device(
            setup.intent.id.clone(),
            setup.ledger_id.clone(),
            setup.wallet.current_state(&setup.intent),
        )
        .is_err());
}

#[test]
fn a_lock_releases_the_claim() {
    let setup = setup(&networks()[0]);
    let held = setup.on_device();
    setup.wallet.relock();
    let signing = setup.on_device();
    let mut device = setup.device();
    let step = drive_device(&signing, &mut device, signing.start().unwrap()).unwrap();
    assert!(
        matches!(step, MultisigDeviceStep::Signed { .. }),
        "{step:?}"
    );
    assert!(matches!(held.start(), Err(CoreError::Locked)));
}
