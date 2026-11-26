//! The device OS gate as one sans-IO sequence (`readLedgerOsAndRestoreStarknet`,
//! `device-transition.ts`, then `assertLedgerFirmwareReady`, `client.ts`): the device answers
//! `GetOsVersion` only on its dashboard, so an open app is closed first and Starknet reopened
//! after, whatever the OS read said.
//!
//! 1. `GetAppAndVersion`. On the dashboard, go to 3.
//! 2. `CloseApp`, then reconnect: closing an app drops the link, and the extension reconnects to
//!    the exact device even when the close was acknowledged. Then `GetAppAndVersion` must name
//!    the dashboard (`dashboard-restore-failed`).
//! 3. `GetOsVersion`. A failed read is kept, not thrown, so Starknet is still reopened.
//! 4. `OpenApp("Starknet")` (the person confirms it on the device), then reconnect, then
//!    `GetAppAndVersion` must name Starknet (`starknet-reopen-failed`). A failure here is reported
//!    only when the OS read succeeded; otherwise the OS read's failure is.
//! 5. The model and version against the wallet's minimums (`unsupported-ledger-device`,
//!    `firmware-below-minimum`).
//!
//! A transport that reports `Disconnected` in place of the close's or open's answer is the
//! expected drop, not a failure. Any other failure, or a reconnect the host could not make to the
//! same device, ends the sequence. A reply's failing status word is named as the parsers name it
//! (`management::data_of`): a locked device or a missing app as the extension's transport names
//! them, the person declining the app change as `rejected-on-device`, any other word as
//! `management-refused`.

use super::management::{data_of, firmware, parse_app_state, parse_os_version};
use super::rejection::{thrown, Thrown};
use super::{
    app_state_request, check_firmware, open_starknet_request, os_version_request, quit_app_request,
    DeviceModel, Firmware, LedgerRejection, OsVersion, RejectionCode, Reply, TransportFailure,
    Version, STARKNET_APP_NAME,
};

/// The step the sequence is on, for the host's progress display only
/// (`LedgerTransitionPhase`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionPhase {
    CheckingDevice,
    ReturningToDashboard,
    ReconnectingDashboard,
    CheckingFirmware,
    OpeningStarknet,
    ReconnectingStarknet,
    ConfirmingStarknet,
}

impl TransitionPhase {
    /// The extension's name for the phase.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CheckingDevice => "checking-device",
            Self::ReturningToDashboard => "returning-to-dashboard",
            Self::ReconnectingDashboard => "reconnecting-dashboard",
            Self::CheckingFirmware => "checking-firmware",
            Self::OpeningStarknet => "opening-starknet",
            Self::ReconnectingStarknet => "reconnecting-starknet",
            Self::ConfirmingStarknet => "confirming-starknet",
        }
    }
}

/// What the host does next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransitionStep {
    /// Send these bytes; pass the answer to [`OsGate::on_reply`].
    Send(Vec<u8>, TransitionPhase),
    /// Reconnect to the same device (never another one), then call [`OsGate::reconnected`], or
    /// [`OsGate::on_reply`] with the failure if it could not.
    Reconnect(TransitionPhase),
    /// Done, Starknet reopened: a supported model at or above the wallet's minimum.
    Supported(Firmware),
    /// Done with the device's OS unchecked.
    Refused(LedgerRejection),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Initial,
    Closing,
    ReconnectingDashboard,
    ConfirmingDashboard,
    ReadingOs,
    Opening,
    ReconnectingStarknet,
    ConfirmingStarknet,
    Ended,
}

/// One OS gate on one device. Every step after an end is refused.
#[derive(Debug)]
pub struct OsGate {
    minimums: Vec<(DeviceModel, Version)>,
    state: State,
    os: Option<Result<OsVersion, LedgerRejection>>,
}

impl OsGate {
    /// Starts the gate with the wallet's OS minimums by model (a model missing is unsupported).
    pub fn start(minimums: &[(DeviceModel, Version)]) -> (Self, TransitionStep) {
        let gate = Self {
            minimums: minimums.to_vec(),
            state: State::Initial,
            os: None,
        };
        let step = TransitionStep::Send(app_state_request(), TransitionPhase::CheckingDevice);
        (gate, step)
    }

    /// The device's answer to the last APDU, or what the transport (or a reconnect) reported.
    pub fn on_reply(&mut self, reply: Reply<'_>) -> TransitionStep {
        match self.state {
            State::Initial => match parse_app_state(reply) {
                Ok(app) if app.is_dashboard() => self.read_os(),
                Ok(_) => self.send(
                    State::Closing,
                    quit_app_request(),
                    TransitionPhase::ReturningToDashboard,
                ),
                Err(rejection) => self.refuse(rejection),
            },
            State::Closing => match dropped(reply) {
                Ok(()) => self.wait(
                    State::ReconnectingDashboard,
                    TransitionPhase::ReconnectingDashboard,
                ),
                Err(rejection) => self.refuse(rejection),
            },
            State::ConfirmingDashboard => match parse_app_state(reply) {
                Ok(app) if app.is_dashboard() => self.read_os(),
                Ok(_) => self.refuse(LedgerRejection::new(RejectionCode::DashboardRestoreFailed)),
                Err(rejection) => self.refuse(rejection),
            },
            State::ReadingOs => {
                self.os = Some(parse_os_version(reply));
                self.send(
                    State::Opening,
                    open_starknet_request(),
                    TransitionPhase::OpeningStarknet,
                )
            }
            State::Opening => match dropped(reply) {
                Ok(()) => self.wait(
                    State::ReconnectingStarknet,
                    TransitionPhase::ReconnectingStarknet,
                ),
                Err(rejection) => self.reopen_failed(rejection),
            },
            State::ConfirmingStarknet => match parse_app_state(reply) {
                Ok(app) if app.name == STARKNET_APP_NAME => self.finish(),
                Ok(_) => {
                    self.reopen_failed(LedgerRejection::new(RejectionCode::StarknetReopenFailed))
                }
                Err(rejection) => self.reopen_failed(rejection),
            },
            // A reconnect that failed (the host reports it as the transport's failure).
            State::ReconnectingDashboard => self.refuse(failure_of(reply)),
            State::ReconnectingStarknet => self.reopen_failed(failure_of(reply)),
            State::Ended => self.refuse(unexpected()),
        }
    }

    /// The host reconnected to the same device after [`TransitionStep::Reconnect`].
    pub fn reconnected(&mut self) -> TransitionStep {
        match self.state {
            State::ReconnectingDashboard => self.send(
                State::ConfirmingDashboard,
                app_state_request(),
                TransitionPhase::ReconnectingDashboard,
            ),
            State::ReconnectingStarknet => self.send(
                State::ConfirmingStarknet,
                app_state_request(),
                TransitionPhase::ConfirmingStarknet,
            ),
            _ => self.refuse(unexpected()),
        }
    }

    /// Ends the gate: this and every later step is refused.
    pub fn end(&mut self) -> TransitionStep {
        self.refuse(unexpected())
    }

    fn send(&mut self, state: State, apdu: Vec<u8>, phase: TransitionPhase) -> TransitionStep {
        self.state = state;
        TransitionStep::Send(apdu, phase)
    }

    fn wait(&mut self, state: State, phase: TransitionPhase) -> TransitionStep {
        self.state = state;
        TransitionStep::Reconnect(phase)
    }

    fn read_os(&mut self) -> TransitionStep {
        self.send(
            State::ReadingOs,
            os_version_request(),
            TransitionPhase::CheckingFirmware,
        )
    }

    fn refuse(&mut self, rejection: LedgerRejection) -> TransitionStep {
        self.state = State::Ended;
        TransitionStep::Refused(rejection)
    }

    /// Reopening Starknet failed: the OS read's own failure comes first.
    fn reopen_failed(&mut self, rejection: LedgerRejection) -> TransitionStep {
        match self.os.take() {
            Some(Err(os)) => self.refuse(os),
            _ => self.refuse(rejection),
        }
    }

    fn finish(&mut self) -> TransitionStep {
        let checked = match self.os.take() {
            Some(Ok(os)) => firmware(&os)
                .and_then(|firmware| check_firmware(&firmware, &self.minimums).map(|()| firmware)),
            Some(Err(rejection)) => Err(rejection),
            None => Err(unexpected()),
        };
        self.state = State::Ended;
        match checked {
            Ok(firmware) => TransitionStep::Supported(firmware),
            Err(rejection) => TransitionStep::Refused(rejection),
        }
    }
}

fn unexpected() -> LedgerRejection {
    LedgerRejection::new(RejectionCode::UnexpectedDeviceError)
}

/// A close's or open's answer: acknowledged, or the link dropped as the app changed.
fn dropped(reply: Reply<'_>) -> Result<(), LedgerRejection> {
    match reply {
        Err(TransportFailure::Disconnected) => Ok(()),
        reply => data_of(reply).map(|_| ()),
    }
}

/// What a failed reconnect (or a reply where none was asked for) ends the gate with.
fn failure_of(reply: Reply<'_>) -> LedgerRejection {
    match reply {
        Err(failure) => thrown(Thrown::Transport(failure)),
        Ok(_) => unexpected(),
    }
}
