//! `HashSigning`: the rules around one device signature that every Ledger signature shares, over
//! `sign_tests`' scripted device.

use std::cell::Cell;

use super::rejection::RejectionCode as C;
use super::sign_tests::{hash, path, Device, MIN, OK, VERSION_2_4_0};
use super::*;
use crate::Felt;

const KEY: &str = "0x1234567890abcdef";

fn device() -> DeviceKey {
    DeviceKey {
        path: path(0),
        app_minimum: MIN,
    }
}

fn sent(step: &HashStep) -> &[u8] {
    match step {
        HashStep::Send(bytes) => bytes,
        other => panic!("expected an APDU, got {other:?}"),
    }
}

/// Answers every APDU up to the signature as the device would; returns the last step.
fn to_signature(signing: &mut HashSigning, first: HashStep, live: &dyn Fn() -> bool) -> HashStep {
    let held = Device::new(KEY);
    assert_eq!(sent(&first), app_version_request());
    let step = signing.on_reply(Ok(VERSION_2_4_0), live);
    assert_eq!(sent(&step), public_key_request(&path(0)));
    let step = signing.on_reply(Ok(&held.public_key_reply()), live);
    sent(&step);
    let step = signing.on_reply(Ok(OK), live);
    sent(&step);
    signing.on_reply(Ok(&held.signature_reply(&hash())), live)
}

#[test]
fn a_live_session_signs_and_then_ends() {
    let live = || true;
    let key = Device::new(KEY).public_key();
    let (mut signing, first) = HashSigning::start(device(), hash(), key, &live);
    let step = to_signature(&mut signing, first, &live);
    assert!(matches!(step, HashStep::Signed(_)), "{step:?}");
    assert_eq!(signing.hash(), hash());
    // Nothing after the end: no replay of the signature, no retry.
    assert_eq!(
        signing.on_reply(Ok(OK), &live),
        HashStep::Failed(HashFailure::Refused(C::UnexpectedDeviceError))
    );
    assert_eq!(
        signing.retry(&live),
        HashStep::Failed(HashFailure::Refused(C::UnexpectedDeviceError))
    );
}

#[test]
fn a_locked_session_sends_no_apdu() {
    let key = Device::new(KEY).public_key();
    let (_, step) = HashSigning::start(device(), hash(), key, &|| false);
    assert_eq!(step, HashStep::Failed(HashFailure::Locked));
}

#[test]
fn a_lock_mid_exchange_stops_the_next_apdu() {
    let live = Cell::new(true);
    let current = || live.get();
    let key = Device::new(KEY).public_key();
    let (mut signing, first) = HashSigning::start(device(), hash(), key, &current);
    sent(&first);
    live.set(false);
    assert_eq!(
        signing.on_reply(Ok(VERSION_2_4_0), &current),
        HashStep::Failed(HashFailure::Locked)
    );
    live.set(true);
    assert_eq!(
        signing.on_reply(Ok(VERSION_2_4_0), &current),
        HashStep::Failed(HashFailure::Refused(C::UnexpectedDeviceError))
    );
}

#[test]
fn a_signature_after_the_lock_is_dropped() {
    // The session ends while the person confirms: the answer is read, the signature dropped.
    let calls = Cell::new(0);
    let current = || {
        calls.set(calls.get() + 1);
        calls.get() < 6
    };
    let key = Device::new(KEY).public_key();
    let (mut signing, first) = HashSigning::start(device(), hash(), key, &current);
    assert_eq!(
        to_signature(&mut signing, first, &current),
        HashStep::Failed(HashFailure::Locked)
    );
}

#[test]
fn a_wrong_device_parks_and_a_retry_starts_again() {
    let live = || true;
    let other = Device::new("0xabc");
    let key = Device::new(KEY).public_key();
    let (mut signing, first) = HashSigning::start(device(), hash(), key, &live);
    sent(&first);
    sent(&signing.on_reply(Ok(VERSION_2_4_0), &live));
    assert_eq!(
        signing.on_reply(Ok(&other.public_key_reply()), &live),
        HashStep::Retry(C::WrongDevice)
    );
    // Parked: a stray answer is not read; only a retry restarts it.
    let mut parked = HashSigning::start(device(), hash(), key, &live).0;
    sent(&parked.on_reply(Ok(VERSION_2_4_0), &live));
    assert_eq!(
        parked.on_reply(Ok(&other.public_key_reply()), &live),
        HashStep::Retry(C::WrongDevice)
    );
    assert_eq!(
        parked.on_reply(Ok(OK), &live),
        HashStep::Failed(HashFailure::Refused(C::UnexpectedDeviceError))
    );
    let first = signing.retry(&live);
    let step = to_signature(&mut signing, first, &live);
    assert!(matches!(step, HashStep::Signed(_)), "{step:?}");
}

#[test]
fn a_retry_waits_for_the_session() {
    let live = Cell::new(true);
    let current = || live.get();
    let key = Device::new(KEY).public_key();
    let (mut signing, _) = HashSigning::start(device(), hash(), key, &current);
    assert_eq!(
        signing.on_reply(Err(TransportFailure::Disconnected), &current),
        HashStep::Retry(C::DisconnectedMidSign)
    );
    live.set(false);
    assert_eq!(
        signing.retry(&current),
        HashStep::Failed(HashFailure::Locked)
    );
}

#[test]
fn a_signature_that_does_not_verify_ends_it() {
    let live = || true;
    let key = Device::new(KEY).public_key();
    let (mut signing, _) = HashSigning::start(device(), hash(), key, &live);
    sent(&signing.on_reply(Ok(VERSION_2_4_0), &live));
    sent(&signing.on_reply(Ok(&Device::new(KEY).public_key_reply()), &live));
    sent(&signing.on_reply(Ok(OK), &live));
    // The device signs another hash than the one sent.
    let forged = Device::new(KEY).signature_reply(&(hash() + Felt::ONE));
    assert_eq!(
        signing.on_reply(Ok(&forged), &live),
        HashStep::Failed(HashFailure::Refused(C::SignatureVerifyFailed))
    );
    assert_eq!(
        signing.retry(&live),
        HashStep::Failed(HashFailure::Refused(C::UnexpectedDeviceError))
    );
}

#[test]
fn a_zero_hash_is_never_sent() {
    let key = Device::new(KEY).public_key();
    let (_, step) = HashSigning::start(device(), Felt::ZERO, key, &|| true);
    assert_eq!(
        step,
        HashStep::Failed(HashFailure::Refused(C::SignatureVerifyFailed))
    );
}
