//! Sans-IO authority read for shared accounts (the extension's `getArgentMultisigConfig` at one
//! pinned block): the read, its bodies and its judging are
//! `wallet_core::accounts::multisig_read`; this module holds one behind a mutex for UniFFI and
//! passes Bramble's pinned class.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use wallet_core::accounts::multisig_read;
use wallet_core::js::felt::same_felt;
use wallet_core::send::judge;
use wallet_core::send::multisig_stale::{nonce_moved_past, read_nonce, HashLookups};
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
/// The first id of an intent read's status lookups.
const LOOKUP_ID: u64 = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Nonce {
    Unasked,
    Pending,
    Read(Option<Felt>),
}

/// What a read made for one stored intent asks after the nonce (`WalletCore::
/// multisig_intent_read`): the signed hashes the extension's `currentArgentIntentContext` looks
/// up. When the nonce moved past the intent, its own hash (`nonceMismatch`); when it is still the
/// intent's, the hashes of its calls' stale versions (`refuseSigningResend`).
pub(crate) struct IntentLookups {
    pub intent_id: String,
    /// When the host started the read: the time a row journaled from it is filed at.
    pub now_ms: u64,
    pub prepared_nonce: String,
    /// The intent's own hash, when it is fully signed (only then can it have been sent).
    pub own: Option<Felt>,
    /// Its calls' stale versions that recovery reads (`needs_lookup`).
    pub stale: Vec<Felt>,
    pub lookups: Option<HashLookups>,
}

struct State {
    authority: multisig_read::MultisigAuthorityRead,
    nonce: Nonce,
    intent: Option<IntentLookups>,
}

impl State {
    /// The intent's lookups, decided once the nonce is read.
    fn lookups(&mut self) -> Option<&mut HashLookups> {
        let Nonce::Read(Some(nonce)) = self.nonce else {
            return None;
        };
        let intent = self.intent.as_mut()?;
        if intent.lookups.is_none() {
            let hashes = if !same_felt(&felt::canonical(&nonce), &intent.prepared_nonce) {
                match intent.own {
                    Some(own) if nonce_moved_past(&nonce, &intent.prepared_nonce) => vec![own],
                    _ => Vec::new(),
                }
            } else {
                intent.stale.clone()
            };
            intent.lookups = Some(HashLookups::new(LOOKUP_ID, hashes));
        }
        intent.lookups.as_mut()
    }

    fn looking_up(&mut self) -> bool {
        self.lookups().is_some_and(|lookups| !lookups.done())
    }
}

/// What a stored intent is checked against before it is signed, submitted or imported (the
/// extension's `currentArgentIntentContext`): the account's authority at one pinned block, then
/// its `starknet_getNonce`, both of the one address this read was started for; and, for a read
/// made for one intent, the status of the signed hashes that say whether it or an earlier
/// version of its calls was already sent.
#[derive(uniffi::Object)]
pub struct MultisigStateRead {
    state: Mutex<State>,
}

impl MultisigStateRead {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn start(
        network: NetworkId,
        address: &str,
        intent: Option<IntentLookups>,
    ) -> Result<Arc<Self>, CoreError> {
        let read = multisig_read::MultisigAuthorityRead::new(
            network,
            address,
            multisig_constants::argent_multisig_class(),
        )?;
        Ok(Arc::new(Self {
            state: Mutex::new(State {
                authority: read,
                nonce: Nonce::Unasked,
                intent,
            }),
        }))
    }

    /// A read of the multisig at `address` that also looks up what `intent` names.
    pub(crate) fn for_intent(
        network: NetworkId,
        address: &str,
        intent: IntentLookups,
    ) -> Result<Arc<Self>, CoreError> {
        Self::start(network, address, Some(intent))
    }

    /// The finished authority read and the nonce, once every request was answered and both
    /// were read; `None` before then or when either failed.
    pub(crate) fn finished(&self) -> Option<(multisig_read::MultisigAuthorityRead, Felt)> {
        let mut state = self.state();
        if state.looking_up() {
            return None;
        }
        match state.nonce {
            Nonce::Read(Some(value)) => Some((state.authority.clone(), value)),
            _ => None,
        }
    }

    /// The intent this read was made for and what its lookups found (`None` for a plain read).
    pub(crate) fn intent(&self) -> Option<(String, u64, HashLookups)> {
        let mut state = self.state();
        let lookups = state.lookups().cloned()?;
        let intent = state.intent.as_ref()?;
        Some((intent.intent_id.clone(), intent.now_ms, lookups))
    }
}

#[uniffi::export]
impl MultisigStateRead {
    /// Starts a read of the multisig at `address` on `network`; the address must be non-zero.
    /// For an import. A sign or submit takes `WalletCore::multisig_intent_read`'s instead.
    #[uniffi::constructor]
    pub fn new(network: NetworkId, address: String) -> Result<Arc<Self>, CoreError> {
        Self::start(network, &address, None)
    }

    /// The next request to send, or `None` once the read is over (`status` says how).
    pub fn next_request(&self) -> Option<RpcRequest> {
        let mut state = self.state();
        if let Some(request) = state.authority.next_request() {
            return Some(request);
        }
        if state.authority.status() != Some(MultisigAuthorityStatus::Available) {
            return None;
        }
        match state.nonce {
            Nonce::Read(None) => None,
            Nonce::Read(Some(_)) => {
                let (id, body) = state.lookups()?.next_request()?;
                Some(RpcRequest::new(id, body))
            }
            _ => {
                state.nonce = Nonce::Pending;
                let address = felt::parse(&state.authority.address())?;
                Some(RpcRequest::new(
                    NONCE_ID,
                    wallet_core::send::rpc_bodies::nonce(NONCE_ID, &address),
                ))
            }
        }
    }

    /// Judges the response to the pending request. A body for any other id is refused and changes
    /// nothing. A nonce that is not one felt (a node error included) ends the read unavailable;
    /// a status lookup's answer, whatever it is, is recorded as found, absent or unknown.
    pub fn accept_response(&self, id: u64, body: String) -> Result<(), CoreError> {
        let mut state = self.state();
        let stray = || CoreError::Rpc {
            reason: "response does not answer this request".into(),
        };
        if state.nonce == Nonce::Pending {
            if id != NONCE_ID {
                return Err(stray());
            }
            state.nonce = Nonce::Read(read_nonce(judge::answer(&body, id)));
            return Ok(());
        }
        if state.looking_up() {
            let accepted = state
                .lookups()
                .is_some_and(|lookups| lookups.accept(id, judge::answer(&body, id)));
            return if accepted { Ok(()) } else { Err(stray()) };
        }
        Ok(state.authority.accept_response(id, &body)?)
    }

    /// The host could not complete the pending request.
    pub fn transport_failed(&self, id: u64) {
        let mut state = self.state();
        if state.nonce == Nonce::Pending {
            if id == NONCE_ID {
                state.nonce = Nonce::Read(None);
            }
        } else if state.looking_up() {
            if let Some(lookups) = state.lookups() {
                lookups.accept(id, None);
            }
        } else {
            state.authority.transport_failed(id);
        }
    }

    /// The authority status once decided; `NetworkUnavailable` when the nonce could not be read.
    /// `None` while a request is still owed.
    pub fn status(&self) -> Option<MultisigAuthorityStatus> {
        let mut state = self.state();
        let looking_up = state.looking_up();
        match (state.authority.status()?, state.nonce) {
            (MultisigAuthorityStatus::Available, Nonce::Read(Some(_))) if !looking_up => {
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
