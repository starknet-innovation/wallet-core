//! P8: the device OS gate the apps run when a Ledger connects (`LedgerOsGate`), against a scripted
//! device: Starknet open, closed for the dashboard's OS read, then reopened, with Bramble's
//! minimums. The sequence's cases are `wallet_core::ledger`'s `transition_tests.rs`.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

use bramble_core::{LedgerOsGate, LedgerOsStep};
use starknet_wallet_core::ledger::{
    app_state_request, open_starknet_request, os_version_request, quit_app_request,
    TransportFailure,
};

/// `GetAppAndVersion`'s reply naming `name`.
fn app(name: &str) -> Vec<u8> {
    let mut bytes = vec![1, name.len() as u8];
    bytes.extend_from_slice(name.as_bytes());
    bytes.extend([5]);
    bytes.extend_from_slice(b"2.4.3");
    bytes.extend([0, 0x90, 0x00]);
    bytes
}

/// `GetOsVersion`'s reply: a Nano X (`0x33000004`) on `version`.
fn os(version: &str) -> Vec<u8> {
    let mut bytes = vec![0x33, 0x00, 0x00, 0x04, version.len() as u8];
    bytes.extend_from_slice(version.as_bytes());
    bytes.extend([0x04, 0xe6, 0x00, 0x00, 0x00, 0x04]);
    bytes.extend_from_slice(b"2.30");
    bytes.extend([0x90, 0x00]);
    bytes
}

fn sends(step: LedgerOsStep, apdu: Vec<u8>, phase: &str) {
    assert_eq!(
        step,
        LedgerOsStep::Send {
            apdu,
            phase: phase.into()
        }
    );
}

fn reconnect(step: LedgerOsStep, phase: &str) {
    assert_eq!(
        step,
        LedgerOsStep::Reconnect {
            phase: phase.into()
        }
    );
}

/// Runs from Starknet open to the OS read's answer `os_reply`, through the close's drop.
fn to_open(gate: &LedgerOsGate, os_reply: Vec<u8>) -> LedgerOsStep {
    sends(gate.start(), app_state_request(), "checking-device");
    sends(
        gate.reply(app("Starknet")),
        quit_app_request(),
        "returning-to-dashboard",
    );
    reconnect(
        gate.transport_failure(TransportFailure::Disconnected),
        "reconnecting-dashboard",
    );
    sends(
        gate.reconnected(),
        app_state_request(),
        "reconnecting-dashboard",
    );
    sends(
        gate.reply(app("BOLOS")),
        os_version_request(),
        "checking-firmware",
    );
    gate.reply(os_reply)
}

#[test]
fn a_device_in_starknet_is_checked_on_the_dashboard_and_reopened() {
    let gate = LedgerOsGate::new();
    sends(
        to_open(&gate, os("2.6.1")),
        open_starknet_request(),
        "opening-starknet",
    );
    reconnect(
        gate.transport_failure(TransportFailure::Disconnected),
        "reconnecting-starknet",
    );
    sends(
        gate.reconnected(),
        app_state_request(),
        "confirming-starknet",
    );
    assert_eq!(
        gate.reply(app("Starknet")),
        LedgerOsStep::Supported {
            model: "nano-x".into(),
            version: "2.6.1".into()
        }
    );
    // Spent.
    assert!(matches!(gate.reconnected(), LedgerOsStep::Refused { .. }));
}

#[test]
fn an_os_below_bramble_minimum_is_refused_with_starknet_back() {
    let gate = LedgerOsGate::new();
    to_open(&gate, os("2.2.3"));
    gate.reply(vec![0x90, 0x00]);
    gate.reconnected();
    assert_eq!(
        gate.reply(app("Starknet")),
        LedgerOsStep::Refused {
            reason: "firmware-below-minimum".into()
        }
    );
}

#[test]
fn declining_to_reopen_starknet_is_refused() {
    let gate = LedgerOsGate::new();
    to_open(&gate, os("2.6.1"));
    assert_eq!(
        gate.reply(vec![0x55, 0x01]),
        LedgerOsStep::Refused {
            reason: "rejected-on-device".into()
        }
    );
}

#[test]
fn a_locked_device_is_named_locked() {
    let gate = LedgerOsGate::new();
    gate.start();
    assert_eq!(
        gate.reply(vec![0x55, 0x15]),
        LedgerOsStep::Refused {
            reason: "device-locked".into()
        }
    );
}

#[test]
fn a_locked_device_or_a_second_start_ends_it() {
    let gate = LedgerOsGate::new();
    gate.start();
    assert_eq!(
        gate.transport_failure(TransportFailure::Locked),
        LedgerOsStep::Refused {
            reason: "device-locked".into()
        }
    );
    let gate = LedgerOsGate::new();
    gate.start();
    assert_eq!(
        gate.start(),
        LedgerOsStep::Refused {
            reason: "unexpected-device-error".into()
        }
    );
}
