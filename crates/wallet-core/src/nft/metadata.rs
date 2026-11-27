//! A token's metadata (`nft-token-metadata.ts resolveNftMetadataUri`): a JSON `data:` URI is
//! decoded here, an `https` or `ipfs://` one becomes one bounded GET for the host, and either
//! document is read into display-safe fields. Any fault is `None` (the card says the art is
//! unavailable), never an error that would block ownership or a transfer.
//!
//! The extension reads metadata only for NFTs the person watched; an indexer's unsolicited
//! candidate never triggers one (`enrichWatchedNfts`). That gate is the caller's.

use super::media::{
    clamp_text, normalize_animation_uri, normalize_https_url, normalize_image_url, NftAnimation,
};
use crate::js::json::{self, decode_utf8, JsValue};
use crate::js::num::number_to_string;
use crate::js::uri::{atob, decode_uri_component};
use crate::portfolio::wire::{HttpHeader, HttpMethod, HttpOutcome, HttpRequest};

/// The extension's bounds on NFT metadata. Bramble's values are its `NFT_POLICY`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NftMetadataLimits {
    /// Longest on-chain URI read (`MAX_NFT_URI_BYTES`, UTF-8 bytes).
    pub uri_bytes: usize,
    /// Longest URI resolved, and longest document a `data:` URI decodes to
    /// (`MAX_DATA_URI_BYTES`, UTF-8 bytes).
    pub data_uri_bytes: usize,
    /// Longest fetched document (`MAX_METADATA_BYTES`).
    pub document_bytes: u64,
    /// One metadata fetch (`NFT_METADATA_TIMEOUT_MS`).
    pub fetch_deadline_ms: u64,
    /// Longest `https` image or external URL (`MAX_IMAGE_URL_LENGTH`, UTF-16 units).
    pub url_length: usize,
    /// Longest image or animation `data:` URI (`MAX_IMAGE_DATA_URI_LENGTH`, UTF-16 units).
    pub art_data_uri_length: usize,
    /// Attributes read from a document (`MAX_ATTRIBUTES`).
    pub attributes: usize,
    /// Name, collection and trait type (`clampNftText`'s 128).
    pub text_length: usize,
    /// Description (1 024).
    pub description_length: usize,
    /// An attribute's value (256).
    pub attribute_value_length: usize,
    /// An attribute's display type (64).
    pub display_type_length: usize,
}

/// One trait (`NftAttribute`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NftAttribute {
    pub display_type: Option<String>,
    pub trait_type: String,
    pub value: String,
}

/// `NftTokenMetadata`: display-safe fields, each `""` (or empty, or `None`) when absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NftTokenMetadata {
    pub name: String,
    pub collection: String,
    pub description: String,
    /// `""`, a public `https` URL, or a safe image `data:` URI.
    pub image: String,
    /// Kept as the extension keeps it, and marked for what it is: see `NftAnimation`.
    pub animation: Option<NftAnimation>,
    /// `""` or a public `https` URL.
    pub external_url: String,
    pub attributes: Vec<NftAttribute>,
}

/// Where a metadata URI leads.
#[derive(Debug, Clone, PartialEq)]
pub enum MetadataSource {
    /// Decided without a request: a `data:` URI's document, or `None` for a URI that is too long,
    /// malformed, or not public `https` or `ipfs://`.
    Resolved(Option<NftTokenMetadata>),
    /// Fetch this document (no credentials, redirect or referrer, per the wire contract) and
    /// judge it with `metadata_fetch_judge`.
    Fetch(HttpRequest),
}

/// `resolveNftMetadataUri`, sans-IO.
pub fn resolve_metadata_uri(uri: &str, limits: &NftMetadataLimits) -> MetadataSource {
    if uri.len() > limits.data_uri_bytes {
        return MetadataSource::Resolved(None);
    }
    if uri.starts_with("data:") {
        let document = decode_data_uri(uri, limits.data_uri_bytes);
        return MetadataSource::Resolved(
            document.and_then(|text| metadata_from_json(&text, limits)),
        );
    }
    let url = normalize_image_url(uri, limits.url_length, limits.art_data_uri_length);
    // A padded `data:` image URI also passes that rule; the extension would `fetch` it, but a
    // fetch here is an `https` GET only, so it reads as no metadata.
    if !url.starts_with("https://") {
        return MetadataSource::Resolved(None);
    }
    MetadataSource::Fetch(HttpRequest {
        method: HttpMethod::Get,
        url,
        headers: vec![HttpHeader {
            name: "accept".into(),
            value: "application/json".into(),
        }],
        body: None,
        max_body_bytes: limits.document_bytes,
        deadline_ms: limits.fetch_deadline_ms,
    })
}

/// The fetched document's fields: `None` unless the status is 2xx and the whole body arrived
/// within `document_bytes`.
pub fn metadata_fetch_judge(
    outcome: &HttpOutcome,
    limits: &NftMetadataLimits,
) -> Option<NftTokenMetadata> {
    let body = outcome.body.as_ref()?;
    if !(200..300).contains(&outcome.status) || body.len() as u64 > limits.document_bytes {
        return None;
    }
    metadata_from_json(&decode_utf8(body), limits)
}

// `decodeDataUri`: one of the four JSON headers, then base64 or percent-encoded UTF-8, within
// `max_bytes` once decoded.
fn decode_data_uri(uri: &str, max_bytes: usize) -> Option<String> {
    let comma = uri.find(',')?;
    let header = uri[..comma].to_lowercase();
    let payload = &uri[comma + 1..];
    let decoded = match header.as_str() {
        "data:application/json" | "data:application/json;charset=utf-8" => {
            decode_uri_component(payload)?
        }
        "data:application/json;base64" | "data:application/json;charset=utf-8;base64" => {
            decode_utf8(&atob(payload)?)
        }
        _ => return None,
    };
    (decoded.len() <= max_bytes).then_some(decoded)
}

fn text(value: Option<&JsValue>, max_length: usize) -> String {
    value
        .and_then(JsValue::as_str)
        .map(|value| clamp_text(value, max_length))
        .unwrap_or_default()
}

fn str_of(value: Option<&JsValue>) -> &str {
    value.and_then(JsValue::as_str).unwrap_or_default()
}

// `normalizedAttribute`: a string, number or boolean value and a trait type, both non-empty.
fn attribute(value: &JsValue, limits: &NftMetadataLimits) -> Option<NftAttribute> {
    value.as_object()?;
    let raw = match value.get("value")? {
        JsValue::String(text) => text.clone(),
        JsValue::Number(number) => number_to_string(*number),
        JsValue::Bool(flag) => flag.to_string(),
        _ => return None,
    };
    let trait_type = text(value.get("trait_type"), limits.text_length);
    let value_text = clamp_text(&raw, limits.attribute_value_length);
    if trait_type.is_empty() || value_text.is_empty() {
        return None;
    }
    let display_type = text(value.get("display_type"), limits.display_type_length);
    Some(NftAttribute {
        display_type: (!display_type.is_empty()).then_some(display_type),
        trait_type,
        value: value_text,
    })
}

// `metadataFromJson`: a JSON object read into display-safe fields; `None` when it is not one,
// or when every field comes out empty.
fn metadata_from_json(document: &str, limits: &NftMetadataLimits) -> Option<NftTokenMetadata> {
    let value = json::parse(document)?;
    value.as_object()?;
    let collection = match value.get("collection") {
        Some(record @ JsValue::Object(_)) => record.get("name"),
        other => other,
    };
    let attributes = value
        .get("attributes")
        .and_then(JsValue::as_array)
        .map(|items| {
            items
                .iter()
                .take(limits.attributes)
                .filter_map(|item| attribute(item, limits))
                .collect()
        })
        .unwrap_or_default();
    let metadata = NftTokenMetadata {
        name: text(value.get("name"), limits.text_length),
        collection: text(collection, limits.text_length),
        description: text(value.get("description"), limits.description_length),
        image: normalize_image_url(
            str_of(value.get("image")),
            limits.url_length,
            limits.art_data_uri_length,
        ),
        animation: normalize_animation_uri(
            str_of(value.get("animation_url")),
            limits.art_data_uri_length,
        ),
        external_url: normalize_https_url(str_of(value.get("external_url")), limits.url_length),
        attributes,
    };
    let empty = metadata.name.is_empty()
        && metadata.collection.is_empty()
        && metadata.description.is_empty()
        && metadata.image.is_empty()
        && metadata.animation.is_none()
        && metadata.external_url.is_empty()
        && metadata.attributes.is_empty();
    (!empty).then_some(metadata)
}

#[cfg(test)]
#[path = "metadata_tests.rs"]
mod tests;
