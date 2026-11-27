//! The chain reads behind an NFT (`rpc-assets.ts`): the re-proof that an account holds a token
//! (`getNftOwnership`) and its metadata URI (`getNftTokenUri`), each over the extension's
//! entrypoint fallbacks, driven by the host as every chain read is (`next_calls`, `accept`,
//! `restart`).

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use wallet_core::history::events::ScanStep;
use wallet_core::nft::metadata::{resolve_metadata_uri, MetadataSource};
use wallet_core::nft::reproof::{NftOwnership, OwnershipRead};
use wallet_core::nft::token_uri::TokenUriRead;
use wallet_core::nft::transfer::parse_token_id;
use wallet_core::nft::{NftStandard, NftTarget, ReadStep};
use wallet_core::send::u256::U256;
use wallet_core::Felt;

use crate::felt;
use crate::ffi_nft::{shown, NftError, NftMetadataSource, NftOwnershipView};
use crate::nft::NFT_METADATA_LIMITS;
use crate::portfolio::wire::{RpcReply, RpcRequest};
use crate::send::wallet_ext::Use;
use crate::wallet::WalletCore;

/// A single-endpoint read over the extension's entrypoint fallbacks (`OwnershipRead`,
/// `TokenUriRead`).
trait Rungs {
    type Out;
    fn start(&mut self, id: u64) -> RpcRequest;
    fn judge(&mut self, reply: &RpcReply, next_id: u64) -> ReadStep<Self::Out>;
}

impl Rungs for OwnershipRead {
    type Out = NftOwnership;
    fn start(&mut self, id: u64) -> RpcRequest {
        OwnershipRead::start(self, id)
    }
    fn judge(&mut self, reply: &RpcReply, next_id: u64) -> ReadStep<NftOwnership> {
        OwnershipRead::judge(self, reply, next_id)
    }
}

impl Rungs for TokenUriRead {
    type Out = String;
    fn start(&mut self, id: u64) -> RpcRequest {
        TokenUriRead::start(self, id)
    }
    fn judge(&mut self, reply: &RpcReply, next_id: u64) -> ReadStep<String> {
        TokenUriRead::judge(self, reply, next_id)
    }
}

/// A `Rungs` read as the host drives every chain read: `next_calls`, `accept`, `restart`.
struct Steps<R: Rungs> {
    read: R,
    next_id: u64,
    outgoing: Option<RpcRequest>,
    waiting: bool,
    /// `Some(None)`: no entrypoint answered.
    outcome: Option<Option<R::Out>>,
}

impl<R: Rungs> Steps<R> {
    fn new(mut read: R) -> Self {
        let first = read.start(1);
        Self {
            read,
            next_id: 2,
            outgoing: Some(first),
            waiting: false,
            outcome: None,
        }
    }

    fn next_calls(&mut self) -> Vec<RpcRequest> {
        let calls: Vec<_> = self.outgoing.take().into_iter().collect();
        self.waiting |= !calls.is_empty();
        calls
    }

    fn accept(&mut self, reply: &RpcReply) -> ScanStep {
        if !std::mem::take(&mut self.waiting) {
            return ScanStep::Continue;
        }
        let id = self.next_id;
        self.next_id += 1;
        match self.read.judge(reply, id) {
            ReadStep::Request(request) => self.outgoing = Some(request),
            ReadStep::Done(out) => self.outcome = Some(Some(out)),
            ReadStep::Failed => self.outcome = Some(None),
            ReadStep::Transport => return ScanStep::Transport,
        }
        ScanStep::Continue
    }

    fn restart(&mut self) {
        self.outcome = None;
        self.waiting = false;
        self.outgoing = Some(self.read.start(self.next_id));
        self.next_id += 1;
    }

    /// The answer, once the read is done; `None` while unfinished or when it failed.
    fn finish(&self) -> Option<&R::Out> {
        self.outcome.as_ref().and_then(Option::as_ref)
    }
}

fn guard<T>(lock: &Mutex<T>) -> MutexGuard<'_, T> {
    lock.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One account's re-proof of one token on its network: `owner_of` / `ownerOf`, else ERC-1155
/// `balance_of` / `balanceOf`. Ask `next_calls` until it is empty, then `finish`. The same read
/// is what `watch_nft` stores a watch from, so a watch is never the host's word alone.
#[derive(uniffi::Object)]
pub struct NftOwnershipRead {
    pub(crate) account_id: String,
    pub(crate) address: Felt,
    pub(crate) target: NftTarget,
    read: Mutex<Steps<OwnershipRead>>,
}

impl NftOwnershipRead {
    /// The finished read's standard when the account holds the token; `None` otherwise.
    pub(crate) fn owned(&self) -> Option<NftStandard> {
        guard(&self.read).finish().and_then(NftOwnership::owned)
    }
}

#[uniffi::export]
impl NftOwnershipRead {
    /// The call to send now to an endpoint of the account's network; empty while waiting for
    /// its reply or when done.
    pub fn next_calls(&self) -> Vec<RpcRequest> {
        guard(&self.read).next_calls()
    }

    /// Judges one reply. `Transport`: `restart` on the next endpoint.
    pub fn accept(&self, reply: RpcReply) -> ScanStep {
        guard(&self.read).accept(&reply)
    }

    /// Discards the read so it replays on another endpoint.
    pub fn restart(&self) {
        guard(&self.read).restart();
    }

    /// The answer; `None` while calls remain or when no entrypoint answered ("Could not read NFT
    /// ownership": in discovery, the candidate is left out and the list marked incomplete).
    pub fn finish(&self) -> Option<NftOwnershipView> {
        guard(&self.read)
            .finish()
            .map(|ownership| NftOwnershipView {
                standard: ownership.standard,
                balance: ownership.balance.to_decimal(),
                owned: ownership.balance > U256::ZERO,
            })
    }
}

/// One token's metadata-URI read: `token_uri`, `tokenURI`, then ERC-1155 `uri`. Ask `next_calls`
/// until it is empty, then `finish`. The URI itself never reaches the host.
#[derive(uniffi::Object)]
pub struct NftTokenUriRead {
    read: Mutex<Steps<TokenUriRead>>,
}

#[uniffi::export]
impl NftTokenUriRead {
    /// A read of `token_id` (base 10) at `contract` (a non-zero lowercase felt).
    #[uniffi::constructor]
    pub fn new(contract: String, token_id: String) -> Result<Arc<Self>, NftError> {
        let (contract, token_id) = read_token(&contract, &token_id)?;
        let read = TokenUriRead::new(contract, token_id, NFT_METADATA_LIMITS.uri_bytes)?;
        Ok(Arc::new(Self {
            read: Mutex::new(Steps::new(read)),
        }))
    }

    /// The call to send now to an endpoint of the token's network; empty while waiting or done.
    pub fn next_calls(&self) -> Vec<RpcRequest> {
        guard(&self.read).next_calls()
    }

    /// Judges one reply. `Transport`: `restart` on the next endpoint.
    pub fn accept(&self, reply: RpcReply) -> ScanStep {
        guard(&self.read).accept(&reply)
    }

    /// Discards the read so it replays on another endpoint.
    pub fn restart(&self) {
        guard(&self.read).restart();
    }

    /// Where the metadata comes from. An unfinished or failed read, or a URI too long, malformed
    /// or not public `https`, `ipfs://` or a JSON `data:` URI, is `Resolved { metadata: None }`.
    pub fn finish(&self) -> NftMetadataSource {
        let Some(uri) = guard(&self.read).finish().cloned() else {
            return NftMetadataSource::Resolved { metadata: None };
        };
        match resolve_metadata_uri(&uri, &NFT_METADATA_LIMITS) {
            MetadataSource::Resolved(metadata) => NftMetadataSource::Resolved {
                metadata: metadata.and_then(shown),
            },
            MetadataSource::Fetch(request) => NftMetadataSource::Fetch { request },
        }
    }
}

/// `contract` (a non-zero felt in the strict lowercase grammar) and `token_id` (base 10, at most
/// 78 digits, within u256), as the extension's NFT inputs read them.
pub(crate) fn read_token(contract: &str, token_id: &str) -> Result<(Felt, U256), NftError> {
    let invalid = || NftError::refused("invalid nft request");
    let contract = felt::parse_non_zero(contract).ok_or_else(invalid)?;
    Ok((contract, parse_token_id(token_id).ok_or_else(invalid)?))
}

#[uniffi::export]
impl WalletCore {
    /// Starts the re-proof that `account_id` holds `token_id` (base 10) at `contract` (a
    /// non-zero lowercase felt) on the account's network.
    pub fn nft_ownership_read(
        &self,
        account_id: String,
        contract: String,
        token_id: String,
    ) -> Result<Arc<NftOwnershipRead>, NftError> {
        let acting = self.plan_account_for(&account_id, Use::Read)?;
        let (contract, token_id) = read_token(&contract, &token_id)?;
        let target = NftTarget {
            contract,
            network: acting.network,
            token_id,
            label: None,
        };
        let address = acting.account.address;
        let read = OwnershipRead::new(contract, address, token_id)?;
        Ok(Arc::new(NftOwnershipRead {
            account_id: acting.account_id,
            address,
            target,
            read: Mutex::new(Steps::new(read)),
        }))
    }
}
