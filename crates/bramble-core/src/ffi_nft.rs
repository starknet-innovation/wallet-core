//! NFTs on mobile (P17, `wallet_core::nft`): discovery through Starkscan or the person's indexer
//! (`wallet.discoverNfts`'s source), the on-chain re-proof of one token (`getNftOwnership`), and
//! its metadata (`getNftTokenUri`, then `resolveNftMetadataUri`), as sans-IO HTTP and RPC steps.
//! The reads are `ffi_nft_read`, the watched list `ffi_nft_watched`, the send `ffi_nft_send`.
//!
//! Nothing a source lists is shown before the chain re-proves it: the host runs an
//! `NftOwnershipRead` per candidate and drops what the account does not hold. Metadata is read
//! for watched NFTs only, as the extension reads it (`enrichWatchedNfts`). Images only: an image
//! is only ever what `media::normalize_image_url` emits, and the metadata's HTML animation never
//! crosses the FFI. Public on-chain data only.

use std::sync::Arc;

pub use wallet_core::nft::discovery::NftCoverage;
use wallet_core::nft::discovery::{DiscoveryInput, DiscoveryStart, NftCandidate, PendingDiscovery};
use wallet_core::nft::media::normalize_image_url;
pub use wallet_core::nft::metadata::NftAttribute;
use wallet_core::nft::metadata::{metadata_fetch_judge, NftTokenMetadata};
use wallet_core::nft::transfer::NftTransferRefusal;
pub use wallet_core::nft::NftStandard;

use crate::networks::NetworkId;
use crate::nft::{NFT_METADATA_LIMITS, NFT_POLICY};
use crate::portfolio::tokens::TokenPreferences;
use crate::portfolio::wire::{HttpOutcome, HttpRequest};
use crate::send::types::SendError;
use crate::send::wallet_ext::Use;
use crate::wallet::WalletCore;
use crate::{felt, ffi_nft_watched};

/// Why an NFT request was refused. `Refused` carries the core's fixed sentence; none carries an
/// address, a URL or node text. The apps read it through `Display`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
#[uniffi::export(Display)]
pub enum NftError {
    #[error("wallet is locked")]
    Locked,
    #[error("unknown account")]
    UnknownAccount,
    /// The NFT preferences, allowance log, contacts or multisig file could not be read.
    #[error("a wallet file could not be read")]
    Storage,
    #[error("{reason}")]
    Refused { reason: String },
}

impl NftError {
    pub(crate) fn refused(reason: &str) -> Self {
        Self::Refused {
            reason: reason.into(),
        }
    }
}

impl From<SendError> for NftError {
    fn from(error: SendError) -> Self {
        match error {
            SendError::Locked => Self::Locked,
            SendError::UnknownAccount => Self::UnknownAccount,
            // `trusted_recipients` fails closed on an unreadable contacts or multisig file.
            _ => Self::Storage,
        }
    }
}

impl From<NftTransferRefusal> for NftError {
    fn from(refusal: NftTransferRefusal) -> Self {
        Self::Refused {
            reason: refusal.to_string(),
        }
    }
}

/// The core's errors here are its fixed sentences ("Invalid NFT", "this account does not own
/// that NFT"), or a storage failure.
impl From<wallet_core::Error> for NftError {
    fn from(error: wallet_core::Error) -> Self {
        match error {
            wallet_core::Error::InvalidInput { reason } => Self::Refused { reason },
            _ => Self::Storage,
        }
    }
}

/// A collection's token standard, as the re-proof found it.
#[uniffi::remote(Enum)]
pub enum NftStandard {
    Erc721,
    Erc1155,
}

/// What one discovery run can say about its candidates: an empty list is "no NFTs" only when
/// `Complete`.
#[uniffi::remote(Enum)]
pub enum NftCoverage {
    Complete,
    Incomplete,
    None,
    Unavailable,
}

/// One untrusted candidate: shown only once its `NftOwnershipRead` says the account holds it.
/// `id` is the extension's `nftId`; `image` is `""` or a normalised image.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct NftCandidateView {
    pub id: String,
    pub contract: String,
    pub network: NetworkId,
    pub token_id: String,
    pub name: String,
    pub collection: String,
    pub image: String,
}

/// One discovery run's candidates and coverage.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct NftDiscoveryView {
    pub coverage: NftCoverage,
    pub nfts: Vec<NftCandidateView>,
}

fn candidate(nft: NftCandidate, network: NetworkId) -> NftCandidateView {
    let limits = &NFT_METADATA_LIMITS;
    let contract = felt::canonical(&nft.id.contract);
    let token_id = nft.id.token_id.to_decimal();
    NftCandidateView {
        id: format!("{}:{contract}:{token_id}", network.catalog_id()),
        contract,
        network,
        token_id,
        name: nft.name,
        collection: nft.collection,
        // The source's `image_url` is untrusted: only the metadata image rule's output is shown.
        image: normalize_image_url(&nft.image, limits.url_length, limits.art_data_uri_length),
    }
}

/// The first step of a discovery run: settled without the network (NFTs off, no source, a
/// refused indexer URL), or one request for the host to perform and hand to `judge`.
#[derive(uniffi::Enum)]
pub enum NftDiscoveryStart {
    Settled {
        discovery: NftDiscoveryView,
    },
    Send {
        request: HttpRequest,
        pending: Arc<NftDiscoveryRequest>,
    },
}

/// A discovery request in flight, bound to the source and owner it asked.
#[derive(uniffi::Object)]
pub struct NftDiscoveryRequest {
    network: NetworkId,
    pending: PendingDiscovery,
}

#[uniffi::export]
impl NftDiscoveryRequest {
    /// Judges the host's outcome: a failed exchange, a status outside 2xx, an oversized or
    /// malformed body is `Unavailable`, never "no NFTs".
    pub fn judge(&self, outcome: HttpOutcome) -> NftDiscoveryView {
        let judged = self.pending.clone().judge(&outcome);
        NftDiscoveryView {
            coverage: judged.coverage,
            nfts: judged
                .nfts
                .into_iter()
                .map(|nft| candidate(nft, self.network))
                .collect(),
        }
    }
}

/// The re-proof's answer: the standard the contract answered as and the account's balance
/// (decimal; 0 or 1 for ERC-721). `owned` is the balance above zero.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct NftOwnershipView {
    pub standard: NftStandard,
    pub balance: String,
    pub owned: bool,
}

/// `NftTokenMetadata` as mobile shows it: display-safe text, an image that is `""`, a public
/// `https` URL or a safe image `data:` URI, and `""` or a public `https` external URL.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct NftMetadata {
    pub name: String,
    pub collection: String,
    pub description: String,
    pub image: String,
    pub external_url: String,
    pub attributes: Vec<NftAttribute>,
}

/// One trait of a token.
#[uniffi::remote(Record)]
pub struct NftAttribute {
    pub display_type: Option<String>,
    pub trait_type: String,
    pub value: String,
}

/// `None` when the document held nothing mobile shows.
pub(crate) fn shown(metadata: NftTokenMetadata) -> Option<NftMetadata> {
    // No animation crosses: mobile has no sandbox to run an HTML document in (docs/PLAN.md P17).
    let NftTokenMetadata {
        name,
        collection,
        description,
        image,
        animation: _,
        external_url,
        attributes,
    } = metadata;
    let metadata = NftMetadata {
        name,
        collection,
        description,
        image,
        external_url,
        attributes,
    };
    let empty = metadata.name.is_empty()
        && metadata.collection.is_empty()
        && metadata.description.is_empty()
        && metadata.image.is_empty()
        && metadata.external_url.is_empty()
        && metadata.attributes.is_empty();
    (!empty).then_some(metadata)
}

/// Where a token's metadata comes from: decided without a request (`None`: art unavailable), or
/// one GET for the host to perform exactly as described and hand to `nft_metadata_judge`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum NftMetadataSource {
    Resolved { metadata: Option<NftMetadata> },
    Fetch { request: HttpRequest },
}

/// Judges a metadata fetch: `None` (art unavailable) unless the status is 2xx and the whole
/// body arrived within the bound and reads as a metadata document.
#[uniffi::export]
pub fn nft_metadata_judge(outcome: HttpOutcome) -> Option<NftMetadata> {
    metadata_fetch_judge(&outcome, &NFT_METADATA_LIMITS).and_then(shown)
}

#[uniffi::export]
impl WalletCore {
    /// Starts discovery for `account_id` on its network (`listOwnedNfts`): the person's saved
    /// indexer replaces Starkscan; without one, Starkscan runs on Mainnet when the person's
    /// explorer choice allows it; with NFTs turned off nothing is sent.
    pub fn nft_discovery(
        &self,
        account_id: String,
        tokens: Arc<TokenPreferences>,
    ) -> Result<NftDiscoveryStart, NftError> {
        let acting = self.plan_account_for(&account_id, Use::Read)?;
        let network = acting.network;
        let (indexer_url, nfts_shown) = ffi_nft_watched::with_watched(&self.data_dir, |nfts| {
            Ok::<_, wallet_core::Error>((nfts.indexer_url()?, nfts.shown()?))
        })?;
        let owner = felt::canonical(&acting.account.address);
        let input = DiscoveryInput {
            owner: &owner,
            network,
            indexer_url: indexer_url.as_deref(),
            nfts_shown,
            explorer_allowed: self.explorer_fallback_allowed(network, tokens),
        };
        Ok(match PendingDiscovery::start(&input, &NFT_POLICY) {
            DiscoveryStart::Settled(settled) => NftDiscoveryStart::Settled {
                discovery: NftDiscoveryView {
                    coverage: settled.coverage,
                    nfts: Vec::new(),
                },
            },
            DiscoveryStart::Send(pending, request) => NftDiscoveryStart::Send {
                request,
                pending: Arc::new(NftDiscoveryRequest { network, pending }),
            },
        })
    }
}
