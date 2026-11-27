//! What a collectible may show (`nft-metadata.ts`): an untrusted image reference becomes a public
//! `https` URL (`ipfs://` through a fixed gateway) or a safe image `data:` URI, and an animation
//! only a self-contained `data:text/html` document; anything else is dropped (`""`) so the card
//! shows its placeholder.
//!
//! The URL is checked without a URL parser, through the endpoint grammar
//! (`nostr::relay_url::is_custom_rpc_url`): a spelling the WHATWG parser would rewrite is dropped
//! rather than rewritten, except the case of the scheme and host and a missing root path, which
//! are spelled here as the parser prints them.

use crate::js::num::{utf16_len, utf16_slice};
use crate::nostr::relay_url::is_custom_rpc_url;

/// The gateway an `ipfs://` reference is read through (`IPFS_GATEWAY`); only the host is fixed.
pub const IPFS_GATEWAY: &str = "https://ipfs.io/ipfs/";

/// Image media types an image `data:` URI may carry (`DATA_URI_IMAGE_TYPES`): an SVG is drawn
/// script-less in an image element, rasters are inert.
const DATA_URI_IMAGE_TYPES: [&str; 6] = [
    "image/avif",
    "image/gif",
    "image/jpeg",
    "image/png",
    "image/svg+xml",
    "image/webp",
];
const SVG: &str = "image/svg+xml";

/// An animation the wallet may show. The extension renders it only inside a sandboxed frame,
/// on the person's request; an images-only screen shows the image and leaves this alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NftAnimation {
    /// A `data:text/html` URI, verbatim: a self-contained document that runs script and must
    /// never be rendered outside an isolated, egress-blocked sandbox.
    Html(String),
}

// `isPrivateIpv4`, over a dotted quad.
fn is_private_ipv4(host: &str) -> bool {
    let parts: Vec<u8> = host
        .split('.')
        .filter_map(|part| part.parse().ok())
        .collect();
    let [first, second, _, _] = parts[..] else {
        return false;
    };
    host.split('.').count() == 4
        && (first == 0
            || first == 10
            || first == 127
            || (first == 169 && second == 254)
            || (first == 172 && (16..=31).contains(&second))
            || (first == 192 && second == 168)
            || first >= 224)
}

/// `normalizeNftHttpsUrl`: a public `https` URL as the parser prints it, or `""` for anything
/// longer than `max_length` (UTF-16 units, before trimming), not `https`, with credentials, or
/// on a local or private host (`localhost`, `*.localhost`, `*.local`, an IPv6 literal, a private
/// or reserved IPv4 address).
pub fn normalize_https_url(value: &str, max_length: usize) -> String {
    if utf16_len(value) > max_length {
        return String::new();
    }
    let trimmed = crate::js::num::js_trim(value);
    let Some(rest) = trimmed
        .get(..8)
        .filter(|scheme| scheme.eq_ignore_ascii_case("https://"))
        .and_then(|_| trimmed.get(8..))
    else {
        return String::new();
    };
    let split = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(split);
    let authority = authority.to_ascii_lowercase();
    let root = if tail.starts_with('/') { "" } else { "/" };
    let href = format!("https://{authority}{root}{tail}");
    // The grammar refuses credentials, so `authority` is `host[:port]`.
    let host = match authority.rfind(':').filter(|_| !authority.ends_with(']')) {
        Some(index) => &authority[..index],
        None => &authority,
    };
    let host = host.strip_suffix('.').unwrap_or(host);
    let local = host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.contains([':', '['])
        || is_private_ipv4(host);
    if local || !is_custom_rpc_url(&href) {
        return String::new();
    }
    href
}

fn is_base64_payload(payload: &str) -> bool {
    let data = payload.trim_end_matches('=');
    !data.is_empty()
        && payload.len() - data.len() <= 2
        && data
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'+' || b == b'/')
}

// A `data:` URI's comma and its lowercased `;`-separated parameters (media type first).
fn data_header(uri: &str) -> Option<(usize, Vec<String>)> {
    let comma = uri.find(',')?;
    let params = uri
        .get("data:".len()..comma)?
        .to_lowercase()
        .split(';')
        .map(str::to_owned)
        .collect();
    Some((comma, params))
}

// `normalizeImageDataUri`: returned verbatim (case-sensitive base64 survives), or `""`.
fn image_data_uri(value: &str, max_length: usize) -> String {
    if utf16_len(value) > max_length {
        return String::new();
    }
    let Some((comma, params)) = data_header(value) else {
        return String::new();
    };
    let payload = &value[comma + 1..];
    let media_type = params.first().map_or("", String::as_str);
    let base64 = params.iter().any(|p| p == "base64");
    let usable = !payload.is_empty()
        && DATA_URI_IMAGE_TYPES.contains(&media_type)
        && (base64 || media_type == SVG)
        && (!base64 || is_base64_payload(payload));
    if usable {
        value.to_owned()
    } else {
        String::new()
    }
}

/// `normalizeImageUrl`: `ipfs://<cid>` through `IPFS_GATEWAY`, a public `https` URL (at most
/// `max_url_length`), or a safe image `data:` URI (at most `max_data_uri_length`), else `""`.
pub fn normalize_image_url(
    value: &str,
    max_url_length: usize,
    max_data_uri_length: usize,
) -> String {
    let trimmed = crate::js::num::js_trim(value);
    if trimmed.starts_with("data:") {
        return image_data_uri(trimmed, max_data_uri_length);
    }
    if let Some(path) = trimmed.strip_prefix("ipfs://") {
        let path = path.strip_prefix("ipfs/").unwrap_or(path);
        if path.is_empty() {
            return String::new();
        }
        return normalize_https_url(&format!("{IPFS_GATEWAY}{path}"), max_url_length);
    }
    normalize_https_url(trimmed, max_url_length)
}

/// `normalizeAnimationUri`: only a `data:text/html` URI (base64 or raw) of at most `max_length`
/// UTF-16 units, verbatim; a remote animation is dropped so an opened collectible never makes a
/// request.
pub fn normalize_animation_uri(value: &str, max_length: usize) -> Option<NftAnimation> {
    let trimmed = crate::js::num::js_trim(value);
    if !trimmed.starts_with("data:") || utf16_len(trimmed) > max_length {
        return None;
    }
    let (comma, params) = data_header(trimmed)?;
    let payload = &trimmed[comma + 1..];
    if payload.is_empty() || params.first().map(String::as_str) != Some("text/html") {
        return None;
    }
    if params.iter().any(|p| p == "base64") && !is_base64_payload(payload) {
        return None;
    }
    Some(NftAnimation::Html(trimmed.to_owned()))
}

/// `clampNftText`: trimmed and cut to `max_length` UTF-16 units.
pub fn clamp_text(value: &str, max_length: usize) -> String {
    utf16_slice(crate::js::num::js_trim(value), 0, max_length)
}

#[cfg(test)]
#[path = "media_tests.rs"]
mod tests;
