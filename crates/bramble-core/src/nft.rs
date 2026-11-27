//! Bramble's NFT choices, for `starknet_wallet_core::nft`.

use starknet_wallet_core::nft::metadata::NftMetadataLimits;
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

/// The extension's metadata bounds: `MAX_NFT_URI_BYTES` (`rpc-assets.ts`), `MAX_DATA_URI_BYTES`,
/// `MAX_METADATA_BYTES` and `MAX_ATTRIBUTES` (`nft-token-metadata.ts`),
/// `NFT_METADATA_TIMEOUT_MS` (`runtime-rpc.ts`), `MAX_IMAGE_URL_LENGTH` and
/// `MAX_IMAGE_DATA_URI_LENGTH` (`nft-metadata.ts`), and `clampNftText`'s text lengths.
pub const NFT_METADATA_LIMITS: NftMetadataLimits = NftMetadataLimits {
    uri_bytes: 32_768,
    data_uri_bytes: 32_768,
    document_bytes: 65_536,
    fetch_deadline_ms: 10_000,
    url_length: 2048,
    art_data_uri_length: 32_768,
    attributes: 32,
    text_length: 128,
    description_length: 1_024,
    attribute_value_length: 256,
    display_type_length: 64,
};
