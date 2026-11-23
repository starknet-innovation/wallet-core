//! The extension's Ledger rejection catalogue (`LedgerRejectionCode`, `ledger-protocol/src/
//! errors.ts`) and the mappings `client.ts` makes onto it. The codes and their names are the
//! extension's; the user-facing copy is the wallet's and is not here. A rejection only says what
//! happened: it never authorises anything.

/// Every code in the extension's catalogue. Many belong to flows outside the device protocol
/// (the browser picker, the panel, submission); they are here so a wallet has the whole set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RejectionCode {
    /// The runtime cannot reach a Ledger at all.
    WebhidUnavailable,
    /// The person closed the device chooser.
    PickerCancelled,
    /// The device is locked.
    DeviceLocked,
    /// The Starknet app is not open (the device answered as the dashboard or another app).
    NoAppOpen,
    /// The device OS is below the wallet's minimum for its model.
    FirmwareBelowMinimum,
    /// The Starknet app is below the wallet's minimum.
    AppBelowMinimum,
    /// A device model (or target id) the wallet does not support.
    UnsupportedLedgerDevice,
    /// A blind sign refused: by the person, or because Blind signing is off, or the link failed
    /// mid-sign. The Starknet app's reply does not tell these apart.
    RejectedOrBlindSigningOff,
    /// Another program holds the device.
    DeviceBusy,
    /// The device must be reconnected.
    Replug,
    /// More than one device: the person must pick the account's.
    MultipleDevices,
    /// Not the device the account was set up on.
    WrongDevice,
    /// The account was set up for another network.
    NetworkMismatch,
    /// An undeployed account with nothing to pay for its deployment.
    DeployUnfunded,
    /// The person rejected the request on the device.
    RejectedOnDevice,
    /// The device locked itself while a signature was pending.
    LockedMidSign,
    /// The device disconnected; nothing was submitted.
    DisconnectedMidSign,
    /// The window holding the device closed before the person confirmed.
    PanelContextLost,
    /// Still waiting for the person to confirm.
    SignTimeout,
    /// The device's signature did not verify against the account's key; nothing was submitted.
    SignatureVerifyFailed,
    /// The wallet restarted mid-sign.
    WorkerEvicted,
    /// Submitted, but the transaction failed on chain.
    ChainFailed,
    /// The dashboard refused a management command (its status word attached).
    ManagementRefused,
    /// A management reply this wallet cannot read.
    ManagementUnreadable,
    /// The device did not return to its dashboard.
    DashboardRestoreFailed,
    /// The device did not reopen the Starknet app.
    StarknetReopenFailed,
    /// Anything else: the fail-closed default.
    UnexpectedDeviceError,
}

impl RejectionCode {
    /// The extension's name for the code.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::WebhidUnavailable => "webhid-unavailable",
            Self::PickerCancelled => "picker-cancelled",
            Self::DeviceLocked => "device-locked",
            Self::NoAppOpen => "no-app-open",
            Self::FirmwareBelowMinimum => "firmware-below-minimum",
            Self::AppBelowMinimum => "app-below-minimum",
            Self::UnsupportedLedgerDevice => "unsupported-ledger-device",
            Self::RejectedOrBlindSigningOff => "rejected-or-blind-signing-off",
            Self::DeviceBusy => "device-busy",
            Self::Replug => "replug",
            Self::MultipleDevices => "multiple-devices",
            Self::WrongDevice => "wrong-device",
            Self::NetworkMismatch => "network-mismatch",
            Self::DeployUnfunded => "deploy-unfunded",
            Self::RejectedOnDevice => "rejected-on-device",
            Self::LockedMidSign => "locked-mid-sign",
            Self::DisconnectedMidSign => "disconnected-mid-sign",
            Self::PanelContextLost => "panel-context-lost",
            Self::SignTimeout => "sign-timeout",
            Self::SignatureVerifyFailed => "signature-verify-failed",
            Self::WorkerEvicted => "worker-evicted",
            Self::ChainFailed => "chain-failed",
            Self::ManagementRefused => "management-refused",
            Self::ManagementUnreadable => "management-unreadable",
            Self::DashboardRestoreFailed => "dashboard-restore-failed",
            Self::StarknetReopenFailed => "starknet-reopen-failed",
            Self::UnexpectedDeviceError => "unexpected-device-error",
        }
    }
}

/// A typed rejection (`LedgerRejection`): the code, and the APDU status word behind it when
/// there was one, for a support line, never the headline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{}", .code.as_str())]
pub struct LedgerRejection {
    pub code: RejectionCode,
    pub status_word: Option<u16>,
}

impl LedgerRejection {
    pub(crate) fn new(code: RejectionCode) -> Self {
        Self {
            code,
            status_word: None,
        }
    }

    pub(crate) fn with_status(code: RejectionCode, status_word: u16) -> Self {
        Self {
            code,
            status_word: Some(status_word),
        }
    }
}

/// A failure the host's transport reports in place of a reply. The extension recognises a thrown
/// error by its text (`THROWN_SIGNATURES`, `client.ts`); a host names the kind instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportFailure {
    /// The device is locked (`/locked/i`).
    Locked,
    /// The device disconnected (`/disconnect/i`).
    Disconnected,
    /// Another program holds the device (`/already open|in use|claimed|busy/i`).
    Busy,
    /// Anything else.
    Other,
}

/// Status words the Starknet app's replies carry (`LedgerError`, `lib/esm/common.js:10`).
pub(crate) const STATUS_BAD_CLA: u16 = 0x6e00;
pub(crate) const STATUS_BAD_INS: u16 = 0x6e01;
pub(crate) const STATUS_INVALID_P1P2: u16 = 0x6e02;
pub(crate) const STATUS_USER_REJECTED: u16 = 0x6e04;
/// What the library reports for every failure it swallows.
pub(crate) const STATUS_EXECUTION_ERROR: u16 = 0x6f00;

/// What `StarknetClient.sendApdu` (`lib/esm/index.js:22-38`) makes of a reply: the status words it
/// passes through, or a throw from the transport for anything else.
pub(crate) enum Outcome<'a> {
    /// A status the library accepts (`NoError`, `BadCla`, `BadIns`, `InvalidP1P2`,
    /// `UserRejected`), with the reply's data.
    Status(&'a [u8], u16),
    /// The transport threw.
    Thrown(Thrown),
}

/// What the transport threw.
pub(crate) enum Thrown {
    /// `TransportStatusError` for a status word outside the library's list.
    Status(u16),
    /// A reply too short to hold a status word.
    Short,
    /// The host's own failure.
    Transport(TransportFailure),
}

pub(crate) fn outcome(reply: super::Reply<'_>) -> Outcome<'_> {
    let reply = match reply {
        Ok(reply) => reply,
        Err(failure) => return Outcome::Thrown(Thrown::Transport(failure)),
    };
    match super::split(reply) {
        None => Outcome::Thrown(Thrown::Short),
        Some((data, status)) => match status {
            super::STATUS_OK
            | STATUS_BAD_CLA
            | STATUS_BAD_INS
            | STATUS_INVALID_P1P2
            | STATUS_USER_REJECTED => Outcome::Status(data, status),
            _ => Outcome::Thrown(Thrown::Status(status)),
        },
    }
}

/// `mapThrown`: a throw that reached the extension. A thrown status word is matched by the text
/// `TransportStatusError` gives it (`@ledgerhq/errors` 6.37.0): `INS_NOT_SUPPORTED (0x6d00)` names
/// a missing app; `Locked device (0x5515)`, `dongle locked` (0x6982) and `CODE_BLOCKED` (0x9840)
/// all match `/locked/i`. No other status word's text matches a signature.
pub(crate) fn thrown(thrown: Thrown) -> LedgerRejection {
    let code = match thrown {
        Thrown::Status(0x6d00) => RejectionCode::NoAppOpen,
        Thrown::Status(0x5515 | 0x6982 | 0x9840) | Thrown::Transport(TransportFailure::Locked) => {
            RejectionCode::DeviceLocked
        }
        Thrown::Transport(TransportFailure::Disconnected) => RejectionCode::DisconnectedMidSign,
        Thrown::Transport(TransportFailure::Busy) => RejectionCode::DeviceBusy,
        Thrown::Status(_) | Thrown::Short | Thrown::Transport(TransportFailure::Other) => {
            RejectionCode::UnexpectedDeviceError
        }
    };
    LedgerRejection::new(code)
}

/// `classifyDeviceStatus`: a silent read's failing status. `UserRejected` is a rejection, a wrong
/// CLA or INS means the Starknet app is not open, and anything else (`ExecutionError` among them,
/// which is not actionable) fails closed.
pub(crate) fn device_status(status: u16) -> LedgerRejection {
    let code = match status {
        STATUS_USER_REJECTED => RejectionCode::RejectedOnDevice,
        STATUS_BAD_CLA | STATUS_BAD_INS => RejectionCode::NoAppOpen,
        _ => RejectionCode::UnexpectedDeviceError,
    };
    LedgerRejection::with_status(code, status)
}

/// `classifySignStatus`: `signHash`'s failing status. The app (2.4.0 to 2.4.3) answers a refusal
/// and Blind signing being off alike with 0x6985, which the library folds into `ExecutionError`
/// together with a mid-sign transport failure, so that names no cause.
pub(crate) fn sign_status(status: u16) -> LedgerRejection {
    let code = match status {
        STATUS_USER_REJECTED => RejectionCode::RejectedOnDevice,
        STATUS_EXECUTION_ERROR => RejectionCode::RejectedOrBlindSigningOff,
        _ => RejectionCode::UnexpectedDeviceError,
    };
    LedgerRejection::with_status(code, status)
}
