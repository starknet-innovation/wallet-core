//! One signature from a Ledger, sans-IO: what the extension's side panel does on the device
//! (`sign-session.ts`, `signWith`) and its background then checks (`ledger-signer.ts`,
//! `ledgerStarkHashSigner`), as one exchange the host only relays.
//!
//! In order: the Starknet app's version against the wallet's minimum; the public key at the
//! account's path, which must be the account's stored key (`wrong-device` otherwise, before the
//! person is asked to confirm); the two `signHash` APDUs; and the device's `(r, s)` verified over
//! this exchange's hash against that stored key through Krusty. Only a [`VerifiedSignature`]
//! leaves, and nothing else can make one, so a caller cannot broadcast an unchecked signature.
//!
//! Departures from the extension, each deliberate: the app version is read on every signature
//! (the panel skips it when the connect flow already checked); the device OS gate
//! (`assertLedgerFirmwareReady`) is not run here, since it needs the dashboard round trip and is a
//! compatibility gate, not a trust decision; and a zero hash is refused before any APDU, since
//! anyone can forge a signature over it.

use starknet_types_core::felt::Felt;

use super::rejection::{LedgerRejection, RejectionCode};
use super::starknet::{
    app_version_request, check_app_version, parse_app_version, parse_public_key, parse_sign_path,
    parse_signature, public_key_request, sign_hash_requests,
};
use super::{LedgerPath, Reply, Version};
use crate::kms;

/// A device signature that verified over the exchange's hash under the account's stored key.
/// Made only by [`DeviceSigning`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifiedSignature {
    r: Felt,
    s: Felt,
}

impl VerifiedSignature {
    pub fn r(&self) -> Felt {
        self.r
    }

    pub fn s(&self) -> Felt {
        self.s
    }
}

/// What the host does next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceStep {
    /// Send these bytes to the device and pass its reply, or the transport's failure, to
    /// [`DeviceSigning::on_reply`].
    Send(Vec<u8>),
    /// Done: the signature to use.
    Signed(VerifiedSignature),
    /// Done: nothing usable was signed.
    Refused(LedgerRejection),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    AppVersion,
    PublicKey,
    SignPath,
    SignHash,
    Ended,
}

/// One hash signed on a device for one account. Every step after an end is refused.
#[derive(Debug)]
pub struct DeviceSigning {
    path: LedgerPath,
    hash: Felt,
    public_key: Felt,
    app_minimum: Version,
    stage: Stage,
}

impl DeviceSigning {
    /// Starts signing `hash` at the account's `path`, whose stored key is `public_key`. Returns
    /// the exchange and its first step.
    pub fn start(
        path: LedgerPath,
        hash: Felt,
        public_key: Felt,
        app_minimum: Version,
    ) -> (Self, DeviceStep) {
        let mut signing = Self {
            path,
            hash,
            public_key,
            app_minimum,
            stage: Stage::AppVersion,
        };
        let step = if hash == Felt::ZERO {
            signing.refuse(LedgerRejection::new(RejectionCode::SignatureVerifyFailed))
        } else {
            DeviceStep::Send(app_version_request())
        };
        (signing, step)
    }

    /// Feeds the device's reply to the last APDU sent.
    pub fn on_reply(&mut self, reply: Reply<'_>) -> DeviceStep {
        match self.advance(reply) {
            Ok(step) => step,
            Err(rejection) => self.refuse(rejection),
        }
    }

    fn advance(&mut self, reply: Reply<'_>) -> Result<DeviceStep, LedgerRejection> {
        let (next, step) = match self.stage {
            Stage::AppVersion => {
                check_app_version(parse_app_version(reply)?, self.app_minimum)?;
                (
                    Stage::PublicKey,
                    DeviceStep::Send(public_key_request(&self.path)),
                )
            }
            Stage::PublicKey => {
                if parse_public_key(reply)? != self.public_key {
                    return Err(LedgerRejection::new(RejectionCode::WrongDevice));
                }
                let [path_apdu, _] = sign_hash_requests(&self.path, &self.hash);
                (Stage::SignPath, DeviceStep::Send(path_apdu))
            }
            Stage::SignPath => {
                parse_sign_path(reply)?;
                let [_, hash_apdu] = sign_hash_requests(&self.path, &self.hash);
                (Stage::SignHash, DeviceStep::Send(hash_apdu))
            }
            Stage::SignHash => {
                let signature = parse_signature(reply)?;
                if !kms::stark_signature_is_valid(
                    &self.public_key,
                    &self.hash,
                    &signature.r,
                    &signature.s,
                ) {
                    return Err(LedgerRejection::new(RejectionCode::SignatureVerifyFailed));
                }
                (
                    Stage::Ended,
                    DeviceStep::Signed(VerifiedSignature {
                        r: signature.r,
                        s: signature.s,
                    }),
                )
            }
            Stage::Ended => return Err(LedgerRejection::new(RejectionCode::UnexpectedDeviceError)),
        };
        self.stage = next;
        Ok(step)
    }

    fn refuse(&mut self, rejection: LedgerRejection) -> DeviceStep {
        self.stage = Stage::Ended;
        DeviceStep::Refused(rejection)
    }
}
