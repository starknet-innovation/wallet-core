//! `wallet_addStarknetChain`'s params, read as the extension's `readProviderChain` reads them
//! (`packages/foundation/wallet-protocol/src/transactions/add-chain.ts`). What the wallet then
//! does with a chain it does not already list is the wallet's choice; this only reads it.

use crate::js::json::JsValue;
use crate::js::num::{js_trim, utf16_len};
use crate::nostr::relay_url::is_custom_rpc_url;

/// A chain a page asks the wallet to add (the spec's `STARKNET_CHAIN`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainRequest {
    /// The page's id for the chain, trimmed.
    pub id: String,
    /// `0x` hex as the page spelled it, either case, non-zero.
    pub chain_id: String,
    /// The name the card and a stored record would show, trimmed.
    pub chain_name: String,
    pub rpc_urls: Option<Vec<String>>,
    pub block_explorer_urls: Option<Vec<String>>,
    pub icon_urls: Option<Vec<String>>,
}

/// `isNetworkLabel`'s bound, in UTF-16 units.
const MAX_NETWORK_LABEL_UNITS: usize = 256;

/// `readProviderChain`. `reserved_names` are the wallet's built-in network names, which a page
/// may not reuse (folded as `foldNetworkName` folds them); `max_urls` bounds each URL list.
pub(super) fn add_chain(
    params: Option<&JsValue>,
    reserved_names: &[&str],
    max_urls: usize,
) -> Option<ChainRequest> {
    let params = params?;
    let known = [
        "api_version",
        "block_explorer_url",
        "chain_id",
        "chain_name",
        "icon_urls",
        "id",
        "native_currency",
        "rpc_urls",
    ];
    let entries = params.as_object()?;
    if !entries.iter().all(|(key, _)| known.contains(&key.as_str()))
        || !super::request::optional_api_version(params.get("api_version"))
    {
        return None;
    }
    let site_name = |key: &str| {
        params
            .get(key)?
            .as_str()
            .filter(|name| is_site_network_name(name, reserved_names))
    };
    let id = site_name("id")?;
    let chain_id = params
        .get("chain_id")?
        .as_str()
        .filter(|id| is_canonical_chain_id(id))?;
    let chain_name = site_name("chain_name")?;
    // A present `native_currency`, `null` included, must be an object; it is then dropped.
    if params
        .get("native_currency")
        .is_some_and(|currency| currency.as_object().is_none())
    {
        return None;
    }
    let list = |key: &str| match params.get(key) {
        None => Some(None),
        Some(value) => url_list(value, max_urls).map(Some),
    };
    Some(ChainRequest {
        id: js_trim(id).to_owned(),
        chain_id: chain_id.to_owned(),
        chain_name: js_trim(chain_name).to_owned(),
        rpc_urls: list("rpc_urls")?,
        block_explorer_urls: list("block_explorer_url")?,
        icon_urls: list("icon_urls")?,
    })
}

/// `isCanonicalChainId`: `0x` and 1–64 hex digits of either case, not zero. Not checked
/// against the field prime, as the extension does not.
fn is_canonical_chain_id(value: &str) -> bool {
    value.strip_prefix("0x").is_some_and(|digits| {
        (1..=64).contains(&digits.len())
            && digits.bytes().all(|b| b.is_ascii_hexdigit())
            && digits.bytes().any(|b| b != b'0')
    })
}

/// `isSiteNetworkName`: a network label (at most 256 units, not blank) of printable ASCII
/// without `<` or `>`, that is not a built-in network's name once folded.
fn is_site_network_name(name: &str, reserved_names: &[&str]) -> bool {
    utf16_len(name) <= MAX_NETWORK_LABEL_UNITS
        && !js_trim(name).is_empty()
        && name
            .bytes()
            .all(|b| (0x20..=0x7e).contains(&b) && b != b'<' && b != b'>')
        && !reserved_names
            .iter()
            .any(|reserved| fold_network_name(reserved) == fold_network_name(name))
}

/// `foldNetworkName`: trimmed, runs of whitespace as one space, lower case.
fn fold_network_name(name: &str) -> String {
    js_trim(name)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// `isBoundedServiceUrlList`: a non-empty list of distinct service URLs, at most `max_urls`.
fn url_list(value: &JsValue, max_urls: usize) -> Option<Vec<String>> {
    let items = value.as_array()?;
    if items.is_empty() || items.len() > max_urls {
        return None;
    }
    let urls = items
        .iter()
        .map(|item| {
            item.as_str()
                .filter(|url| is_custom_rpc_url(url))
                .map(str::to_owned)
        })
        .collect::<Option<Vec<_>>>()?;
    let mut seen = urls.clone();
    seen.sort();
    seen.dedup();
    (seen.len() == urls.len()).then_some(urls)
}

#[cfg(test)]
#[path = "add_chain_tests.rs"]
mod tests;
