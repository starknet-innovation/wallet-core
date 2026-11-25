//! `KeyEnumeration` against a scripted device: the APDUs in order, and every way a page ends.

use super::rejection::RejectionCode as C;
use super::*;
use crate::Felt;

const MIN: Version = Version {
    major: 2,
    minor: 4,
    patch: 0,
};
const VERSION_2_4_0: &[u8] = &[2, 4, 0, 0x90, 0x00];

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

/// `getPubKey`'s reply for x = `last` (a format byte, X, an arbitrary Y, 0x9000).
fn key_reply(last: u8) -> Vec<u8> {
    let mut x = [0u8; 32];
    x[31] = last;
    [&[0x04][..], &x, &[0x33; 32], &[0x90, 0x00]].concat()
}

fn sent(step: &EnumerationStep) -> &[u8] {
    match step {
        EnumerationStep::Send(bytes) => bytes,
        other => panic!("expected an APDU, got {other:?}"),
    }
}

fn refused(step: EnumerationStep) -> C {
    match step {
        EnumerationStep::Refused(rejection) => rejection.code,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[test]
fn reads_each_paths_key_in_order() {
    let page = vec![(7, path(7)), (8, path(8))];
    let (mut enumeration, step) = KeyEnumeration::start(page, MIN);
    assert_eq!(sent(&step), app_version_request());
    let step = enumeration.on_reply(Ok(VERSION_2_4_0));
    assert_eq!(sent(&step), public_key_request(&path(7)));
    let step = enumeration.on_reply(Ok(&key_reply(0x17)));
    assert_eq!(sent(&step), public_key_request(&path(8)));
    let step = enumeration.on_reply(Ok(&key_reply(0x18)));
    assert_eq!(
        step,
        EnumerationStep::Keys(vec![
            DeviceKeyAt {
                index: 7,
                public_key_x: Felt::from(0x17_u8),
            },
            DeviceKeyAt {
                index: 8,
                public_key_x: Felt::from(0x18_u8),
            },
        ])
    );
    assert_eq!(
        refused(enumeration.on_reply(Ok(&key_reply(1)))),
        C::UnexpectedDeviceError
    );
}

#[test]
fn a_failure_mid_page_returns_no_key() {
    let page = vec![(0, path(0)), (1, path(1))];
    let (mut enumeration, _) = KeyEnumeration::start(page, MIN);
    enumeration.on_reply(Ok(VERSION_2_4_0));
    enumeration.on_reply(Ok(&key_reply(1)));
    assert_eq!(
        refused(enumeration.on_reply(Err(TransportFailure::Disconnected))),
        C::UnexpectedDeviceError
    );
    assert_eq!(
        refused(enumeration.on_reply(Ok(&key_reply(2)))),
        C::UnexpectedDeviceError
    );
}

#[test]
fn refuses_an_app_below_the_minimum_before_any_key() {
    let (mut enumeration, _) = KeyEnumeration::start(vec![(0, path(0))], MIN);
    assert_eq!(
        refused(enumeration.on_reply(Ok(&[2, 3, 9, 0x90, 0x00]))),
        C::AppBelowMinimum
    );
}

#[test]
fn a_page_holds_one_to_five_paths() {
    let (_, step) = KeyEnumeration::start(Vec::new(), MIN);
    assert_eq!(refused(step), C::UnexpectedDeviceError);
    let six = (0..6).map(|index| (index, path(index))).collect();
    let (_, step) = KeyEnumeration::start(six, MIN);
    assert_eq!(refused(step), C::UnexpectedDeviceError);
    let five = (0..5).map(|index| (index, path(index))).collect();
    let (_, step) = KeyEnumeration::start(five, MIN);
    assert_eq!(sent(&step), app_version_request());
}
