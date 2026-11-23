//! Sans-IO authority read for shared accounts (the extension's `getArgentMultisigConfig` at one
//! pinned block): the read, its bodies and its judging are
//! `wallet_core::accounts::multisig_read`; this module holds one behind a mutex for UniFFI and
//! passes Bramble's pinned class.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use wallet_core::accounts::multisig_read;
use wallet_core::send::judge;
use wallet_core::Felt;

use crate::networks::NetworkId;
use crate::rpc_request::RpcRequest;
use crate::{felt, multisig_constants, CoreError};

pub use wallet_core::accounts::multisig_read::MultisigAuthorityStatus;

/// Outcome of one authority read, as the extension's `ArgentMultisigAuthorityStatus` minus
/// `unsupported-signers`, which needs `SignerLinked` event discovery (deferred).
#[uniffi::remote(Enum)]
pub enum MultisigAuthorityStatus {
    Available,
    Undeployed,
    UnsupportedClass,
    InvalidConfig,
    NetworkUnavailable,
}

/// Reads one address's class, threshold and signer GUIDs at one pinned block.
#[derive(uniffi::Object)]
pub struct MultisigAuthorityRead {
    read: Mutex<multisig_read::MultisigAuthorityRead>,
}

impl MultisigAuthorityRead {
    /// Starts a read for a canonical, non-zero address.
    pub fn start(network: NetworkId, address: &str) -> Result<Self, CoreError> {
        let read = multisig_read::MultisigAuthorityRead::new(
            network,
            address,
            multisig_constants::argent_multisig_class(),
        )?;
        Ok(Self {
            read: Mutex::new(read),
        })
    }

    fn read(&self) -> MutexGuard<'_, multisig_read::MultisigAuthorityRead> {
        self.read.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The network this read targets.
    pub fn network(&self) -> NetworkId {
        self.read().network()
    }

    /// The canonical address this read targets.
    pub fn address(&self) -> String {
        self.read().address()
    }

    /// Threshold and ascending signer GUIDs, once the status is `Available`.
    pub fn authority(&self) -> Option<(u32, Vec<Felt>)> {
        self.read().authority()
    }
}

#[uniffi::export]
impl MultisigAuthorityRead {
    /// Starts a read of `address` on `network`; the address must be a non-zero felt.
    #[uniffi::constructor]
    pub fn new(network: NetworkId, address: String) -> Result<Arc<Self>, CoreError> {
        Self::start(network, &address).map(Arc::new)
    }

    /// The next request to send, or `None` once the status is decided.
    pub fn next_request(&self) -> Option<RpcRequest> {
        self.read().next_request()
    }

    /// Judges the response to the pending request. A body for any other id is refused and changes
    /// nothing; a malformed or failed answer settles `NetworkUnavailable`.
    pub fn accept_response(&self, id: u64, body: String) -> Result<(), CoreError> {
        Ok(self.read().accept_response(id, &body)?)
    }

    /// The host could not complete the pending request (timeout, TLS, HTTP status).
    pub fn transport_failed(&self, id: u64) {
        self.read().transport_failed(id);
    }

    /// The decided status, once there is one.
    pub fn status(&self) -> Option<MultisigAuthorityStatus> {
        self.read().status()
    }
}

/// The id of the nonce request, past any the authority read can use (it re-pins at most three
/// times, so it numbers a handful).
const NONCE_ID: u64 = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Nonce {
    Unasked,
    Pending,
    Read(Option<Felt>),
}

/// What a stored intent is checked against before it is signed, submitted or imported (the
/// extension's `currentArgentIntentContext`): the account's authority at one pinned block, then
/// its `starknet_getNonce`, both of the one address this read was started for.
#[derive(uniffi::Object)]
pub struct MultisigStateRead {
    authority: Mutex<multisig_read::MultisigAuthorityRead>,
    nonce: Mutex<Nonce>,
}

impl MultisigStateRead {
    fn guards(
        &self,
    ) -> (
        MutexGuard<'_, multisig_read::MultisigAuthorityRead>,
        MutexGuard<'_, Nonce>,
    ) {
        (
            self.authority
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
            self.nonce.lock().unwrap_or_else(PoisonError::into_inner),
        )
    }

    /// The finished authority read and the nonce, once both were read; `None` before then or
    /// when either failed.
    pub(crate) fn finished(&self) -> Option<(multisig_read::MultisigAuthorityRead, Felt)> {
        let (authority, nonce) = self.guards();
        match *nonce {
            Nonce::Read(Some(value)) => Some((authority.clone(), value)),
            _ => None,
        }
    }
}

#[uniffi::export]
impl MultisigStateRead {
    /// Starts a read of the multisig at `address` on `network`; the address must be non-zero.
    #[uniffi::constructor]
    pub fn new(network: NetworkId, address: String) -> Result<Arc<Self>, CoreError> {
        let read = MultisigAuthorityRead::start(network, &address)?;
        Ok(Arc::new(Self {
            authority: read.read,
            nonce: Mutex::new(Nonce::Unasked),
        }))
    }

    /// The next request to send, or `None` once the read is over (`status` says how).
    pub fn next_request(&self) -> Option<RpcRequest> {
        let (mut authority, mut nonce) = self.guards();
        if let Some(request) = authority.next_request() {
            return Some(request);
        }
        if authority.status() != Some(MultisigAuthorityStatus::Available) {
            return None;
        }
        match *nonce {
            Nonce::Read(_) => None,
            _ => {
                *nonce = Nonce::Pending;
                let address = felt::parse(&authority.address())?;
                Some(RpcRequest::new(
                    NONCE_ID,
                    wallet_core::send::rpc_bodies::nonce(NONCE_ID, &address),
                ))
            }
        }
    }

    /// Judges the response to the pending request. A body for any other id is refused and changes
    /// nothing. A nonce that is not one felt (a node error included) ends the read unavailable.
    pub fn accept_response(&self, id: u64, body: String) -> Result<(), CoreError> {
        let (mut authority, mut nonce) = self.guards();
        if *nonce != Nonce::Pending {
            return Ok(authority.accept_response(id, &body)?);
        }
        if id != NONCE_ID {
            return Err(CoreError::Rpc {
                reason: "response does not answer this request".into(),
            });
        }
        let value = match judge::answer(&body, id) {
            Some(judge::Answer::Result(serde_json::Value::String(text))) => felt::parse(&text),
            _ => None,
        };
        *nonce = Nonce::Read(value);
        Ok(())
    }

    /// The host could not complete the pending request.
    pub fn transport_failed(&self, id: u64) {
        let (mut authority, mut nonce) = self.guards();
        if *nonce == Nonce::Pending {
            if id == NONCE_ID {
                *nonce = Nonce::Read(None);
            }
        } else {
            authority.transport_failed(id);
        }
    }

    /// The authority status once decided; `NetworkUnavailable` when the nonce could not be read.
    /// `None` while a request is still owed.
    pub fn status(&self) -> Option<MultisigAuthorityStatus> {
        let (authority, nonce) = self.guards();
        match (authority.status()?, *nonce) {
            (MultisigAuthorityStatus::Available, Nonce::Read(Some(_))) => {
                Some(MultisigAuthorityStatus::Available)
            }
            (MultisigAuthorityStatus::Available, Nonce::Read(None)) => {
                Some(MultisigAuthorityStatus::NetworkUnavailable)
            }
            (MultisigAuthorityStatus::Available, _) => None,
            (other, _) => Some(other),
        }
    }
}
