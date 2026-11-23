//! The Starknet app's three commands the extension uses (v1: blind signing only), as
//! `StarknetClient` sends them (`hw-app-starknet` 3.0.2, `lib/esm/index.js`) and `client.ts` reads
//! them: the app version, a public key read silently, and a signature over a hash.

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

/// `ledgerAppVersion`: the version's three bytes. The library lets a transport throw through
/// here, so this is where a locked, busy or disconnected device shows.
pub fn parse_app_version(reply: Reply<'_>) -> Result<Version, LedgerRejection> {
    match outcome(reply) {
        Outcome::Thrown(throw) => Err(thrown(throw)),
        Outcome::Status(_, status) if status != STATUS_OK => Err(device_status(status)),
        Outcome::Status(data, _) => match data {
            [major, minor, patch, ..] => Ok(Version {
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

/// `ledgerPublicKeyX`: the key's x-coordinate. The reply's data is a format byte, then `X ‖ Y`
/// (32 bytes each), of which the library returns `data[1..65]`; only X is kept. The library
/// swallows every throw here into `ExecutionError`.
pub fn parse_public_key(reply: Reply<'_>) -> Result<Felt, LedgerRejection> {
    match outcome(reply) {
        Outcome::Thrown(_) => Err(device_status(STATUS_EXECUTION_ERROR)),
        Outcome::Status(_, status) if status != STATUS_OK => Err(device_status(status)),
        Outcome::Status(data, _) => data
            .get(1..33)
            .filter(|_| data.len() >= 65)
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

/// `ledgerSignHashOnDevice`: the reply to the hash, `r` at `data[1..33]`, `s` at `data[33..65]`,
/// the recovery byte after them ignored. The library swallows every throw into `ExecutionError`.
pub fn parse_signature(reply: Reply<'_>) -> Result<DeviceSignature, LedgerRejection> {
    match outcome(reply) {
        Outcome::Thrown(_) => Err(sign_status(STATUS_EXECUTION_ERROR)),
        Outcome::Status(_, status) if status != STATUS_OK => Err(sign_status(status)),
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
