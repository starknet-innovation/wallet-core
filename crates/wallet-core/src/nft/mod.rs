//! NFTs (P17): the extension's collectibles (`packages/features/portfolio/src/collectibles/`,
//! `apps/extension/src/background/portfolio/nft-*`, `provider/watch-nft.ts`), sans-IO.
//! Discovery, the on-chain re-proof, metadata, the two transfers and the watched list land here
//! one PR at a time; Bramble's numbers are `bramble-core`'s `NFT_POLICY`.

/// A collection's token standard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NftStandard {
    Erc721,
    Erc1155,
}

pub mod watched;

pub use watched::{NftTarget, WatchedNft, WatchedNfts};
