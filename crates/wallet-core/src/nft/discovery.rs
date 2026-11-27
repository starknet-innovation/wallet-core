//! NFT discovery: which source one run asks (`createHttpNftIndexer` in `nft-indexer.ts`, and
//! `handleDiscoverNfts`'s gate in `nft-ops.ts`), the request, and the judged candidates with
//! their coverage. Nothing here is trusted: the wallet re-proves every candidate on chain before
//! it shows one, and an outage or a gap is never read as "no NFTs".
//!
//! Departures from the extension, each failing closed where it trusts its caller: a saved
//! indexer URL that `is_custom_rpc_url` refuses, or an owner that is not a non-zero felt in the
//! extension's grammar, sends nothing and reports `Unavailable` (the extension trusts its stored
//! preferences and account); and an indexer token id past u256 is a record it could not read
//! (the extension keeps it, and its re-proof then fails).

use super::{indexer, starkscan, NftId, NftPolicy};
use crate::js::num::{js_trim, utf16_len};
use crate::nostr::relay_url::is_custom_rpc_url;
use crate::portfolio::wire::{HttpOutcome, HttpRequest};
use crate::NetworkId;

/// What one discovery run can say about its candidates (`NftCoverage`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NftCoverage {
    /// The source answered and claims its list is whole.
    Complete,
    /// The list may be missing NFTs: a partial index, more than one page or the cap, or a
    /// record the wallet could not read.
    Incomplete,
    /// No source ran: only watched NFTs show.
    None,
    /// The source failed or answered something malformed.
    Unavailable,
}

/// One untrusted candidate the source listed (`DiscoveredNft` as discovery fills it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NftCandidate {
    pub id: NftId,
    /// The source's token name, trimmed and at most 128 UTF-16 units; empty when absent.
    pub name: String,
    /// The source's collection name, trimmed and bounded; empty when absent or unsafe.
    pub collection: String,
    /// An indexer's `image_url` exactly as sent (at most 32,768 UTF-16 units, else empty). It is
    /// NOT yet normalised: nothing may display it before the metadata image rule (IPFS gateway,
    /// public `https`, safe `data:` images) has run over it.
    pub image: String,
}

/// One discovery run's candidates and coverage (`NftDiscovery`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NftDiscovery {
    pub coverage: NftCoverage,
    pub nfts: Vec<NftCandidate>,
}

impl NftDiscovery {
    pub(super) const fn empty(coverage: NftCoverage) -> Self {
        Self {
            coverage,
            nfts: Vec::new(),
        }
    }
}

/// What the wallet knows before discovery runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoveryInput<'a> {
    /// The account (or watched multisig) whose NFTs are listed, as the wallet stores it.
    pub owner: &'a str,
    pub network: NetworkId,
    /// The person's saved indexer URL (`nftIndexerUrl`), if any.
    pub indexer_url: Option<&'a str>,
    /// NFTs are turned on (`nftsShown`); off, nothing runs and no address leaves.
    pub nfts_shown: bool,
    /// The person's explorer choice lets Starkscan serve this network (the Activity lane's gate).
    pub explorer_allowed: bool,
}

/// The first step of a run: settled without the network, or one request to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryStart {
    Settled(NftDiscovery),
    Send(PendingDiscovery, HttpRequest),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Indexer,
    Starkscan,
}

/// A discovery request in flight, bound to the source and owner it asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingDiscovery {
    source: Source,
    owner: String,
    policy: NftPolicy,
}

impl PendingDiscovery {
    /// `listOwnedNfts`: a saved indexer URL is the person's own choice and replaces Starkscan;
    /// without one, Starkscan runs on Mainnet when the explorer is allowed; otherwise nothing
    /// is sent (`None`).
    pub fn start(input: &DiscoveryInput<'_>, policy: &NftPolicy) -> DiscoveryStart {
        let settled = |coverage| DiscoveryStart::Settled(NftDiscovery::empty(coverage));
        let owner_ok = crate::felt::parse_non_zero(input.owner).is_some();
        let (source, request) = match input.indexer_url {
            _ if !input.nfts_shown => return settled(NftCoverage::None),
            Some(url) if !owner_ok || !is_custom_rpc_url(url) => {
                return settled(NftCoverage::Unavailable)
            }
            Some(url) => (
                Source::Indexer,
                indexer::request(url, input.owner, input.network, policy),
            ),
            None if !input.explorer_allowed || input.network != NetworkId::Mainnet => {
                return settled(NftCoverage::None)
            }
            None => match starkscan::request(input.owner, policy) {
                Some(request) => (Source::Starkscan, request),
                None => return settled(NftCoverage::Unavailable),
            },
        };
        let pending = Self {
            source,
            owner: input.owner.to_string(),
            policy: *policy,
        };
        DiscoveryStart::Send(pending, request)
    }

    /// Judges the host's outcome: any failed exchange, status outside 2xx, oversized or
    /// malformed body is `Unavailable`.
    pub fn judge(self, outcome: &HttpOutcome) -> NftDiscovery {
        let judged = match self.source {
            Source::Indexer => indexer::judge(outcome, &self.policy),
            Source::Starkscan => starkscan::judge(outcome, &self.owner),
        };
        judged.unwrap_or(NftDiscovery::empty(NftCoverage::Unavailable))
    }
}

/// A valid answer to the indexer URL setting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexerUrlSetting {
    /// Empty after trimming: the saved URL is removed and Starkscan runs again.
    Clear,
    /// The trimmed URL to save.
    Set(String),
}

/// `setNftIndexerUrl` over `readSetNftIndexerUrlInput`: at most 2,048 UTF-16 units as typed,
/// then trimmed and checked by the custom RPC rule; `None` refuses it ("Invalid indexer
/// endpoint").
pub fn indexer_url_setting(typed: &str) -> Option<IndexerUrlSetting> {
    if utf16_len(typed) > 2048 {
        return None;
    }
    let trimmed = js_trim(typed);
    if trimmed.is_empty() {
        Some(IndexerUrlSetting::Clear)
    } else {
        is_custom_rpc_url(trimmed).then(|| IndexerUrlSetting::Set(trimmed.to_string()))
    }
}

/// `trimTrailingSlashes`.
pub(super) fn trim_trailing_slashes(value: &str) -> &str {
    value.trim_end_matches('/')
}

#[cfg(test)]
#[path = "discovery_tests.rs"]
mod tests;
