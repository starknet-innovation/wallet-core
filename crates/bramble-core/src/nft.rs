//! Bramble's NFT choices, for `starknet_wallet_core::nft`.

use starknet_wallet_core::nft::NftPolicy;

/// Starkscan over the Activity lane's base with `STARKSCAN_NFT_PAGE_SIZE` (`starkscan-nfts.ts`),
/// `MAX_DISCOVERED_NFTS` (`nft-indexer.ts`), and a set indexer's `HTTP_BODY_LIMITS.nftIndexer`
/// and `HTTP_REQUEST_TIMEOUT_MS` (`core/http-json.ts`).
pub const NFT_POLICY: NftPolicy = NftPolicy {
    starkscan_api_base: crate::HISTORY_POLICY.explorer_api_base,
    starkscan_page_size: 100,
    max_discovered: 100,
    indexer_body_limit: 4 * 1024 * 1024,
    indexer_deadline_ms: 15_000,
};
