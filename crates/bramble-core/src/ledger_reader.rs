//! The Ledger account picker's two halves (P8), the extension's onboarding
//! (`onboard-enumerate.ts`, then `wallet.deriveLedgerAccounts`):
//!
//! - `LedgerKeyReader` reads a page of public keys off the device: the host moves its APDUs
//!   (`wallet_core::ledger::KeyEnumeration`) and gets the keys back, read silently at the scheme's
//!   path for each index;
//! - `ledger_account_candidates` places each key's address under a family on a network, as the
//!   request `add_ledger_account` or `create_ledger_wallet` then takes.
//!
//! Public keys and addresses only: nothing here signs or touches the vault.

use std::sync::{Arc, Mutex, PoisonError};

use wallet_core::ledger::{EnumerationStep, KeyEnumeration, TransportFailure, MAX_PAGE};

use crate::account_catalog::AccountFamily;
use crate::ledger::{LedgerScheme, LEDGER_APP_MIN};
use crate::ledger_wallet::{record, LedgerAccountRequest};
use crate::networks::NetworkId;
use crate::{felt, CoreError};

/// One key the device reported.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct LedgerKey {
    pub account_index: u32,
    pub public_key_x: String,
}

/// What the host does next with a `LedgerKeyReader`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum LedgerReadStep {
    /// Send `apdu` to the device; pass its answer to `reply`, or what the transport reported to
    /// `transport_failure`.
    Send { apdu: Vec<u8> },
    /// Done: the page's keys, in index order.
    Keys { keys: Vec<LedgerKey> },
    /// Done: the device refused (`reason` is the extension's rejection code). No key is returned;
    /// a new reader starts again.
    Refused { reason: String },
}

fn step(step: EnumerationStep) -> LedgerReadStep {
    match step {
        EnumerationStep::Send(apdu) => LedgerReadStep::Send { apdu },
        EnumerationStep::Keys(keys) => LedgerReadStep::Keys {
            keys: keys
                .into_iter()
                .map(|key| LedgerKey {
                    account_index: key.index,
                    public_key_x: felt::canonical(&key.public_key_x),
                })
                .collect(),
        },
        EnumerationStep::Refused(rejection) => LedgerReadStep::Refused {
            reason: rejection.code.as_str().into(),
        },
    }
}

fn invalid(reason: &str) -> CoreError {
    CoreError::InvalidInput {
        reason: reason.into(),
    }
}

/// One page of public keys read off a device.
#[derive(uniffi::Object)]
pub struct LedgerKeyReader {
    enumeration: Mutex<KeyEnumeration>,
    first: Mutex<Option<LedgerReadStep>>,
}

#[uniffi::export]
impl LedgerKeyReader {
    /// Reads the keys at `scheme`'s path for `count` indexes from `start_index` (1 to 5, every
    /// index below 2^31), with Bramble's Starknet app minimum.
    #[uniffi::constructor]
    pub fn new(scheme: LedgerScheme, start_index: u32, count: u32) -> Result<Arc<Self>, CoreError> {
        let count = usize::try_from(count).map_err(|_| invalid("page size out of range"))?;
        if !(1..=MAX_PAGE).contains(&count) {
            return Err(invalid("page size out of range"));
        }
        let page = (0..count)
            .map(|offset| {
                let index = u32::try_from(offset)
                    .ok()
                    .and_then(|offset| start_index.checked_add(offset))?;
                Some((index, scheme.device(index)?.path))
            })
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| invalid("Ledger account index out of range"))?;
        let (enumeration, first) = KeyEnumeration::start(page, LEDGER_APP_MIN);
        Ok(Arc::new(Self {
            enumeration: Mutex::new(enumeration),
            first: Mutex::new(Some(step(first))),
        }))
    }

    /// The first step. Asked again, the exchange ends: that step and every later one is refused.
    pub fn start(&self) -> LedgerReadStep {
        let first = self
            .first
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        first.unwrap_or_else(|| {
            self.enumeration
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .end();
            LedgerReadStep::Refused {
                reason: "unexpected-device-error".into(),
            }
        })
    }

    /// The device's answer to the last APDU: its data, then the status word.
    pub fn reply(&self, reply: Vec<u8>) -> LedgerReadStep {
        self.advance(Ok(&reply))
    }

    /// What the transport reported in place of an answer.
    pub fn transport_failure(&self, failure: TransportFailure) -> LedgerReadStep {
        self.advance(Err(failure))
    }
}

impl LedgerKeyReader {
    fn advance(&self, reply: Result<&[u8], TransportFailure>) -> LedgerReadStep {
        // The first step is spent once the host answers anything.
        self.first
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let mut enumeration = self
            .enumeration
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        step(enumeration.on_reply(reply))
    }
}

/// One account the picker offers: the request that adds it, and the address it places.
#[derive(Debug, Clone, uniffi::Record)]
pub struct LedgerCandidate {
    pub request: LedgerAccountRequest,
    pub address: String,
}

/// `deriveLedgerAccounts`: each of up to five keys read at `scheme` placed as a `family` account
/// on `network`, at Bramble's class. Refuses Braavos, a key that is not a non-zero felt, and an
/// index of 2^31 or more, as `add_ledger_account` would.
#[uniffi::export]
pub fn ledger_account_candidates(
    network: NetworkId,
    family: AccountFamily,
    scheme: LedgerScheme,
    keys: Vec<LedgerKey>,
) -> Result<Vec<LedgerCandidate>, CoreError> {
    if keys.len() > MAX_PAGE {
        return Err(invalid("too many Ledger accounts at once"));
    }
    keys.into_iter()
        .map(|key| {
            let request = LedgerAccountRequest {
                network,
                family,
                scheme,
                account_index: key.account_index,
                public_key_x: key.public_key_x,
            };
            let address = record(&request)?.address;
            Ok(LedgerCandidate { request, address })
        })
        .collect()
}
