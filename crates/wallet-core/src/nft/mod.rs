//! NFTs (P17): the extension's collectibles (`packages/features/portfolio/src/collectibles/`,
//! `apps/extension/src/background/portfolio/nft-*`, `provider/watch-nft.ts`), sans-IO.
//! Discovery, the on-chain re-proof, metadata, the two transfers and the watched list land here
//! one PR at a time; Bramble's numbers are `bramble-core`'s `nft::NFT_POLICY`.

pub mod discovery;
mod indexer;
mod starkscan;

use crate::send::u256::U256;
use crate::Felt;

/// A collection's token standard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NftStandard {
    Erc721,
    Erc1155,
}

/// One token: its collection's contract and its id. The extension keys an NFT as
/// `${networkId}:${contract}:${tokenId}` (`nftId`): the network is the caller's, and the
/// standard is not part of the identity, since discovery cannot know it (the re-proof detects
/// it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NftId {
    pub contract: Felt,
    pub token_id: U256,
}

/// A wallet's NFT numbers (Bramble's: `bramble-core`'s `nft::NFT_POLICY`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NftPolicy {
    /// The Starkscan API base (`https://…/v1`) positions are read from. The extension reads
    /// them over its Activity client, so this is that same base.
    pub starkscan_api_base: &'static str,
    /// Positions asked for in the one Starkscan page read (`STARKSCAN_NFT_PAGE_SIZE`; Starkscan
    /// takes 1 to 100).
    pub starkscan_page_size: usize,
    /// Candidates one discovery may return (`MAX_DISCOVERED_NFTS`): each costs a re-proof.
    pub max_discovered: usize,
    /// A set indexer's reply budget (`HTTP_BODY_LIMITS.nftIndexer`).
    pub indexer_body_limit: u64,
    /// A set indexer's deadline for the whole exchange (`HTTP_REQUEST_TIMEOUT_MS`).
    pub indexer_deadline_ms: u64,
}

/// A policy for this crate's unit tests, at the extension's numbers; `bramble-core` sets
/// Bramble's.
#[cfg(test)]
pub(crate) const TEST_POLICY: NftPolicy = NftPolicy {
    starkscan_api_base: "https://starkscan.test/v1",
    starkscan_page_size: 100,
    max_discovered: 100,
    indexer_body_limit: 4 * 1024 * 1024,
    indexer_deadline_ms: 15_000,
};
