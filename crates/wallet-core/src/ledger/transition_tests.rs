//! The OS gate's sequence: the cases of the extension's `device-transition.test.ts`
//! (`readLedgerOsAndRestoreStarknet`), then `client.test.ts`'s firmware gate over its result.

use super::management_tests::{app_state, nano_x_os, os_with};
use super::rejection::RejectionCode as C;
use super::*;

/// What the scripted host answers each step with.
enum Answer {
    Reply(Vec<u8>),
    Fail(TransportFailure),
    Reconnected,
}
use Answer::*;

const OK: [u8; 2] = [0x90, 0x00];

fn version(major: u16, minor: u16, patch: u16) -> Version {
    Version {
        major,
        minor,
        patch,
    }
}

/// Bramble's minimums (`bramble-core`'s `LEDGER_OS_MIN`).
const MINIMUMS: [(DeviceModel, Version); 2] = [
    (
        DeviceModel::NanoX,
        Version {
            major: 2,
            minor: 6,
            patch: 1,
        },
    ),
    (
        DeviceModel::Stax,
        Version {
            major: 1,
            minor: 9,
            patch: 1,
        },
    ),
];

/// A Nano X on OS 2.6.1, at Bramble's minimum.
fn current_os() -> Vec<u8> {
    os_with(b"\x041.16")
}

fn app(name: &str) -> Answer {
    Reply(app_state(name.as_bytes(), "2.4.3", &[]))
}

/// The run: each APDU's (CLA, INS), the phases shown, the reconnects, and the end.
struct Run {
    sent: Vec<(u8, u8)>,
    phases: Vec<&'static str>,
    reconnects: usize,
    end: TransitionStep,
}

fn run(answers: Vec<Answer>) -> Run {
    let (mut gate, mut step) = OsGate::start(&MINIMUMS);
    let mut run = Run {
        sent: Vec::new(),
        phases: Vec::new(),
        reconnects: 0,
        end: TransitionStep::Refused(LedgerRejection::new(C::UnexpectedDeviceError)),
    };
    let mut answers = answers.into_iter();
    loop {
        match &step {
            TransitionStep::Send(apdu, phase) => {
                run.sent.push((apdu[0], apdu[1]));
                run.phases.push(phase.as_str());
            }
            TransitionStep::Reconnect(phase) => run.phases.push(phase.as_str()),
            _ => {
                assert!(answers.next().is_none(), "answers left over");
                run.end = step;
                run.phases.dedup();
                return run;
            }
        }
        step = match (answers.next().expect("an answer"), &step) {
            (Reply(bytes), TransitionStep::Send(..)) => gate.on_reply(Ok(&bytes)),
            (Fail(failure), _) => gate.on_reply(Err(failure)),
            (Reconnected, TransitionStep::Reconnect(_)) => {
                run.reconnects += 1;
                gate.reconnected()
            }
            _ => panic!("answer does not fit {step:?}"),
        };
    }
}

fn refused(code: C) -> TransitionStep {
    TransitionStep::Refused(LedgerRejection::new(code))
}

fn supported() -> TransitionStep {
    TransitionStep::Supported(Firmware {
        model: DeviceModel::NanoX,
        version: version(2, 6, 1),
    })
}

#[test]
fn dashboard_starts_skip_close_app_and_confirm_starknet_after_reopening() {
    let run = run(vec![
        app("BOLOS"),
        Reply(current_os()),
        Reply(OK.to_vec()),
        Reconnected,
        app("Starknet"),
    ]);
    assert_eq!(run.end, supported());
    assert_eq!(run.reconnects, 1);
    assert_eq!(
        run.sent,
        [(0xb0, 0x01), (0xe0, 0x01), (0xe0, 0xd8), (0xb0, 0x01)]
    );
    assert_eq!(
        run.phases,
        [
            "checking-device",
            "checking-firmware",
            "opening-starknet",
            "reconnecting-starknet",
            "confirming-starknet"
        ]
    );
}

#[test]
fn app_starts_reconnect_after_both_disconnecting_transitions() {
    let run = run(vec![
        app("Starknet"),
        Fail(TransportFailure::Disconnected),
        Reconnected,
        app("BOLOS"),
        Reply(current_os()),
        Fail(TransportFailure::Disconnected),
        Reconnected,
        app("Starknet"),
    ]);
    assert_eq!(run.end, supported());
    assert_eq!(run.reconnects, 2);
    assert_eq!(
        run.sent,
        [
            (0xb0, 0x01),
            (0xb0, 0xa7),
            (0xb0, 0x01),
            (0xe0, 0x01),
            (0xe0, 0xd8),
            (0xb0, 0x01)
        ]
    );
    assert_eq!(
        run.phases,
        [
            "checking-device",
            "returning-to-dashboard",
            "reconnecting-dashboard",
            "checking-firmware",
            "opening-starknet",
            "reconnecting-starknet",
            "confirming-starknet"
        ]
    );
}

#[test]
fn an_acknowledged_close_app_still_requires_a_dashboard_reconnect() {
    let run = run(vec![
        app("Starknet"),
        Reply(OK.to_vec()),
        Reconnected,
        app("BOLOS"),
        Reply(current_os()),
        Reply(OK.to_vec()),
        Reconnected,
        app("Starknet"),
    ]);
    assert_eq!(run.end, supported());
    assert_eq!(run.reconnects, 2);
}

#[test]
fn a_dashboard_that_does_not_come_back_is_refused() {
    let run = run(vec![
        app("Starknet"),
        Reply(OK.to_vec()),
        Reconnected,
        app("Ethereum"),
    ]);
    assert_eq!(run.end, refused(C::DashboardRestoreFailed));
}

#[test]
fn open_app_disconnect_is_accepted_only_after_starknet_is_confirmed() {
    let run = run(vec![
        app("BOLOS"),
        Reply(current_os()),
        Fail(TransportFailure::Disconnected),
        Reconnected,
        app("Ethereum"),
    ]);
    assert_eq!(run.end, refused(C::StarknetReopenFailed));
    assert_eq!(run.reconnects, 1);
}

#[test]
fn restores_starknet_before_rejecting_a_malformed_os_reply() {
    // A truncated MCU field.
    let run = run(vec![
        app("BOLOS"),
        Reply(os_with(b"\x042.")),
        Reply(OK.to_vec()),
        Reconnected,
        app("Starknet"),
    ]);
    assert_eq!(run.end, refused(C::ManagementUnreadable));
    assert_eq!(run.sent.last(), Some(&(0xb0, 0x01)));
}

#[test]
fn the_os_reads_failure_comes_before_a_failed_reopen() {
    let run = run(vec![
        app("BOLOS"),
        Reply(os_with(b"\x042.")),
        Reply(OK.to_vec()),
        Reconnected,
        app("Ethereum"),
    ]);
    assert_eq!(run.end, refused(C::ManagementUnreadable));
}

#[test]
fn a_reconnect_that_cannot_bind_the_selected_device_fails_closed() {
    let run = run(vec![
        app("BOLOS"),
        Reply(current_os()),
        Reply(OK.to_vec()),
        Fail(TransportFailure::Disconnected),
    ]);
    assert_eq!(run.end, refused(C::DisconnectedMidSign));
    assert_eq!(run.sent.len(), 3);
}

// The person declining to open Starknet on the device: the open's refusal, never success.
#[test]
fn a_declined_open_app_is_not_reclassified_as_success() {
    let run = run(vec![
        app("BOLOS"),
        Reply(current_os()),
        Reply(vec![0x55, 0x01]),
    ]);
    assert_eq!(
        run.end,
        TransitionStep::Refused(LedgerRejection::with_status(C::ManagementRefused, 0x5501))
    );
    assert_eq!(run.reconnects, 0);
    let failed = run_with_open_failure(TransportFailure::Other);
    assert_eq!(failed, refused(C::UnexpectedDeviceError));
}

fn run_with_open_failure(failure: TransportFailure) -> TransitionStep {
    run(vec![app("BOLOS"), Reply(current_os()), Fail(failure)]).end
}

#[test]
fn a_transport_failure_mid_sequence_ends_it() {
    let run = run(vec![app("Starknet"), Fail(TransportFailure::Locked)]);
    assert_eq!(run.end, refused(C::DeviceLocked));
    let run = run_first(Fail(TransportFailure::Busy));
    assert_eq!(run, refused(C::DeviceBusy));
}

fn run_first(answer: Answer) -> TransitionStep {
    run(vec![answer]).end
}

#[test]
fn a_refused_management_command_names_the_refusal_not_a_missing_app() {
    assert_eq!(
        run_first(Reply(vec![0x69, 0x85])),
        TransitionStep::Refused(LedgerRejection::with_status(C::ManagementRefused, 0x6985))
    );
}

#[test]
fn an_os_below_the_minimum_is_refused_after_starknet_is_back() {
    let below = |os: Vec<u8>| {
        run(vec![
            app("BOLOS"),
            Reply(os),
            Reply(OK.to_vec()),
            Reconnected,
            app("Starknet"),
        ])
        .end
    };
    // Ledger's Nano X fixture runs OS 2.2.3.
    assert_eq!(below(nano_x_os()), refused(C::FirmwareBelowMinimum));
    // The legacy Nano S (0x3110…) has no Starknet 2.4 build; a Nano S Plus has no minimum here.
    let mut nano_s = current_os();
    nano_s[1] = 0x10;
    nano_s[0] = 0x31;
    assert_eq!(below(nano_s), refused(C::UnsupportedLedgerDevice));
    let mut nano_s_plus = current_os();
    nano_s_plus[1] = 0x10;
    assert_eq!(below(nano_s_plus), refused(C::UnsupportedLedgerDevice));
}

#[test]
fn every_step_after_an_end_is_refused() {
    let (mut gate, _) = OsGate::start(&MINIMUMS);
    assert_eq!(gate.reconnected(), refused(C::UnexpectedDeviceError));
    assert_eq!(
        gate.on_reply(Ok(&app_state(b"BOLOS", "1", &[]))),
        refused(C::UnexpectedDeviceError)
    );
}
