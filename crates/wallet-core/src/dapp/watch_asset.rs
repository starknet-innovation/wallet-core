//! `wallet_watchAsset`'s params, read as the extension's `readProviderWatchAsset` and
//! `readProviderWatchNft` read them (`packages/foundation/wallet-protocol/src/transactions/
//! watch-asset.ts`): an ERC-20 by the wallet API, or an ERC-721 or ERC-1155 collectible in the
//! EIP-747 shape, which the wallet API does not have yet. A page's `image` (and a collectible's
//! `name`) is accepted as a string and dropped: the wallet never fetches what a site points at.

use starknet_types_core::felt::Felt;

use crate::js::big::parse_bigint;
use crate::js::json::JsValue;
use crate::js::num::utf16_len;
pub use crate::nft::NftStandard;

/// An asset a page asks the wallet to track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchAsset {
    /// The page's own decimals, name and symbol are claims for the review to weigh, never facts.
    Erc20 {
        address: Felt,
        decimals: Option<u8>,
        name: Option<String>,
        symbol: Option<String>,
    },
    Nft {
        address: Felt,
        standard: NftStandard,
        /// A u256, in decimal.
        token_id: String,
    },
}

/// `isSafeDecimals`'s bound.
const MAX_DECIMALS: f64 = 36.0;
/// `tokenIdDecimal`'s bound on the spelling, in UTF-16 units.
const MAX_TOKEN_ID_UNITS: usize = 80;

/// The ERC-20 reader, then the collectible one, as the extension's bridge tries them.
pub(super) fn watch_asset(params: Option<&JsValue>) -> Option<WatchAsset> {
    let params = params?;
    let entries = params.as_object()?;
    if !entries
        .iter()
        .all(|(key, _)| ["api_version", "options", "type"].contains(&key.as_str()))
        || !super::request::optional_api_version(params.get("api_version"))
    {
        return None;
    }
    let options = params.get("options")?;
    let options_entries = options.as_object()?;
    match params.get("type")?.as_str()? {
        "ERC20" => erc20(options, options_entries),
        "ERC721" => nft(options, options_entries, NftStandard::Erc721),
        "ERC1155" => nft(options, options_entries, NftStandard::Erc1155),
        _ => None,
    }
}

/// A present value, `null` included, must be a string.
fn optional_string(value: Option<&JsValue>) -> Option<Option<String>> {
    match value {
        None => Some(None),
        Some(value) => value.as_str().map(|text| Some(text.to_owned())),
    }
}

/// `readProviderWatchAsset`.
fn erc20(options: &JsValue, entries: &[(String, JsValue)]) -> Option<WatchAsset> {
    let known = ["address", "decimals", "image", "name", "symbol"];
    if !entries.iter().all(|(key, _)| known.contains(&key.as_str())) {
        return None;
    }
    optional_string(options.get("image"))?;
    let decimals = match options.get("decimals") {
        None => None,
        Some(value) => Some(safe_decimals(value)?),
    };
    Some(WatchAsset::Erc20 {
        address: non_zero_address(options)?,
        decimals,
        name: optional_string(options.get("name"))?,
        symbol: optional_string(options.get("symbol"))?,
    })
}

/// `readProviderWatchNft`.
fn nft(
    options: &JsValue,
    entries: &[(String, JsValue)],
    standard: NftStandard,
) -> Option<WatchAsset> {
    let known = ["address", "image", "name", "tokenId"];
    if !entries.iter().all(|(key, _)| known.contains(&key.as_str())) {
        return None;
    }
    optional_string(options.get("image"))?;
    optional_string(options.get("name"))?;
    let address = non_zero_address(options)?;
    let token_id = token_id_decimal(options.get("tokenId")?.as_str()?)?;
    Some(WatchAsset::Nft {
        address,
        standard,
        token_id,
    })
}

/// `isNonZeroStarkFieldElement` over `options.address`.
fn non_zero_address(options: &JsValue) -> Option<Felt> {
    crate::felt::parse_non_zero(options.get("address")?.as_str()?)
}

/// `isSafeDecimals`: an integer from 0 to 36.
fn safe_decimals(value: &JsValue) -> Option<u8> {
    let number = value.as_f64()?;
    (number.fract() == 0.0 && (0.0..=MAX_DECIMALS).contains(&number)).then_some(number as u8)
}

/// `tokenIdDecimal`: a u256 written as base-10 digits (no leading zero, at most 78) or as `0x`
/// and 1–64 hex digits of either case, at most 80 units long, returned in decimal.
fn token_id_decimal(value: &str) -> Option<String> {
    let units = utf16_len(value);
    if units == 0 || units > MAX_TOKEN_ID_UNITS {
        return None;
    }
    let decimal = value == "0"
        || (value.len() <= 78
            && !value.starts_with('0')
            && value.bytes().all(|b| b.is_ascii_digit()));
    let hex = value.strip_prefix("0x").is_some_and(|digits| {
        (1..=64).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_hexdigit())
    });
    if !decimal && !hex {
        return None;
    }
    let parsed = parse_bigint(value)?;
    let max = parse_bigint(&format!("0x{}", "f".repeat(64)))?;
    (parsed.magnitude <= max.magnitude).then(|| parsed.magnitude.to_decimal())
}

#[cfg(test)]
#[path = "watch_asset_tests.rs"]
mod tests;
