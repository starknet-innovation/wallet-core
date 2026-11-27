//! Starkscan as the built-in NFT indexer (`starkscan-nfts.ts`, the client's
//! `getAddressNftPositions` and `parseStarkscanNftPositions`): one page of current ERC-721
//! positions, Mainnet only. A malformed field anywhere, another chain or owner, a zero contract,
//! a token id past u256 or a duplicate position refuses the page; the collection name is the
//! deployer's own text, so a bad one is dropped rather than refusing the page. Starkscan never
//! claims global completeness: the page is complete only within the disclosed ERC-721 scope.

use super::discovery::{trim_trailing_slashes, NftCandidate, NftCoverage, NftDiscovery};
use super::{NftId, NftPolicy};
use crate::history::starkscan::{
    felt_value, integer_value, nullable_string, starkscan_get, starkscan_json, string_value,
};
use crate::js::felt::{canonical_felt, same_felt};
use crate::js::json::JsValue;
use crate::js::num::{js_trim, utf16_len};
use crate::portfolio::wire::{HttpOutcome, HttpRequest};
use crate::send::u256::U256;
use crate::{felt, NetworkId};

/// `STARKSCAN_NFT_NETWORK`.
const NETWORK: NetworkId = NetworkId::Mainnet;
/// Starkscan's page maximum (`MAX_NFT_POSITIONS`, and the client's `limit` range).
const MAX_POSITIONS: usize = 100;
const SCHEMA_VERSION: &str = "starkscan.nft_positions.v1";
const DECLARED_SCOPE: &str = "indexed_standard_erc721_transfers";
/// The one exclusion the wallet discloses at opt-in; any other reason is a gap.
const DISCLOSED_EXCLUSION: &str = "unsupported_erc1155";
const MAX_COLLECTION_NAME: usize = 64;

/// `GET {base}/SN_MAIN/address/{canonical owner}/nft-positions?limit={n}`; `None` (sending
/// nothing) for an owner `inputAddress` refuses or a limit outside 1 to 100.
pub(super) fn request(owner: &str, policy: &NftPolicy) -> Option<HttpRequest> {
    let owner = felt::parse_non_zero(owner)?;
    let limit = policy.starkscan_page_size;
    (1..=MAX_POSITIONS).contains(&limit).then(|| {
        starkscan_get(format!(
            "{}/{}/address/{}/nft-positions?limit={limit}",
            trim_trailing_slashes(policy.starkscan_api_base),
            NETWORK.catalog_id(),
            felt::canonical(&owner)
        ))
    })
}

/// The reply as candidates for `owner`; `None` is `Unavailable`.
pub(super) fn judge(outcome: &HttpOutcome, owner: &str) -> Option<NftDiscovery> {
    from_positions(&parse_page(&starkscan_json(outcome)?)?, owner)
}

struct Position {
    token_address: String,
    token_id: String,
    collection_name: Option<String>,
    collection_indexed: bool,
}

struct Page {
    chain_id: String,
    owner_address: String,
    items: Vec<Position>,
    has_more: bool,
    status_ready: bool,
    declared_scope: String,
    complete_within_scope: bool,
    reason_codes: Vec<String>,
}

fn boolean(value: Option<&JsValue>) -> Option<bool> {
    match value? {
        JsValue::Bool(value) => Some(*value),
        _ => None,
    }
}

fn one_of<'a>(value: Option<&JsValue>, allowed: &[&'a str]) -> Option<&'a str> {
    let text = value?.as_str()?;
    allowed.iter().copied().find(|allowed| *allowed == text)
}

// `tokenIdValue`: decimal (leading zeros allowed) or `0x` hex, at most 80 units.
fn token_id_value(value: Option<&JsValue>) -> Option<String> {
    let text = string_value(value, 80)?;
    let ok = match text.strip_prefix("0x") {
        Some(hex) => (1..=64).contains(&hex.len()) && hex.bytes().all(|b| b.is_ascii_hexdigit()),
        None => (1..=78).contains(&text.len()) && text.bytes().all(|b| b.is_ascii_digit()),
    };
    ok.then_some(text)
}

// `parseNftPosition`.
fn parse_position(row: &JsValue) -> Option<Position> {
    row.as_object()?;
    let collection = row.get("collection")?;
    collection.as_object()?;
    let collection_name = match collection.get("name")? {
        JsValue::Null => None,
        JsValue::String(name) => Some(name.clone()),
        _ => return None,
    };
    nullable_string(collection.get("symbol"), usize::MAX)?;
    one_of(row.get("standard"), &["erc721"])?;
    one_of(row.get("evidenceSource"), &["indexed_erc721_ownership"])?;
    integer_value(row.get("observedAtBlock"))?;
    felt_value(row.get("lastTransferHash"))?;
    Some(Position {
        token_address: felt_value(row.get("tokenAddress"))?,
        token_id: token_id_value(row.get("tokenId"))?,
        collection_name,
        collection_indexed: one_of(
            collection.get("metadataStatus"),
            &["indexed", "unavailable"],
        )? == "indexed",
    })
}

// `parseStarkscanNftPositions` with `parseNftCoverage`.
fn parse_page(value: &JsValue) -> Option<Page> {
    value.as_object()?;
    one_of(value.get("schemaVersion"), &[SCHEMA_VERSION])?;
    integer_value(value.get("snapshotBlock"))?;
    nullable_string(value.get("nextCursor"), 2_048)?;
    let items = value.get("items")?.as_array()?;
    if items.len() > MAX_POSITIONS {
        return None;
    }
    let coverage = value.get("coverage")?;
    coverage.as_object()?;
    boolean(coverage.get("globallyComplete"))?;
    integer_value(coverage.get("indexedFromBlock"))?;
    integer_value(coverage.get("indexedThroughBlock"))?;
    let reasons = coverage.get("reasonCodes")?.as_array()?;
    if reasons.len() > 1_024 {
        return None;
    }
    Some(Page {
        chain_id: string_value(value.get("chainId"), 64)?,
        owner_address: felt_value(value.get("ownerAddress"))?,
        items: items.iter().map(parse_position).collect::<Option<_>>()?,
        has_more: boolean(value.get("hasMore"))?,
        status_ready: one_of(coverage.get("status"), &["partial", "ready"])? == "ready",
        declared_scope: string_value(coverage.get("declaredScope"), 128)?,
        complete_within_scope: boolean(coverage.get("completeWithinScope"))?,
        reason_codes: reasons
            .iter()
            .map(|code| string_value(Some(code), 128))
            .collect::<Option<_>>()?,
    })
}

// C0/C1 controls and every Unicode Bidi_Control character.
fn is_unsafe(c: char) -> bool {
    matches!(
        u32::from(c),
        0..=0x1f | 0x7f..=0x9f | 0x061c | 0x200e | 0x200f | 0x202a..=0x202e | 0x2066..=0x2069
    )
}

// `safeName`.
fn safe_name(name: Option<&str>, indexed: bool) -> String {
    match name {
        Some(name)
            if indexed
                && utf16_len(name) <= MAX_COLLECTION_NAME
                && !name.chars().any(is_unsafe) =>
        {
            js_trim(name).to_string()
        }
        _ => String::new(),
    }
}

// `BigInt(tokenId)` within u256.
fn token_id(text: &str) -> Option<U256> {
    match text.strip_prefix("0x") {
        Some(hex) => U256::from_hex(hex),
        None => U256::from_decimal(text),
    }
}

// `nftDiscoveryFromPositions`.
fn from_positions(page: &Page, owner: &str) -> Option<NftDiscovery> {
    if page.chain_id != NETWORK.catalog_id() || !same_felt(&page.owner_address, owner) {
        return None;
    }
    let mut nfts: Vec<NftCandidate> = Vec::new();
    for item in &page.items {
        let contract = canonical_felt(&item.token_address)
            .and_then(|canonical| felt::parse_non_zero(&canonical))?;
        let id = NftId {
            contract,
            token_id: token_id(&item.token_id)?,
        };
        if nfts.iter().any(|kept| kept.id == id) {
            return None;
        }
        nfts.push(NftCandidate {
            id,
            name: String::new(),
            collection: safe_name(item.collection_name.as_deref(), item.collection_indexed),
            image: String::new(),
        });
    }
    let complete = page.status_ready
        && page.complete_within_scope
        && page.declared_scope == DECLARED_SCOPE
        && page
            .reason_codes
            .iter()
            .all(|code| code == DISCLOSED_EXCLUSION)
        && !page.has_more;
    Some(NftDiscovery {
        coverage: if complete {
            NftCoverage::Complete
        } else {
            NftCoverage::Incomplete
        },
        nfts,
    })
}

#[cfg(test)]
#[path = "starkscan_tests.rs"]
mod tests;
