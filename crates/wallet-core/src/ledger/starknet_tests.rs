//! The Starknet app's APDUs byte for byte (`hw-app-starknet` 3.0.2), and its replies read as
//! `client.ts` reads them; the cases of `ledger-protocol/tests/client.test.ts` and
//! `errors.test.ts` among them.

use super::rejection::RejectionCode as C;
use super::*;
use crate::Felt;

const H: u32 = 0x8000_0000;
/// `m/2645'/1195502025'/734980059'/0'/5'/0` (Bramble's `LedgerW`, index 5).
const LEDGERW_5: &str = "80000a55c741e9c9abcee7db800000008000000500000000";
/// `m/2645'/1195502025'/1148870696'/0'/0'/12` (Ready's `argentx`, index 12).
const ARGENTX_12: &str = "80000a55c741e9c9c47a602880000000800000000000000c";

pub(super) fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap())
        .collect()
}

fn components(children: [u32; 6]) -> [PathComponent; 6] {
    children.map(|c| PathComponent {
        index: c & !H,
        hardened: c & H != 0,
    })
}

fn path(children: [u32; 6]) -> LedgerPath {
    LedgerPath::new(components(children)).unwrap()
}

fn ledgerw(index: u32) -> LedgerPath {
    path([
        2645 | H,
        1_195_502_025 | H,
        734_980_059 | H,
        H,
        index | H,
        0,
    ])
}

fn argentx(index: u32) -> LedgerPath {
    path([2645 | H, 1_195_502_025 | H, 1_148_870_696 | H, H, H, index])
}

/// A reply: data, then the status word.
fn reply(data: &[u8], status: u16) -> Vec<u8> {
    [data, &status.to_be_bytes()].concat()
}

fn rejected(code: C, status_word: Option<u16>) -> LedgerRejection {
    LedgerRejection { code, status_word }
}

fn felt32(last: u8) -> Vec<u8> {
    let mut bytes = vec![0; 32];
    bytes[31] = last;
    bytes
}

#[test]
fn paths_encode_as_the_library_serialises_them() {
    assert_eq!(ledgerw(5).to_bytes().to_vec(), hex(LEDGERW_5));
    assert_eq!(argentx(12).to_bytes().to_vec(), hex(ARGENTX_12));
    // The text is the extension's stored spelling (`ledger-path.test.ts`).
    assert_eq!(
        ledgerw(5).to_string(),
        "m/2645'/1195502025'/734980059'/0'/5'/0"
    );
    assert_eq!(
        argentx(12).to_string(),
        "m/2645'/1195502025'/1148870696'/0'/0'/12"
    );
    assert_eq!(
        ledgerw(H - 1).to_string(),
        format!("m/2645'/1195502025'/734980059'/0'/{}'/0", H - 1)
    );
}

#[test]
fn paths_refuse_what_the_library_throws_on() {
    let ok = components([2645 | H, 1_195_502_025 | H, 734_980_059 | H, H, H, 0]);
    let with = |at: usize, component: PathComponent| {
        let mut components = ok;
        components[at] = component;
        LedgerPath::new(components)
    };
    assert!(with(4, PathComponent::hardened(H - 1)).is_some());
    assert!(with(4, PathComponent::hardened(H)).is_none());
    assert!(with(5, PathComponent::plain(H)).is_none());
    assert!(with(0, PathComponent::hardened(44)).is_none());
    assert!(with(0, PathComponent::plain(2645)).is_none());
}

#[test]
fn requests_are_the_library_bytes() {
    assert_eq!(app_version_request(), hex("5a00000000"));
    // Silent: P1 0.
    assert_eq!(
        public_key_request(&ledgerw(5)),
        hex(&format!("5a01000018{LEDGERW_5}"))
    );
    let [path_apdu, hash_apdu] = sign_hash_requests(&argentx(12), &Felt::ONE);
    assert_eq!(path_apdu, hex(&format!("5a02000018{ARGENTX_12}")));
    assert_eq!(hash_apdu, hex(&format!("5a02010020{}01", "00".repeat(31))));
    // The largest felt fills 32 bytes, left-padded as `signHash` pads 63 digits.
    let [_, hash_apdu] = sign_hash_requests(&ledgerw(0), &Felt::MAX);
    assert_eq!(
        hash_apdu,
        hex(&format!("5a020100200800000000000011{}", "00".repeat(24)))
    );
}

#[test]
fn app_version_reads_three_bytes() {
    let version = |major, minor, patch| Version {
        major,
        minor,
        patch,
    };
    assert_eq!(
        parse_app_version(Ok(&reply(&[2, 4, 0], 0x9000))),
        Ok(version(2, 4, 0))
    );
    assert_eq!(
        parse_app_version(Ok(&reply(&[255, 1, 0, 7], 0x9000))),
        Ok(version(255, 1, 0))
    );
    // Too few bytes for a version (`isVersionPart` of `undefined`).
    let unexpected = Err(rejected(C::UnexpectedDeviceError, None));
    assert_eq!(parse_app_version(Ok(&reply(&[2, 4], 0x9000))), unexpected);
    assert_eq!(parse_app_version(Ok(&reply(&[], 0x9000))), unexpected);
}

#[test]
fn app_version_maps_statuses_and_throws() {
    let read = |status: u16| parse_app_version(Ok(&reply(&[0, 0, 0], status))).unwrap_err();
    // Passed through by the library, then `classifyDeviceStatus`.
    for status in [0x6e00, 0x6e01] {
        assert_eq!(read(status), rejected(C::NoAppOpen, Some(status)));
    }
    assert_eq!(read(0x6e04), rejected(C::RejectedOnDevice, Some(0x6e04)));
    assert_eq!(
        read(0x6e02),
        rejected(C::UnexpectedDeviceError, Some(0x6e02))
    );
    // Thrown by the transport, then `mapThrown` over the error's text.
    assert_eq!(read(0x6d00), rejected(C::NoAppOpen, None));
    for status in [0x5515, 0x6982, 0x9840] {
        assert_eq!(read(status), rejected(C::DeviceLocked, None));
    }
    for status in [0x6f00, 0x6985, 0x6a80, 0x5501, 0x0000, 0xffff] {
        assert_eq!(read(status), rejected(C::UnexpectedDeviceError, None));
    }
    for short in [&[][..], &[0x90]] {
        assert_eq!(
            parse_app_version(Ok(short)),
            Err(rejected(C::UnexpectedDeviceError, None))
        );
    }
    let failed = |failure| parse_app_version(Err(failure)).unwrap_err();
    assert_eq!(
        failed(TransportFailure::Locked),
        rejected(C::DeviceLocked, None)
    );
    assert_eq!(
        failed(TransportFailure::Disconnected),
        rejected(C::DisconnectedMidSign, None)
    );
    assert_eq!(
        failed(TransportFailure::Busy),
        rejected(C::DeviceBusy, None)
    );
    assert_eq!(
        failed(TransportFailure::Other),
        rejected(C::UnexpectedDeviceError, None)
    );
}

#[test]
fn app_version_gate() {
    let v = |major, minor, patch| Version {
        major,
        minor,
        patch,
    };
    let minimum = v(2, 4, 0);
    for ok in [v(2, 4, 0), v(2, 4, 1), v(2, 5, 0), v(3, 0, 0)] {
        assert_eq!(check_app_version(ok, minimum), Ok(()));
    }
    for below in [v(2, 3, 9), v(2, 3, 255), v(1, 9, 9), v(0, 0, 0)] {
        assert_eq!(
            check_app_version(below, minimum),
            Err(rejected(C::AppBelowMinimum, None))
        );
    }
}

#[test]
fn public_key_keeps_x() {
    // A format byte, X = 0x…02, Y all 0xff (`client.test.ts`).
    let key = [vec![0x04], felt32(2), vec![0xff; 32]].concat();
    assert_eq!(parse_public_key(Ok(&reply(&key, 0x9000))), Ok(Felt::TWO));
    // The library returns `data[1..65]`: a byte past it is not read.
    let longer = [key.clone(), vec![0]].concat();
    assert_eq!(parse_public_key(Ok(&reply(&longer, 0x9000))), Ok(Felt::TWO));
    let unexpected = Err(rejected(C::UnexpectedDeviceError, None));
    // A key a byte short, and an X at or above the prime.
    assert_eq!(parse_public_key(Ok(&reply(&key[..64], 0x9000))), unexpected);
    let big = [vec![0x04], vec![0xff; 64]].concat();
    assert_eq!(parse_public_key(Ok(&reply(&big, 0x9000))), unexpected);
}

#[test]
fn public_key_maps_statuses_and_throws() {
    let read = |status: u16| parse_public_key(Ok(&reply(&[], status))).unwrap_err();
    assert_eq!(read(0x6e04), rejected(C::RejectedOnDevice, Some(0x6e04)));
    assert_eq!(read(0x6e00), rejected(C::NoAppOpen, Some(0x6e00)));
    assert_eq!(
        read(0x6e02),
        rejected(C::UnexpectedDeviceError, Some(0x6e02))
    );
    // Everything the library swallows becomes `ExecutionError`.
    let execution = rejected(C::UnexpectedDeviceError, Some(0x6f00));
    for status in [0x6985, 0x6d00, 0x5515] {
        assert_eq!(read(status), execution);
    }
    assert_eq!(parse_public_key(Ok(&[0x90])).unwrap_err(), execution);
    assert_eq!(
        parse_public_key(Err(TransportFailure::Locked)).unwrap_err(),
        execution
    );
}

#[test]
fn signature_reads_r_and_s() {
    let data = [vec![0x41], felt32(0x0a), felt32(0x0b), vec![0]].concat();
    let signature = DeviceSignature {
        r: Felt::from(10u8),
        s: Felt::from(11u8),
    };
    assert_eq!(parse_sign_path(Ok(&reply(&[], 0x9000))), Ok(()));
    assert_eq!(parse_signature(Ok(&reply(&data, 0x9000))), Ok(signature));
    // The recovery byte is never read.
    assert_eq!(
        parse_signature(Ok(&reply(&data[..65], 0x9000))),
        Ok(signature)
    );
    let unexpected = Err(rejected(C::UnexpectedDeviceError, None));
    // `client.test.ts`: a one-byte r.
    assert_eq!(parse_signature(Ok(&reply(&[0x41, 1], 0x9000))), unexpected);
    assert_eq!(parse_signature(Ok(&reply(&data[..64], 0x9000))), unexpected);
    // r, then s, at or above the prime.
    let high_r = [vec![0x41], vec![0xff; 32], felt32(0x0b)].concat();
    let high_s = [vec![0x41], felt32(0x0a), vec![0xff; 32]].concat();
    assert_eq!(parse_signature(Ok(&reply(&high_r, 0x9000))), unexpected);
    assert_eq!(parse_signature(Ok(&reply(&high_s, 0x9000))), unexpected);
}

#[test]
fn signing_maps_statuses_and_throws() {
    let blind = rejected(C::RejectedOrBlindSigningOff, Some(0x6f00));
    for read in [
        |reply: Reply<'_>| parse_sign_path(reply).map(|_| ()),
        |reply: Reply<'_>| parse_signature(reply).map(|_| ()),
    ] {
        let status = |status: u16| read(Ok(&reply(&[], status))).unwrap_err();
        assert_eq!(status(0x6e04), rejected(C::RejectedOnDevice, Some(0x6e04)));
        assert_eq!(
            status(0x6e01),
            rejected(C::UnexpectedDeviceError, Some(0x6e01))
        );
        assert_eq!(
            status(0x6e00),
            rejected(C::UnexpectedDeviceError, Some(0x6e00))
        );
        // The app's 0x6985 (refused, or Blind signing off) is not on the library's list.
        assert_eq!(status(0x6985), blind);
        assert_eq!(read(Ok(&[])).unwrap_err(), blind);
        for failure in [
            TransportFailure::Locked,
            TransportFailure::Disconnected,
            TransportFailure::Busy,
            TransportFailure::Other,
        ] {
            assert_eq!(read(Err(failure)).unwrap_err(), blind);
        }
    }
}

#[test]
fn rejection_codes_are_the_extension_catalogue() {
    // `LedgerRejectionCode` in `errors.ts`, in its order.
    let codes = [
        (C::WebhidUnavailable, "webhid-unavailable"),
        (C::PickerCancelled, "picker-cancelled"),
        (C::DeviceLocked, "device-locked"),
        (C::NoAppOpen, "no-app-open"),
        (C::FirmwareBelowMinimum, "firmware-below-minimum"),
        (C::AppBelowMinimum, "app-below-minimum"),
        (C::UnsupportedLedgerDevice, "unsupported-ledger-device"),
        (
            C::RejectedOrBlindSigningOff,
            "rejected-or-blind-signing-off",
        ),
        (C::DeviceBusy, "device-busy"),
        (C::Replug, "replug"),
        (C::MultipleDevices, "multiple-devices"),
        (C::WrongDevice, "wrong-device"),
        (C::NetworkMismatch, "network-mismatch"),
        (C::DeployUnfunded, "deploy-unfunded"),
        (C::RejectedOnDevice, "rejected-on-device"),
        (C::LockedMidSign, "locked-mid-sign"),
        (C::DisconnectedMidSign, "disconnected-mid-sign"),
        (C::PanelContextLost, "panel-context-lost"),
        (C::SignTimeout, "sign-timeout"),
        (C::SignatureVerifyFailed, "signature-verify-failed"),
        (C::WorkerEvicted, "worker-evicted"),
        (C::ChainFailed, "chain-failed"),
        (C::ManagementRefused, "management-refused"),
        (C::ManagementUnreadable, "management-unreadable"),
        (C::DashboardRestoreFailed, "dashboard-restore-failed"),
        (C::StarknetReopenFailed, "starknet-reopen-failed"),
        (C::UnexpectedDeviceError, "unexpected-device-error"),
    ];
    for (code, name) in codes {
        assert_eq!(code.as_str(), name);
    }
    // `errors.test.ts`: a rejection carries its code and status word; it prints only the code.
    let rejection = rejected(C::AppBelowMinimum, Some(0x6d00));
    assert_eq!(rejection.to_string(), "app-below-minimum");
}
