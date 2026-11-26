//! The dashboard's APDUs and replies (`device-transition.ts`), and the device model and firmware
//! gate (`client.ts`); the replies are `@bramble/testkit/ledger-management`'s and the cases those
//! of `device-transition.test.ts` and `client.test.ts`.

use super::rejection::RejectionCode as C;
use super::starknet_tests::hex;
use super::*;

/// `ledgerAppStateResponse(name, version, trailingFields)`.
pub(super) fn app_state(name: &[u8], version: &str, trailing: &[&[u8]]) -> Vec<u8> {
    let mut bytes = vec![1, name.len() as u8];
    bytes.extend_from_slice(name);
    bytes.push(version.len() as u8);
    bytes.extend_from_slice(version.as_bytes());
    bytes.push(0);
    for field in trailing {
        bytes.push(field.len() as u8);
        bytes.extend_from_slice(field);
    }
    bytes.extend([0x90, 0x00]);
    bytes
}

/// `ledgerNanoXOsResponse()`: Ledger's Nano X `GetOsVersion` fixture.
pub(super) fn nano_x_os() -> Vec<u8> {
    let mut bytes = vec![0x33, 0x00, 0x00, 0x04, 0x05];
    bytes.extend_from_slice(b"2.2.3");
    bytes.extend([0x04, 0xe6, 0x00, 0x00, 0x00, 0x04]);
    bytes.extend_from_slice(b"2.30");
    bytes.push(0x04);
    bytes.extend_from_slice(b"1.16");
    bytes.extend([0x01, 0x00, 0x01, 0x00, 0x01, 0x00, 0x90, 0x00]);
    bytes
}

/// An OS reply with the Nano X's target id, OS `2.6.1` and flags, then `rest`.
pub(super) fn os_with(rest: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0x33, 0x00, 0x00, 0x04, 0x05];
    bytes.extend_from_slice(b"2.6.1");
    bytes.extend([0x04, 0xe6, 0x00, 0x00, 0x00]);
    bytes.extend_from_slice(rest);
    bytes.extend([0x90, 0x00]);
    bytes
}

fn unreadable<T: std::fmt::Debug>(result: Result<T, LedgerRejection>) {
    assert_eq!(
        result.unwrap_err(),
        LedgerRejection {
            code: C::ManagementUnreadable,
            status_word: None
        }
    );
}

fn version(major: u16, minor: u16, patch: u16) -> Version {
    Version {
        major,
        minor,
        patch,
    }
}

#[test]
fn requests_are_the_management_bytes() {
    assert_eq!(app_state_request(), hex("b001000000"));
    assert_eq!(os_version_request(), hex("e001000000"));
    assert_eq!(quit_app_request(), hex("b0a7000000"));
    assert_eq!(open_starknet_request(), hex("e0d8000008537461726b6e6574"));
    assert_eq!(STARKNET_APP_NAME.as_bytes(), b"Starknet");
}

#[test]
fn app_state_reads_name_and_version() {
    let state = parse_app_state(Ok(&app_state(b"Starknet", "2.4.0", &[]))).unwrap();
    assert_eq!(state.name, STARKNET_APP_NAME);
    assert_eq!(state.version, "2.4.0");
    assert!(!state.is_dashboard());
    for name in [&b"BOLOS"[..], b"OLOS", b"OLOS\0"] {
        let state = parse_app_state(Ok(&app_state(name, "2.4.0", &[]))).unwrap();
        assert!(state.is_dashboard());
    }
    // Fields newer firmware appends are tolerated: flags and four more.
    let extras = app_state(b"BOLOS", "2.4.0", &[&[1], &[2, 3], &[]]);
    assert!(parse_app_state(Ok(&extras)).unwrap().is_dashboard());
    let four_more = app_state(b"Starknet", "2.4.0", &[&[1u8][..]; 4]);
    assert!(parse_app_state(Ok(&four_more)).is_ok());
    // `TextDecoder` drops a byte-order mark.
    let marked = app_state("\u{feff}BOLOS".as_bytes(), "2.4.0", &[]);
    assert!(parse_app_state(Ok(&marked)).unwrap().is_dashboard());
}

#[test]
fn app_state_refuses_what_it_cannot_read() {
    // Flags plus five more fields is one too many.
    unreadable(parse_app_state(Ok(&app_state(
        b"Starknet",
        "2.4.0",
        &[&[1u8][..]; 5],
    ))));
    let mut format = app_state(b"Starknet", "2.4.0", &[]);
    format[0] = 2;
    unreadable(parse_app_state(Ok(&format)));
    unreadable(parse_app_state(Ok(&app_state(&[b'a'; 65], "2.4.0", &[]))));
    assert!(parse_app_state(Ok(&app_state(&[b'a'; 64], "2.4.0", &[]))).is_ok());
    let long_version = "1".repeat(33);
    unreadable(parse_app_state(Ok(&app_state(
        b"Starknet",
        &long_version,
        &[],
    ))));
    unreadable(parse_app_state(Ok(&app_state(&[0xff], "2.4.0", &[]))));
    // A field running past the status word, and replies with no fields at all.
    unreadable(parse_app_state(Ok(&[1, 9, b'a', 0x90, 0x00])));
    unreadable(parse_app_state(Ok(&[1, 0x90, 0x00])));
    unreadable(parse_app_state(Ok(&[0x90, 0x00])));
    unreadable(parse_app_state(Ok(&[0x90])));
    // Over 260 bytes in all, though every field is in bounds.
    let long = app_state(b"Starknet", "2.4.0", &[&[0u8; 64][..]; 4]);
    assert!(long.len() > 260);
    unreadable(parse_app_state(Ok(&long)));
}

#[test]
fn a_refused_command_names_the_refusal() {
    for read in [
        |reply: Reply<'_>| parse_app_state(reply).map(|_| ()),
        |reply: Reply<'_>| parse_os_version(reply).map(|_| ()),
    ] {
        assert_eq!(
            read(Ok(&[0x69, 0x85])).unwrap_err(),
            LedgerRejection {
                code: C::ManagementRefused,
                status_word: Some(0x6985)
            }
        );
        assert_eq!(
            read(Err(TransportFailure::Locked)).unwrap_err().code,
            C::DeviceLocked
        );
        assert_eq!(
            read(Err(TransportFailure::Other)).unwrap_err().code,
            C::UnexpectedDeviceError
        );
    }
}

#[test]
fn os_version_reads_ledger_envelope() {
    assert_eq!(
        parse_os_version(Ok(&nano_x_os())),
        Ok(OsVersion {
            target_id: 0x3300_0004,
            version: "2.2.3".into()
        })
    );
    // The MCU version alone, and with five optional fields after it.
    assert!(parse_os_version(Ok(&os_with(&[1, b'1']))).is_ok());
    assert!(parse_os_version(Ok(&os_with(&[1, b'1', 0, 0, 0, 0, 0]))).is_ok());
}

#[test]
fn os_version_refuses_what_it_cannot_read() {
    // `device-transition.test.ts`: a truncated, an oversized and a non-UTF-8 MCU version.
    unreadable(parse_os_version(Ok(&os_with(&[0x04, 0x32, 0x2e]))));
    let oversized = [&[33][..], &[0x31; 33]].concat();
    unreadable(parse_os_version(Ok(&os_with(&oversized))));
    unreadable(parse_os_version(Ok(&os_with(&[0x01, 0xff]))));
    // An empty or unprintable MCU version, and six optional fields.
    unreadable(parse_os_version(Ok(&os_with(&[0]))));
    unreadable(parse_os_version(Ok(&os_with(&[1, 0x07]))));
    unreadable(parse_os_version(Ok(&os_with(&[1, b'1', 0, 0, 0, 0, 0, 0]))));
    // An empty OS version, and a reply shorter than a target id.
    unreadable(parse_os_version(Ok(&[
        0x33, 0, 0, 4, 0, 0, 1, b'1', 0x90, 0x00,
    ])));
    unreadable(parse_os_version(Ok(&[0x33, 0, 0, 0x90, 0x00])));
}

#[test]
fn firmware_names_the_model_from_ledger_masks() {
    let os = |target_id, version: &str| OsVersion {
        target_id,
        version: version.into(),
    };
    // `client.test.ts`: every supported model, at any hardware revision.
    for (target_id, model) in [
        (0x3300_1234, DeviceModel::NanoX),
        (0x3310_abcd, DeviceModel::NanoSPlus),
        (0x3320_0042, DeviceModel::Stax),
        (0x3330_ffff, DeviceModel::Flex),
        (0x3340_0099, DeviceModel::NanoGen5),
    ] {
        assert_eq!(
            firmware(&os(target_id, "1.5.1")),
            Ok(Firmware {
                model,
                version: version(1, 5, 1)
            })
        );
    }
    // An unknown model is unsupported before its version is read.
    for target_id in [0x3110_0004, 0x3100_0004, 0x3350_0000, 0xb300_0004] {
        assert_eq!(
            firmware(&os(target_id, "x")).unwrap_err().code,
            C::UnsupportedLedgerDevice
        );
    }
    assert_eq!(
        firmware(&os(0x3300_0004, "65535.0.0")).unwrap().version,
        version(65535, 0, 0)
    );
    for bad in [
        "2.7",
        "2.7.1.0",
        "02.7.1",
        "2.7.-1",
        "65536.0.0",
        "2.7.1 ",
        "",
        "a.b.c",
    ] {
        assert_eq!(
            firmware(&os(0x3300_0004, bad)).unwrap_err(),
            LedgerRejection {
                code: C::UnexpectedDeviceError,
                status_word: None
            }
        );
    }
}

#[test]
fn firmware_gate() {
    let minimums = [(DeviceModel::NanoX, version(2, 6, 1))];
    let nano_x = |v| Firmware {
        model: DeviceModel::NanoX,
        version: v,
    };
    for ok in [version(2, 6, 1), version(2, 6, 2), version(3, 0, 0)] {
        assert_eq!(check_firmware(&nano_x(ok), &minimums), Ok(()));
    }
    for below in [version(2, 6, 0), version(2, 5, 9), version(1, 9, 9)] {
        assert_eq!(
            check_firmware(&nano_x(below), &minimums).unwrap_err().code,
            C::FirmwareBelowMinimum
        );
    }
    let stax = Firmware {
        model: DeviceModel::Stax,
        version: version(9, 9, 9),
    };
    assert_eq!(
        check_firmware(&stax, &minimums).unwrap_err().code,
        C::UnsupportedLedgerDevice
    );
}
