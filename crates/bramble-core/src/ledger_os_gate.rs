//! The Ledger device OS gate the apps run at the start of the connect flow (P8): the extension's
//! `assertLedgerFirmwareReady` over `readLedgerOsAndRestoreStarknet`, with Bramble's
//! `LEDGER_OS_MIN`. The device reports its OS only from the dashboard, so an open app is closed
//! and Starknet reopened (the person confirms that on the device); the host carries the bytes and
//! reconnects to the same device when told to (`wallet_core::ledger::OsGate`). The Starknet app's
//! own version is still checked on every signature (`LEDGER_APP_MIN`).

use std::sync::{Arc, Mutex, PoisonError};

use wallet_core::ledger::{OsGate, TransitionStep, TransportFailure};

use crate::ledger::LEDGER_OS_MIN;

/// What the host does next with a `LedgerOsGate`. `phase` is the extension's progress name
/// (`checking-device`, `returning-to-dashboard`, `reconnecting-dashboard`, `checking-firmware`,
/// `opening-starknet`, `reconnecting-starknet`, `confirming-starknet`), for display only.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum LedgerOsStep {
    /// Send `apdu` to the device; pass its answer to `reply`, or what the transport reported to
    /// `transport_failure` (a drop while an app closes or opens is expected: report it as
    /// `Disconnected`).
    Send { apdu: Vec<u8>, phase: String },
    /// An app closed or opened, which drops the link: reconnect to the same device, never another
    /// one, then call `reconnected`, or `transport_failure` if it could not.
    Reconnect { phase: String },
    /// Done, with Starknet open again: a supported `model` (`nano-x`, `nano-s-plus`, `stax`,
    /// `flex`, `nano-gen5`) on OS `version` at or above Bramble's minimum for it.
    Supported { model: String, version: String },
    /// Done: `reason` is the extension's rejection code (`firmware-below-minimum`,
    /// `unsupported-ledger-device`, `dashboard-restore-failed`, `starknet-reopen-failed`,
    /// `management-refused`, `management-unreadable`, `device-locked`, ...). A new gate starts
    /// again.
    Refused { reason: String },
}

fn step(step: TransitionStep) -> LedgerOsStep {
    match step {
        TransitionStep::Send(apdu, phase) => LedgerOsStep::Send {
            apdu,
            phase: phase.as_str().into(),
        },
        TransitionStep::Reconnect(phase) => LedgerOsStep::Reconnect {
            phase: phase.as_str().into(),
        },
        TransitionStep::Supported(firmware) => LedgerOsStep::Supported {
            model: firmware.model.as_str().into(),
            version: format!(
                "{}.{}.{}",
                firmware.version.major, firmware.version.minor, firmware.version.patch
            ),
        },
        TransitionStep::Refused(rejection) => LedgerOsStep::Refused {
            reason: rejection.code.as_str().into(),
        },
    }
}

/// One OS check of one device.
#[derive(uniffi::Object)]
pub struct LedgerOsGate {
    gate: Mutex<OsGate>,
    first: Mutex<Option<LedgerOsStep>>,
}

#[uniffi::export]
impl LedgerOsGate {
    /// A gate with Bramble's OS minimums.
    #[uniffi::constructor]
    pub fn new() -> Arc<Self> {
        let (gate, first) = OsGate::start(&LEDGER_OS_MIN);
        Arc::new(Self {
            gate: Mutex::new(gate),
            first: Mutex::new(Some(step(first))),
        })
    }

    /// The first step. Asked again, the gate ends: that step and every later one is refused.
    pub fn start(&self) -> LedgerOsStep {
        let first = self.take_first();
        first.unwrap_or_else(|| self.advance(OsGate::end))
    }

    /// The device's answer to the last APDU: its data, then the status word.
    pub fn reply(&self, reply: Vec<u8>) -> LedgerOsStep {
        self.take_first();
        self.advance(|gate| gate.on_reply(Ok(&reply)))
    }

    /// What the transport reported in place of an answer, or why a reconnect failed.
    pub fn transport_failure(&self, failure: TransportFailure) -> LedgerOsStep {
        self.take_first();
        self.advance(|gate| gate.on_reply(Err(failure)))
    }

    /// The host reconnected to the same device after `Reconnect`.
    pub fn reconnected(&self) -> LedgerOsStep {
        self.take_first();
        self.advance(OsGate::reconnected)
    }
}

impl LedgerOsGate {
    /// The first step is spent once the host answers anything.
    fn take_first(&self) -> Option<LedgerOsStep> {
        self.first
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }

    fn advance(&self, f: impl FnOnce(&mut OsGate) -> TransitionStep) -> LedgerOsStep {
        step(f(&mut self
            .gate
            .lock()
            .unwrap_or_else(PoisonError::into_inner)))
    }
}
