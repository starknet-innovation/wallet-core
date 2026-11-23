//! The Starknet app's three commands the extension uses (v1: blind signing only), as
//! `StarknetClient` sends them (`hw-app-starknet` 3.0.2, `lib/esm/index.js`) and `client.ts` reads
//! them: the app version, a public key read silently, and a signature over a hash.
//!
//! Cross-checked against two other sources: Software Mansion's Rust Ledger signer
//! (`starknet-rust-signers` 0.20.1, `src/ledger.rs`, used with real devices by starkli) and the
//! Starknet app's own protocol description (`LedgerHQ/app-starknet`, `docs/apdu.md`). Every request
//! byte agrees with all three. Replies are held to the exact lengths both Rust and the spec
//! require, stricter than the JavaScript library, which slices whatever it gets. One thing the Rust
//! signer does that this does not: below app 2.0.0 it shifts the hash left 4 bits; the wallet's
//! minimum (2.4.0, checked first with [`check_app_version`]) never reaches that branch.

use starknet_types_core::felt::Felt;

use super::rejection::{
    device_status, outcome, sign_status, thrown, LedgerRejection, Outcome, RejectionCode,
    STATUS_EXECUTION_ERROR,
};
use super::{apdu, LedgerPath, Reply, Version, STATUS_OK};
use crate::felt;

/// The Starknet app's class byte (`CLA`, `lib/esm/common.js:1`).
const CLA: u8 = 0x5a;
/// Its instructions (`INS`, `lib/esm/common.js:1-9`).
const INS_GET_VERSION: u8 = 0x00;
const INS_GET_PUB_KEY: u8 = 0x01;
const INS_SIGN_HASH: u8 = 0x02;

/// The signature's length byte ahead of `r ‖ s ‖ v` (`docs/apdu.md`: `L = 0x41`), which the Rust
/// signer requires too (`data[0] == 65`).
const SIGNATURE_LENGTH: u8 = 0x41;

/// A signature the device returned: `r` and `s`. The recovery byte after them is not read.
/// Unverified: the caller checks it against the account's key before anything is broadcast.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceSignature {
    pub r: Felt,
    pub s: Felt,
}

fn unexpected() -> LedgerRejection {
    LedgerRejection::new(RejectionCode::UnexpectedDeviceError)
}

/// `getAppVersion` (`index.js:43-52`): no parameters, no data.
pub fn app_version_request() -> Vec<u8> {
    apdu(CLA, INS_GET_VERSION, 0, 0, [])
}

/// `ledgerAppVersion`: the version's three bytes, exactly (`docs/apdu.md`; the Rust signer's
/// `VERSION_SIZE`). The library lets a transport throw through here, so this is where a locked,
/// busy or disconnected device shows.
pub fn parse_app_version(reply: Reply<'_>) -> Result<Version, LedgerRejection> {
    match outcome(reply) {
        Outcome::Thrown(throw) => Err(thrown(throw)),
        Outcome::Status(_, status) if status != STATUS_OK => Err(device_status(status)),
        Outcome::Status(data, _) => match data {
            [major, minor, patch] => Ok(Version {
                major: (*major).into(),
                minor: (*minor).into(),
                patch: (*patch).into(),
            }),
            _ => Err(unexpected()),
        },
    }
}

/// `assertLedgerAppReady`: refuses an app below the wallet's minimum. A gate on compatibility,
/// not on trust: every signature is still verified.
pub fn check_app_version(version: Version, minimum: Version) -> Result<(), LedgerRejection> {
    if version >= minimum {
        Ok(())
    } else {
        Err(LedgerRejection::new(RejectionCode::AppBelowMinimum))
    }
}

/// `getPubKey(path, false)` (`index.js:60-76`): P1 0, so the device shows nothing, as the
/// extension reads keys to enumerate accounts.
pub fn public_key_request(path: &LedgerPath) -> Vec<u8> {
    apdu(CLA, INS_GET_PUB_KEY, 0, 0, path.to_bytes())
}

/// `ledgerPublicKeyX`: the key's x-coordinate. The reply's data is exactly 65 bytes, a leading
/// byte then `X ‖ Y` (32 bytes each), as the Rust signer's `PUBLIC_KEY_SIZE` requires; only X is
/// kept. The leading byte is not checked: `docs/apdu.md` calls it a length (64) and the
/// extension's test a format byte (0x04), and neither Rust nor JavaScript reads it. The library
/// swallows every throw here into `ExecutionError`.
pub fn parse_public_key(reply: Reply<'_>) -> Result<Felt, LedgerRejection> {
    match outcome(reply) {
        Outcome::Thrown(_) => Err(device_status(STATUS_EXECUTION_ERROR)),
        Outcome::Status(_, status) if status != STATUS_OK => Err(device_status(status)),
        Outcome::Status(data, _) => data
            .get(1..33)
            .filter(|_| data.len() == 65)
            .and_then(|x| felt::from_be_bytes(x.try_into().ok()?))
            .ok_or_else(unexpected),
    }
}

/// `signHash(path, hash)` (`index.js:102-128`): the path (P1 0), then the hash (P1 1) as 32
/// big-endian bytes. A felt is below 2^252, so the library's 63-digit limit always holds.
pub fn sign_hash_requests(path: &LedgerPath, hash: &Felt) -> [Vec<u8>; 2] {
    [
        apdu(CLA, INS_SIGN_HASH, 0, 0, path.to_bytes()),
        apdu(CLA, INS_SIGN_HASH, 1, 0, hash.to_bytes_be()),
    ]
}

/// The reply to `signHash`'s first APDU: send the hash only after this is `Ok`. Stricter than the
/// library, which sends the hash after any status it accepts: any status but success stops here,
/// classified as the final reply's would be.
pub fn parse_sign_path(reply: Reply<'_>) -> Result<(), LedgerRejection> {
    match outcome(reply) {
        Outcome::Thrown(_) => Err(sign_status(STATUS_EXECUTION_ERROR)),
        Outcome::Status(_, status) if status != STATUS_OK => Err(sign_status(status)),
        Outcome::Status(..) => Ok(()),
    }
}

/// `ledgerSignHashOnDevice`: the reply to the hash, exactly 66 bytes: the length byte `0x41`,
/// then `r` (`data[1..33]`), `s` (`data[33..65]`) and the recovery byte, which is ignored
/// (`docs/apdu.md`; the Rust signer checks the same length and byte). The library swallows every
/// throw into `ExecutionError`.
pub fn parse_signature(reply: Reply<'_>) -> Result<DeviceSignature, LedgerRejection> {
    match outcome(reply) {
        Outcome::Thrown(_) => Err(sign_status(STATUS_EXECUTION_ERROR)),
        Outcome::Status(_, status) if status != STATUS_OK => Err(sign_status(status)),
        Outcome::Status(data, _) if data.len() != 66 || data.first() != Some(&SIGNATURE_LENGTH) => {
            Err(unexpected())
        }
        Outcome::Status(data, _) => {
            let part = |range: std::ops::Range<usize>| {
                felt::from_be_bytes(data.get(range)?.try_into().ok()?)
            };
            match (part(1..33), part(33..65)) {
                (Some(r), Some(s)) => Ok(DeviceSignature { r, s }),
                _ => Err(unexpected()),
            }
        }
    }
}
