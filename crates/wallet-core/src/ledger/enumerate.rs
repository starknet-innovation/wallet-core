//! Reading a page of public keys off a device for an account picker, sans-IO: the extension's
//! `enumerateLedgerPubKeys` (`onboard-enumerate.ts`). The Starknet app's version against the
//! wallet's minimum, then each path's key, read silently (nothing shows on the device), one APDU
//! at a time. Public keys only: nothing is signed, and no key is derived here.
//!
//! As in `sign`, the device OS gate is the host's connect flow, not this exchange. A key read here
//! is what the device reports; every later signature at that path is still checked against it
//! (`sign::DeviceSigning`).

use starknet_types_core::felt::Felt;

use super::rejection::{LedgerRejection, RejectionCode};
use super::starknet::{
    app_version_request, check_app_version, parse_app_version, parse_public_key, public_key_request,
};
use super::{LedgerPath, Reply, Version};

/// The most keys one page reads (`MAX_LEDGER_ACCOUNT_BATCH`).
pub const MAX_PAGE: usize = 5;

/// One key the device reported, at the page entry's `index`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceKeyAt {
    pub index: u32,
    pub public_key_x: Felt,
}

/// What the host does next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnumerationStep {
    /// Send these bytes to the device and pass its reply, or the transport's failure, to
    /// [`KeyEnumeration::on_reply`].
    Send(Vec<u8>),
    /// Done: every page entry's key, in the page's order.
    Keys(Vec<DeviceKeyAt>),
    /// Done: the device refused; no key is returned.
    Refused(LedgerRejection),
}

/// One page of keys being read. Every step after an end is refused.
#[derive(Debug)]
pub struct KeyEnumeration {
    page: Vec<(u32, LedgerPath)>,
    app_minimum: Version,
    keys: Vec<DeviceKeyAt>,
    /// Whether the app version has been read and passed.
    ready: bool,
    ended: bool,
}

impl KeyEnumeration {
    /// Starts reading the key at each `(index, path)` of `page`, which holds 1 to [`MAX_PAGE`]
    /// entries. Returns the exchange and its first step.
    pub fn start(page: Vec<(u32, LedgerPath)>, app_minimum: Version) -> (Self, EnumerationStep) {
        let mut enumeration = Self {
            app_minimum,
            keys: Vec::with_capacity(page.len()),
            page,
            ready: false,
            ended: false,
        };
        let step = if (1..=MAX_PAGE).contains(&enumeration.page.len()) {
            EnumerationStep::Send(app_version_request())
        } else {
            enumeration.refuse(LedgerRejection::new(RejectionCode::UnexpectedDeviceError))
        };
        (enumeration, step)
    }

    /// Feeds the device's reply to the last APDU sent.
    pub fn on_reply(&mut self, reply: Reply<'_>) -> EnumerationStep {
        match self.advance(reply) {
            Ok(step) => step,
            Err(rejection) => self.refuse(rejection),
        }
    }

    fn advance(&mut self, reply: Reply<'_>) -> Result<EnumerationStep, LedgerRejection> {
        if self.ended {
            return Err(LedgerRejection::new(RejectionCode::UnexpectedDeviceError));
        }
        if self.ready {
            let index = self.page.get(self.keys.len()).map(|(index, _)| *index);
            let index =
                index.ok_or_else(|| LedgerRejection::new(RejectionCode::UnexpectedDeviceError))?;
            let public_key_x = parse_public_key(reply)?;
            self.keys.push(DeviceKeyAt {
                index,
                public_key_x,
            });
        } else {
            check_app_version(parse_app_version(reply)?, self.app_minimum)?;
            self.ready = true;
        }
        Ok(match self.page.get(self.keys.len()) {
            Some((_, path)) => EnumerationStep::Send(public_key_request(path)),
            None => {
                self.ended = true;
                EnumerationStep::Keys(std::mem::take(&mut self.keys))
            }
        })
    }

    /// Ends the exchange: every later reply is refused and no key is returned.
    pub fn end(&mut self) {
        self.refuse(LedgerRejection::new(RejectionCode::UnexpectedDeviceError));
    }

    fn refuse(&mut self, rejection: LedgerRejection) -> EnumerationStep {
        self.ended = true;
        self.keys.clear();
        EnumerationStep::Refused(rejection)
    }
}
