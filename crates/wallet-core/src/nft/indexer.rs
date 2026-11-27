//! A person-set NFT indexer (`nft-indexer.ts`, `normalizeIndexerNft` in `nft-metadata.ts`):
//!
//! `GET {indexerUrl}/accounts/{address}/nfts?network={networkId}` answers `NftRecord[]` or
//! `{ nfts: NftRecord[] }`, `NftRecord = { contract_address, token_id, name?,
//! collection_name?, image_url? }`. A record the wallet cannot read makes the list incomplete,
//! not shorter; past `max_discovered` the list is cut, and incomplete.

use super::discovery::{trim_trailing_slashes, NftCandidate, NftCoverage, NftDiscovery};
use super::{NftId, NftPolicy};
use crate::js::felt::canonical_felt;
use crate::js::json::{self, JsValue};
use crate::js::num::{js_trim, number_to_string, utf16_len, utf16_slice};
use crate::portfolio::wire::{HttpMethod, HttpOutcome, HttpRequest};
use crate::send::u256::U256;
use crate::NetworkId;

/// `MAX_TEXT_LENGTH`: a name's bound, in UTF-16 units.
const MAX_TEXT_LENGTH: usize = 128;
/// `MAX_IMAGE_DATA_URI_LENGTH`: no image reference the metadata rule keeps is longer.
const MAX_IMAGE_LENGTH: usize = 32_768;
/// `MAX_TOKEN_ID_DIGITS`.
const MAX_TOKEN_ID_DIGITS: usize = 78;

/// `ownedNftsUrl` as a `fetchBoundedJson` GET: no header, the indexer's budget and deadline.
/// `owner` is sent as the wallet stores it, and the saved URL has passed `is_custom_rpc_url`.
pub(super) fn request(
    indexer_url: &str,
    owner: &str,
    network: NetworkId,
    policy: &NftPolicy,
) -> HttpRequest {
    HttpRequest {
        method: HttpMethod::Get,
        url: format!(
            "{}/accounts/{owner}/nfts?network={}",
            trim_trailing_slashes(indexer_url),
            network.catalog_id()
        ),
        headers: Vec::new(),
        body: None,
        max_body_bytes: policy.indexer_body_limit,
        deadline_ms: policy.indexer_deadline_ms,
    }
}

/// `fetchIndexerBody`, `recordsFrom` and `foldDiscovered`; `None` is `Unavailable`.
pub(super) fn judge(outcome: &HttpOutcome, policy: &NftPolicy) -> Option<NftDiscovery> {
    let body = outcome.body.as_ref()?;
    if !(200..300).contains(&outcome.status) || body.len() as u64 > policy.indexer_body_limit {
        return None;
    }
    let value = json::parse(&json::decode_utf8(body))?;
    let records = match &value {
        JsValue::Array(records) => records,
        JsValue::Object(_) => match value.get("nfts") {
            Some(JsValue::Array(records)) => records,
            _ => return None,
        },
        _ => return None,
    };
    Some(fold(records, policy.max_discovered))
}

fn fold(records: &[JsValue], max: usize) -> NftDiscovery {
    let mut nfts: Vec<NftCandidate> = Vec::new();
    let mut complete = records.len() <= max;
    for record in records {
        if nfts.len() >= max {
            break;
        }
        match candidate(record) {
            None => complete = false,
            Some(nft) if !nfts.iter().any(|kept| kept.id == nft.id) => nfts.push(nft),
            Some(_) => {}
        }
    }
    NftDiscovery {
        coverage: if complete {
            NftCoverage::Complete
        } else {
            NftCoverage::Incomplete
        },
        nfts,
    }
}

/// `normalizeIndexerNft`, but for the image (see `NftCandidate::image`).
fn candidate(record: &JsValue) -> Option<NftCandidate> {
    record.as_object()?;
    let contract = canonical_felt(record.get("contract_address")?.as_str()?)
        .and_then(|canonical| crate::felt::parse_non_zero(&canonical))?;
    let token_id = match record.get("token_id")? {
        JsValue::String(text) => js_trim(text).to_string(),
        JsValue::Number(number) => js_trim(&number_to_string(*number)).to_string(),
        _ => return None,
    };
    // `isBaseUnitsAmount` and the digit cap, then the u256 range.
    let bytes = token_id.as_bytes();
    if bytes.len() > MAX_TOKEN_ID_DIGITS || (bytes.len() > 1 && bytes[0] == b'0') {
        return None;
    }
    let token_id = U256::from_decimal(&token_id)?;
    let image = match record.get("image_url") {
        Some(JsValue::String(text)) if utf16_len(text) <= MAX_IMAGE_LENGTH => text.clone(),
        _ => String::new(),
    };
    Some(NftCandidate {
        id: NftId { contract, token_id },
        name: clamp_text(record.get("name")),
        collection: clamp_text(record.get("collection_name")),
        image,
    })
}

/// `clampNftText`.
fn clamp_text(value: Option<&JsValue>) -> String {
    match value {
        Some(JsValue::String(text)) => utf16_slice(js_trim(text), 0, MAX_TEXT_LENGTH),
        _ => String::new(),
    }
}

#[cfg(test)]
#[path = "indexer_tests.rs"]
mod tests;
