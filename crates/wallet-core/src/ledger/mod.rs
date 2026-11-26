//! The Ledger device protocol as sans-IO APDU bytes: the extension's `ledger-protocol` package
//! (`client.ts`, `errors.ts`, the management replies of `device-transition.ts`) over the device
//! library it pins, `@ledgerhq/hw-app-starknet` 3.0.2 (Apache-2.0).
//!
//! Each command is a builder returning the exact APDUs the library sends, in order, and a reader
//! taking the device's reply (data then the two status-word bytes) or the failure the host's
//! transport reported. A reader returns a typed value or a [`LedgerRejection`] from the
//! extension's catalogue; it never panics on a reply, and anything it does not recognise is
//! `unexpected-device-error`. The readers reproduce what the library does to a reply before the
//! extension sees it (which status words it passes through, which it folds into `ExecutionError`,
//! which throw), since the extension's classification depends on it:
//!
//! - the Starknet app (`starknet`): its version, a public key read silently, and a blind signature
//!   over a hash (`signHash`);
//! - the dashboard (`management`): the open app, the OS version, the device model and the firmware
//!   gate, and the commands that close and reopen an app.
//!
//! Minimum versions are a wallet's choice and arguments here (Bramble's are `bramble-core`'s
//! `ledger`). One signature end to end, verified through Krusty against the account's stored key
//! before it can be used, is `sign::DeviceSigning`; the readers alone never verify. That exchange
//! under the wallet's rules (a lock stops it, a refusal parks it for a retry), for every signature
//! a Ledger account makes, is `hash_signing::HashSigning`. A page of
//! public keys for an account picker is `enumerate::KeyEnumeration`. The
//! dashboard-and-reopen sequence that reads the OS version (`readLedgerOsAndRestoreStarknet`) and
//! the transport's I/O are not here; the transport's Bluetooth framing, which only carries bytes, is
//! `ble`.
//!
//! The vectors are the library's encoding and the extension's tests. Golden transcripts from a
//! device or Ledger's Speculos emulator are owed to a Bramble generator; none is in
//! `tests/fixtures/` yet.

pub mod ble;
mod enumerate;
mod hash_signing;
mod management;
mod path;
mod rejection;
mod sign;
mod starknet;

#[cfg(test)]
mod ble_tests;
#[cfg(test)]
mod enumerate_tests;
#[cfg(test)]
mod hash_signing_tests;
#[cfg(test)]
mod management_tests;
#[cfg(test)]
mod sign_tests;
#[cfg(test)]
mod starknet_tests;

pub use enumerate::{DeviceKeyAt, EnumerationStep, KeyEnumeration, MAX_PAGE};
pub use hash_signing::{DeviceKey, HashFailure, HashSigning, HashStep};
pub use management::{
    app_state_request, check_firmware, firmware, open_starknet_request, os_version_request,
    parse_app_state, parse_os_version, quit_app_request, AppState, DeviceModel, Firmware,
    OsVersion, STARKNET_APP_NAME,
};
pub use path::{LedgerPath, PathComponent};
pub use rejection::{LedgerRejection, RejectionCode, TransportFailure};
pub use sign::{DeviceSigning, DeviceStep, VerifiedSignature};
pub use starknet::{
    app_version_request, check_app_version, parse_app_version, parse_public_key, parse_sign_path,
    parse_signature, public_key_request, sign_hash_requests, DeviceSignature,
};

/// The device's reply to one APDU (its data, then the status word), or the failure the host's
/// transport reported instead.
pub type Reply<'a> = Result<&'a [u8], TransportFailure>;

/// A three-part version: the Starknet app's (each part a byte) or the device OS's (each part at
/// most `0xffff`). Ordered part by part, as `meetsMinimum` compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

/// The status word that means success.
const STATUS_OK: u16 = 0x9000;

/// One APDU as `Transport.send` frames it (`hw-transport` 6.35.6, `Transport.js:185`): the header,
/// a one-byte data length, the data. The length fits a byte by construction.
fn apdu<const N: usize>(cla: u8, ins: u8, p1: u8, p2: u8, data: [u8; N]) -> Vec<u8> {
    const { assert!(N < 256) };
    let mut bytes = vec![cla, ins, p1, p2, N as u8];
    bytes.extend_from_slice(&data);
    bytes
}

/// A reply's data and status word, or `None` for a reply too short to hold one (the transport's
/// `readUInt16BE` throws on it).
fn split(reply: &[u8]) -> Option<(&[u8], u16)> {
    let (data, status) = reply.split_at_checked(reply.len().checked_sub(2)?)?;
    Some((data, u16::from_be_bytes(status.try_into().ok()?)))
}
