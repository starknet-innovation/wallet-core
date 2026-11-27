//! What a collectible may show (`nft-metadata.ts`): an untrusted image reference becomes a public
//! `https` URL (`ipfs://` through a fixed gateway) or a safe image `data:` URI, and an animation
//! only a self-contained `data:text/html` document; anything else is dropped (`""`) so the card
//! shows its placeholder.
//!
//! A URL is read as `new URL` reads it (`js::url::parse_https`) and kept as its `href`, and the
//! local and private host checks run on the parsed hostname, so they judge the host a client
//! connects to.

use crate::js::num::{js_trim, string_to_number, utf16_len, utf16_slice};
use crate::js::url::parse_https;

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

// `isPrivateIpv4`: four parts, each `Number(part)` an integer from 0 to 255.
fn is_private_ipv4(host: &str) -> bool {
    let parts: Vec<f64> = host.split('.').map(string_to_number).collect();
    if parts.len() != 4
        || parts
            .iter()
            .any(|part| part.fract() != 0.0 || !(0.0..=255.0).contains(part))
    {
        return false;
    }
    let (first, second) = (parts[0], parts[1]);
    first == 0.0
        || first == 10.0
        || first == 127.0
        || (first == 169.0 && second == 254.0)
        || (first == 172.0 && (16.0..=31.0).contains(&second))
        || (first == 192.0 && second == 168.0)
        || first >= 224.0
}

/// `normalizeNftHttpsUrl`: `new URL(value.trim()).href` for a public `https` URL, or `""` for
/// anything longer than `max_length` (UTF-16 units, before trimming), not `https`, with
/// credentials, or on a local or private host (`localhost`, `*.localhost`, `*.local`, an IPv6
/// literal, a private or reserved IPv4 address). A host only IDNA could read is dropped too
/// (`js::url`).
pub fn normalize_https_url(value: &str, max_length: usize) -> String {
    if utf16_len(value) > max_length {
        return String::new();
    }
    let Some(url) = parse_https(js_trim(value)) else {
        return String::new();
    };
    // `normalizeHostname`: lowercase, one trailing dot dropped.
    let host = url.hostname.strip_suffix('.').unwrap_or(&url.hostname);
    let local = url.credentials
        || host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || host.contains(':')
        || is_private_ipv4(host);
    if local {
        String::new()
    } else {
        url.href
    }
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
    let trimmed = js_trim(value);
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
    let trimmed = js_trim(value);
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
    utf16_slice(js_trim(value), 0, max_length)
}

#[cfg(test)]
#[path = "media_tests.rs"]
mod tests;
