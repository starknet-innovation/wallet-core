//! Watched NFTs and the gallery's switches on mobile (`wallet_core::nft::watched`): the
//! extension's `nfts`, `nftIndexerUrl` and `collectiblesPreview` preferences, in their own file
//! beside the vault (never in it). A watch is stored only from the core's own finished
//! `NftOwnershipRead`, never from the host's word that the account owns the token
//! (`watchOwnedNft`). Every read-modify-write runs under one process-wide lock.

use std::path::Path;
use std::sync::{Mutex, PoisonError};

use wallet_core::nft::discovery::{indexer_url_setting, IndexerUrlSetting};
pub use wallet_core::nft::WatchedNft;
use wallet_core::nft::{NftTarget, WatchedNfts};

use crate::ffi_nft::NftError;
use crate::ffi_nft_read::NftOwnershipRead;
use crate::files::Directory;
use crate::networks::NetworkId;
use crate::rpc_override::is_rpc_url;
use crate::send::wallet_ext::Use;
use crate::wallet::WalletCore;

/// The document's file in the wallet directory. It names watched contracts and their owners, so
/// deleting the wallet removes it.
pub(crate) const NFT_FILE: &str = "nfts.v1.json";

/// Serialises every read-modify-write of the file within the process.
static FILE_LOCK: Mutex<()> = Mutex::new(());

/// Runs `f` over the document in `dir` under the process-wide lock, the one way any code here
/// reaches it.
pub(crate) fn with_watched<T>(dir: &Path, f: impl FnOnce(&WatchedNfts<'_>) -> T) -> T {
    let _guard = FILE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let storage = Directory(dir);
    f(&WatchedNfts::new(&storage, NFT_FILE))
}

/// A watched NFT, spelled as stored: `id` is the extension's `nftId`.
#[uniffi::remote(Record)]
pub struct WatchedNft {
    pub id: String,
    pub contract: String,
    pub network: NetworkId,
    pub token_id: String,
    pub label: Option<String>,
    pub owner: Option<String>,
    pub spam: Option<bool>,
}

#[uniffi::export]
impl WalletCore {
    /// Every watched NFT, in stored order (`wallet.loadWatchedNfts`), spam-hidden ones included
    /// so the gallery can offer to unhide them.
    pub fn watched_nfts(&self) -> Result<Vec<WatchedNft>, NftError> {
        Ok(with_watched(&self.data_dir, |nfts| nfts.list())?)
    }

    /// `account_id`'s watched NFTs on its network that are not hidden as spam
    /// (`watchedCandidates`): each still needs its `NftOwnershipRead` before it is shown.
    pub fn watched_nft_candidates(&self, account_id: String) -> Result<Vec<WatchedNft>, NftError> {
        let acting = self.plan_account_for(&account_id, Use::Read)?;
        let (network, owner) = (acting.network, acting.account.address);
        Ok(with_watched(&self.data_dir, |nfts| {
            nfts.gallery(network, &owner)
        })?)
    }

    /// `watchOwnedNft` from `read`, once it has finished on the account's network: refused
    /// unless it found the account holding the token. `label` is the watch form's (1 to 64
    /// characters once trimmed). Replaces any record of the same id, its spam flag included.
    /// Hiding a discovered NFT as spam is this, then `set_nft_spam`.
    pub fn watch_nft(
        &self,
        read: std::sync::Arc<NftOwnershipRead>,
        label: Option<String>,
    ) -> Result<WatchedNft, NftError> {
        let acting = self.plan_account_for(&read.account_id, Use::Read)?;
        let target = NftTarget {
            label,
            ..read.target.clone()
        };
        let owned = read.owned().is_some();
        Ok(with_watched(&self.data_dir, |nfts| {
            nfts.watch_owned(&target, acting.network, &read.address, owned)
        })?)
    }

    /// `removeNft`: an unknown id changes nothing.
    pub fn remove_nft(&self, id: String) -> Result<(), NftError> {
        Ok(with_watched(&self.data_dir, |nfts| nfts.remove(&id))?)
    }

    /// `setNftSpam`: hide as spam, or unhide; an unknown id changes nothing.
    pub fn set_nft_spam(&self, id: String, spam: bool) -> Result<(), NftError> {
        Ok(with_watched(&self.data_dir, |nfts| {
            nfts.set_spam(&id, spam)
        })?)
    }

    /// The person's NFT indexer endpoint, if set. It may carry an API key: never log it.
    pub fn nft_indexer_url(&self) -> Result<Option<String>, NftError> {
        Ok(with_watched(&self.data_dir, |nfts| nfts.indexer_url())?)
    }

    /// `setNftIndexerUrl`: empty after trimming clears it (Starkscan runs again); anything else
    /// must be an endpoint mobile may store (`https` only, as for an RPC override), else
    /// "Invalid indexer endpoint".
    pub fn set_nft_indexer_url(&self, url: String) -> Result<(), NftError> {
        let url = match indexer_url_setting(&url) {
            Some(IndexerUrlSetting::Clear) => String::new(),
            Some(IndexerUrlSetting::Set(url)) if is_rpc_url(&url) => url,
            _ => return Err(NftError::refused("Invalid indexer endpoint")),
        };
        Ok(with_watched(&self.data_dir, |nfts| {
            nfts.set_indexer_url(&url)
        })?)
    }

    /// `nftsShown`: on unless the person turned NFTs off. Off, discovery sends nothing.
    pub fn nfts_shown(&self) -> Result<bool, NftError> {
        Ok(with_watched(&self.data_dir, |nfts| nfts.shown())?)
    }

    /// The display switch (`collectiblesPreview`).
    pub fn set_nfts_shown(&self, shown: bool) -> Result<(), NftError> {
        Ok(with_watched(&self.data_dir, |nfts| nfts.set_shown(shown))?)
    }
}
