//! One hash signed on a Ledger under the wallet's rules (P8): the extension's
//! `accountHashSigner` for a keyless Ledger account, around [`DeviceSigning`]'s exchange. Every
//! signature a Ledger account makes goes through this: inside a send plan
//! (`send::plan_device`) and outside one (an Argent multisig co-signature).
//!
//! What it adds to the exchange, as the extension's sign queue and side panel do:
//!
//! - a lock stops it: the session is asked before the first APDU and before each answer is read,
//!   so no further APDU reaches the device (`rejectAll` on lock), and a signature the device
//!   returns after the session ended is dropped (rejected at settle time);
//! - a refusal (locked, app not open, blind signing off, the wrong device, a lost link) parks it,
//!   as the side panel keeps a refused request parked (`sign-session.ts`): [`HashSigning::retry`]
//!   signs the same hash again from the start;
//! - a signature that does not verify over the hash under the account's stored key ends it, as
//!   the background gate throws (`ledgerStarkHashSigner`).
//!
//! The caller supplies the hash. Each caller signs only a hash it computed or re-verified itself,
//! never one a host or a dapp hands over: a STRK20 viewing key is one signature over a public
//! scope hash, so signing a supplied hash would be a key oracle.

use starknet_types_core::felt::Felt;

use super::sign::{DeviceSigning, DeviceStep, VerifiedSignature};
use super::{LedgerPath, RejectionCode, Reply, Version};

/// The device an account signs on: its derivation path, and the oldest Starknet app the wallet
/// accepts there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceKey {
    pub path: LedgerPath,
    pub app_minimum: Version,
}

/// What the caller does next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HashStep {
    /// Send these bytes to the device; pass its answer to [`HashSigning::on_reply`].
    Send(Vec<u8>),
    /// Done: the signature, verified over the hash under the account's key.
    Signed(VerifiedSignature),
    /// Parked with nothing signed: the device refused for `RejectionCode`. Once the person has
    /// fixed the device, [`HashSigning::retry`] starts again.
    Retry(RejectionCode),
    /// Ended with nothing signed.
    Failed(HashFailure),
}

/// Why a signature ended with nothing signed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashFailure {
    /// The session ended.
    Locked,
    /// The device's signature did not verify (`signature-verify-failed`), or a step came out of
    /// order (`unexpected-device-error`).
    Refused(RejectionCode),
}

#[derive(Debug)]
enum State {
    Exchange(DeviceSigning),
    Parked,
    Ended,
}

/// One hash signed on one account's device. Every step after an end fails.
#[derive(Debug)]
pub struct HashSigning {
    device: DeviceKey,
    hash: Felt,
    public_key: Felt,
    state: State,
}

impl HashSigning {
    /// Starts signing `hash` on `device` for the account whose stored key is `public_key`.
    /// `session_current` says whether the session that authorised it is still the live one.
    pub fn start(
        device: DeviceKey,
        hash: Felt,
        public_key: Felt,
        session_current: &dyn Fn() -> bool,
    ) -> (Self, HashStep) {
        let mut signing = Self {
            device,
            hash,
            public_key,
            state: State::Ended,
        };
        let step = signing.begin(session_current);
        (signing, step)
    }

    /// The hash being signed.
    pub fn hash(&self) -> Felt {
        self.hash
    }

    /// Feeds the device's answer to the last APDU sent.
    pub fn on_reply(&mut self, reply: Reply<'_>, session_current: &dyn Fn() -> bool) -> HashStep {
        let State::Exchange(signing) = &mut self.state else {
            return self.end(HashFailure::Refused(RejectionCode::UnexpectedDeviceError));
        };
        if !session_current() {
            return self.end(HashFailure::Locked);
        }
        let step = signing.on_reply(reply);
        self.judge(step, session_current)
    }

    /// Signs the same hash again from the start, after a [`HashStep::Retry`].
    pub fn retry(&mut self, session_current: &dyn Fn() -> bool) -> HashStep {
        if !matches!(self.state, State::Parked) {
            return self.end(HashFailure::Refused(RejectionCode::UnexpectedDeviceError));
        }
        self.begin(session_current)
    }

    fn begin(&mut self, session_current: &dyn Fn() -> bool) -> HashStep {
        if !session_current() {
            return self.end(HashFailure::Locked);
        }
        let (signing, step) = DeviceSigning::start(
            self.device.path,
            self.hash,
            self.public_key,
            self.device.app_minimum,
        );
        self.state = State::Exchange(signing);
        self.judge(step, session_current)
    }

    fn judge(&mut self, step: DeviceStep, session_current: &dyn Fn() -> bool) -> HashStep {
        match step {
            DeviceStep::Send(apdu) => HashStep::Send(apdu),
            DeviceStep::Signed(_) if !session_current() => self.end(HashFailure::Locked),
            DeviceStep::Signed(signature) => {
                self.state = State::Ended;
                HashStep::Signed(signature)
            }
            DeviceStep::Refused(rejection)
                if rejection.code == RejectionCode::SignatureVerifyFailed =>
            {
                self.end(HashFailure::Refused(rejection.code))
            }
            DeviceStep::Refused(rejection) => {
                self.state = State::Parked;
                HashStep::Retry(rejection.code)
            }
        }
    }

    fn end(&mut self, failure: HashFailure) -> HashStep {
        self.state = State::Ended;
        HashStep::Failed(failure)
    }
}
