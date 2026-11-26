//! Ledger's Bluetooth LE framing for the apps (P8, D1): the GATT identifiers each app scans and
//! connects by, the MTU handshake, and one exchange's frames out and reply in, over
//! `wallet_core::ledger::ble`. The apps keep only the Bluetooth I/O.
//!
//! The framing carries bytes and never reads, alters or authorises an APDU. What the device
//! answers still goes to the reader or plan that asked (`LedgerKeyReader`, a send's
//! `SendStep::Device`), and a signature is verified against the account's key before use. A
//! frame error ends the exchange as `TransportFailure::Other`, which the host passes on.

use std::sync::{Mutex, PoisonError};

use wallet_core::ledger::ble::{self, Reassembler};
use wallet_core::ledger::TransportFailure;

use crate::CoreError;

/// One device model's GATT service and characteristics, lowercase.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct LedgerBleSpec {
    /// Ledger's product name.
    pub model: String,
    /// The primary service a scan filters on.
    pub service: String,
    /// The device's reply frames and the MTU answer arrive here; subscribe before writing.
    pub notify: String,
    /// Write with response.
    pub write: String,
    /// Write without response; preferred when the characteristic allows it.
    pub write_cmd: String,
}

/// What every Bluetooth link starts from.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct LedgerBleLink {
    /// Every Bluetooth Ledger model's identifiers. A peripheral advertising none of these
    /// services is not a Ledger the wallet talks to.
    pub specs: Vec<LedgerBleSpec>,
    /// Written once after subscribing, to ask the device its frame size.
    pub mtu_request: Vec<u8>,
    /// The frame size until the device answers the MTU request.
    pub default_frame_size: u32,
}

/// The identifiers, MTU request and starting frame size for a Bluetooth link.
#[uniffi::export]
pub fn ledger_ble_link() -> LedgerBleLink {
    LedgerBleLink {
        specs: ble::BLE_SPECS
            .iter()
            .map(|spec| LedgerBleSpec {
                model: spec.model.into(),
                service: spec.service.into(),
                notify: spec.notify.into(),
                write: spec.write.into(),
                write_cmd: spec.write_cmd.into(),
            })
            .collect(),
        mtu_request: ble::MTU_REQUEST.to_vec(),
        // 20 always fits.
        default_frame_size: u32::try_from(ble::DEFAULT_FRAME_SIZE).unwrap_or(u32::MAX),
    }
}

/// The frame size in the device's answer to `ledger_ble_mtu_request`. The host frames at the
/// smaller of this and its own longest write. Refuses an answer that is not one.
#[uniffi::export]
pub fn ledger_ble_frame_size(mtu_reply: Vec<u8>) -> Result<u32, CoreError> {
    let size = ble::parse_mtu_reply(&mtu_reply).map_err(|_| CoreError::InvalidInput {
        reason: "not a Ledger MTU answer".into(),
    })?;
    Ok(u32::try_from(size).unwrap_or(u32::MAX))
}

/// What one notification did to a `LedgerBleExchange`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum LedgerBleStep {
    /// More frames are due.
    More,
    /// The device's whole reply, data then status word: pass it to the reader's or plan's reply.
    Reply { reply: Vec<u8> },
    /// The frames were malformed: pass `failure` to the reader's or plan's transport failure. The
    /// exchange is over.
    Failed { failure: TransportFailure },
}

/// One frame to write whole to the write characteristic.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct LedgerBleFrame {
    pub bytes: Vec<u8>,
}

/// One APDU's exchange: the frames to write, then the notifications that carry its reply.
#[derive(uniffi::Object)]
pub struct LedgerBleExchange {
    frames: Vec<LedgerBleFrame>,
    reassembler: Mutex<Reassembler>,
}

#[uniffi::export]
impl LedgerBleExchange {
    /// Frames `apdu` at `frame_size` bytes per write. Refuses an empty APDU, one over 65,535
    /// bytes, or a frame size of 5 or less.
    #[uniffi::constructor]
    pub fn new(apdu: Vec<u8>, frame_size: u32) -> Result<std::sync::Arc<Self>, CoreError> {
        let frame_size = usize::try_from(frame_size).unwrap_or(usize::MAX);
        let frames = ble::frame_apdu(&apdu, frame_size)
            .map_err(|_| CoreError::InvalidInput {
                reason: "Ledger APDU or frame size out of range".into(),
            })?
            .into_iter()
            .map(|bytes| LedgerBleFrame { bytes })
            .collect();
        Ok(std::sync::Arc::new(Self {
            frames,
            reassembler: Mutex::new(Reassembler::new()),
        }))
    }

    /// The frames to write, in order, each whole.
    pub fn frames(&self) -> Vec<LedgerBleFrame> {
        self.frames.clone()
    }

    /// Takes one notification from the notify characteristic.
    pub fn push(&self, frame: Vec<u8>) -> LedgerBleStep {
        let mut reassembler = self
            .reassembler
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        match reassembler.push(&frame) {
            Ok(None) => LedgerBleStep::More,
            Ok(Some(reply)) => LedgerBleStep::Reply { reply },
            Err(_) => LedgerBleStep::Failed {
                failure: TransportFailure::Other,
            },
        }
    }
}
