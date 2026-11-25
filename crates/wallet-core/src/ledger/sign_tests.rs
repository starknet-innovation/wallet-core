//! `DeviceSigning` against a scripted device whose signatures Krusty makes, so the verify gate
//! is exercised with real Stark signatures.

use krusty_kms_common::SecretFelt;

use super::rejection::RejectionCode as C;
use super::*;
use crate::{kms, Felt};

const MIN: Version = Version {
    major: 2,
    minor: 4,
    patch: 0,
};

fn path(index: u32) -> LedgerPath {
    let h = PathComponent::hardened;
    LedgerPath::new([
        h(2645),
        h(1_195_502_025),
        h(734_980_059),
        h(0),
        h(index),
        PathComponent::plain(0),
    ])
    .unwrap()
}

fn felt(hex: &str) -> Felt {
    Felt::from_hex(hex).unwrap()
}

/// A device holding one key: its public key and its signature over a hash.
struct Device {
    key: SecretFelt,
}

impl Device {
    fn new(key: &str) -> Self {
        Self {
            key: SecretFelt::new(felt(key)),
        }
    }

    fn public_key(&self) -> Felt {
        felt(&kms::stark_public_key(&self.key).unwrap())
    }

    /// `getPubKey`'s reply: a format byte, X, an arbitrary Y, then 0x9000.
    fn public_key_reply(&self) -> Vec<u8> {
        [
            &[0x04][..],
            &self.public_key().to_bytes_be(),
            &[0x11; 32],
            &[0x90, 0x00],
        ]
        .concat()
    }

    /// `signHash`'s final reply: 0x41, r, s, a recovery byte, then 0x9000.
    fn signature_reply(&self, hash: &Felt) -> Vec<u8> {
        let signature = kms::sign_stark_hash(&self.key, &format!("{hash:#x}")).unwrap();
        let (r, s) = (felt(&signature.r), felt(&signature.s));
        [
            &[0x41][..],
            &r.to_bytes_be(),
            &s.to_bytes_be(),
            &[0x01, 0x90, 0x00],
        ]
        .concat()
    }
}

const OK: &[u8] = &[0x90, 0x00];
const VERSION_2_4_0: &[u8] = &[2, 4, 0, 0x90, 0x00];

fn hash() -> Felt {
    felt("0x2d1a9e9e5e8f2b6a4a2c0b5d7e3f1a9c8b7d6e5f4a3b2c1d0e9f8a7b6c5d4e3")
}

fn sent(step: &DeviceStep) -> &[u8] {
    match step {
        DeviceStep::Send(bytes) => bytes,
        other => panic!("expected an APDU, got {other:?}"),
    }
}

fn refused(step: DeviceStep) -> RejectionCode {
    match step {
        DeviceStep::Refused(rejection) => rejection.code,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn signs_in_the_extensions_order_and_verifies() {
    let device = Device::new("0x1234567890abcdef");
    let (mut signing, step) = DeviceSigning::start(path(5), hash(), device.public_key(), MIN);
    assert_eq!(sent(&step), app_version_request());
    let step = signing.on_reply(Ok(VERSION_2_4_0));
    assert_eq!(sent(&step), public_key_request(&path(5)));
    let step = signing.on_reply(Ok(&device.public_key_reply()));
    let [path_apdu, hash_apdu] = sign_hash_requests(&path(5), &hash());
    assert_eq!(sent(&step), path_apdu);
    let step = signing.on_reply(Ok(OK));
    assert_eq!(sent(&step), hash_apdu);
    let reply = device.signature_reply(&hash());
    let DeviceStep::Signed(signature) = signing.on_reply(Ok(&reply)) else {
        panic!("expected a signature");
    };
    assert!(kms::stark_signature_is_valid(
        &device.public_key(),
        &hash(),
        &signature.r(),
        &signature.s()
    ));
    assert_eq!(signature.r().to_bytes_be()[..], reply[1..33]);
    assert_eq!(signature.s().to_bytes_be()[..], reply[33..65]);
    // Ended: a further reply is refused, never a second signature.
    assert_eq!(
        refused(signing.on_reply(Ok(&reply))),
        C::UnexpectedDeviceError
    );
}

/// Drives a signing to its last APDU with `device` answering, then feeds `last`.
fn sign_with(stored: Felt, device: &Device, last: &[u8]) -> DeviceStep {
    let (mut signing, _) = DeviceSigning::start(path(0), hash(), stored, MIN);
    signing.on_reply(Ok(VERSION_2_4_0));
    signing.on_reply(Ok(&device.public_key_reply()));
    signing.on_reply(Ok(OK));
    signing.on_reply(Ok(last))
}

#[test]
fn refuses_a_signature_that_does_not_verify() {
    let device = Device::new("0x1234567890abcdef");
    // Over another hash.
    let other = device.signature_reply(&felt("0x1234"));
    assert_eq!(
        refused(sign_with(device.public_key(), &device, &other)),
        C::SignatureVerifyFailed
    );
    // `s` altered.
    let mut altered = device.signature_reply(&hash());
    altered[64] ^= 1;
    assert_eq!(
        refused(sign_with(device.public_key(), &device, &altered)),
        C::SignatureVerifyFailed
    );
}

#[test]
fn refuses_another_devices_key_before_the_person_confirms() {
    let device = Device::new("0x1234567890abcdef");
    let stored = Device::new("0x42").public_key();
    let (mut signing, _) = DeviceSigning::start(path(0), hash(), stored, MIN);
    signing.on_reply(Ok(VERSION_2_4_0));
    assert_eq!(
        refused(signing.on_reply(Ok(&device.public_key_reply()))),
        C::WrongDevice
    );
    assert_eq!(refused(signing.on_reply(Ok(OK))), C::UnexpectedDeviceError);
}

#[test]
fn refuses_an_app_below_the_minimum() {
    let device = Device::new("0x1234567890abcdef");
    let (mut signing, _) = DeviceSigning::start(path(0), hash(), device.public_key(), MIN);
    assert_eq!(
        refused(signing.on_reply(Ok(&[2, 3, 9, 0x90, 0x00]))),
        C::AppBelowMinimum
    );
}

#[test]
fn stops_when_the_device_refuses_the_path() {
    let device = Device::new("0x1234567890abcdef");
    let (mut signing, _) = DeviceSigning::start(path(0), hash(), device.public_key(), MIN);
    signing.on_reply(Ok(VERSION_2_4_0));
    signing.on_reply(Ok(&device.public_key_reply()));
    // 0x6986: refused by the person, or blind signing off.
    assert_eq!(
        refused(signing.on_reply(Ok(&[0x69, 0x86]))),
        C::RejectedOrBlindSigningOff
    );
}

#[test]
fn a_transport_failure_ends_the_exchange() {
    let device = Device::new("0x1234567890abcdef");
    let (mut signing, _) = DeviceSigning::start(path(0), hash(), device.public_key(), MIN);
    assert_eq!(
        refused(signing.on_reply(Err(TransportFailure::Locked))),
        C::DeviceLocked
    );
    assert_eq!(
        refused(signing.on_reply(Ok(VERSION_2_4_0))),
        C::UnexpectedDeviceError
    );
}

#[test]
fn refuses_a_zero_hash_before_any_apdu() {
    let device = Device::new("0x1234567890abcdef");
    let (mut signing, step) = DeviceSigning::start(path(0), Felt::ZERO, device.public_key(), MIN);
    assert_eq!(refused(step), C::SignatureVerifyFailed);
    assert_eq!(
        refused(signing.on_reply(Ok(VERSION_2_4_0))),
        C::UnexpectedDeviceError
    );
}
