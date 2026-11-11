//! Nostr relay protocol as sans-IO judges, mirroring the extension's `nostr-relay-socket.ts` and
//! `nostr-relay.ts`. The host owns the WebSockets and the timers; the core builds every frame it
//! sends and judges every frame it receives. This module holds the shared rules (relay selection,
//! frame parsing, the gift-wrap batch) and publishing; `sync` holds the paged read.
//!
//! How many relays a wallet may select is its choice, so it is an argument ([`RelayLimits`]).

use serde_json::{json, Value};

use super::relay_url::is_nostr_relay_url;
use super::{
    has_recipient_tag, is_public_key, parse_event_json, NostrEvent, GIFT_WRAP_KIND, MAX_BATCH_ITEMS,
};

/// `MAX_RELAY_FRAME_BYTES`: a larger relay frame is ignored.
pub const MAX_RELAY_FRAME_BYTES: usize = 140_000;

/// The extension's relay errors, each displayed as its exact message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RelayError {
    #[error("Invalid Nostr relay configuration")]
    InvalidRelayConfiguration,
    #[error("Invalid Nostr gift wrap batch")]
    InvalidGiftWrapBatch,
    #[error("Invalid Nostr gift wrap")]
    InvalidGiftWrap,
    #[error("Invalid Nostr sync request")]
    InvalidSyncRequest,
    #[error("Nostr relays are unavailable")]
    RelaysUnavailable,
}

/// A wallet's relay numbers (Bramble's: two relays, the length of its default list, and four
/// pages).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelayLimits {
    /// The most relays one publish or sync may select.
    pub max_relays: usize,
    /// `MAX_SYNC_PAGES`: pages read from one relay per sync while its answers come back full.
    /// The first page is always read.
    pub max_sync_pages: usize,
}

/// What the host does after handing the core a frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayStep {
    /// Keep reading.
    Continue,
    /// Send `close_frame` if present; this relay is done.
    Finish { close_frame: Option<String> },
    /// Send `close_frame`, then `request` (the next page) on the same socket, and keep reading.
    NextPage {
        close_frame: String,
        request: String,
    },
}

/// `validRelayUrls`: 1..=`max_relays` distinct relay URLs, each `isNostrRelayUrl`.
pub fn valid_relay_urls(relay_urls: &[String], max_relays: usize) -> Result<(), RelayError> {
    let unique = relay_urls
        .iter()
        .enumerate()
        .all(|(i, url)| !relay_urls[..i].contains(url));
    if relay_urls.is_empty()
        || relay_urls.len() > max_relays
        || !unique
        || !relay_urls.iter().all(|url| is_nostr_relay_url(url))
    {
        return Err(RelayError::InvalidRelayConfiguration);
    }
    Ok(())
}

/// `parseFrame`: a JSON array of at most four elements, no more than 140,000 bytes.
pub(crate) fn parse_frame(frame: &str) -> Option<Vec<Value>> {
    if frame.len() > MAX_RELAY_FRAME_BYTES {
        return None;
    }
    let value: Value = serde_json::from_str(frame).ok()?;
    let parts = value.as_array()?;
    (parts.len() <= 4).then(|| parts.clone())
}

/// `NostrGiftWrapDelivery`: one recipient-bound gift wrap awaiting relay acknowledgement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NostrGiftWrapDelivery {
    pub event_json: String,
    pub recipient_public_key: String,
}

/// `parsedGiftWraps`, checks in the extension's order: the batch size, then each wrap in turn
/// (parses, canonical recipient, kind 1059, the recipient's `p` tag), then distinct ids.
pub fn parsed_gift_wraps(
    gift_wraps: &[NostrGiftWrapDelivery],
) -> Result<Vec<NostrEvent>, RelayError> {
    if gift_wraps.is_empty() || gift_wraps.len() > MAX_BATCH_ITEMS {
        return Err(RelayError::InvalidGiftWrapBatch);
    }
    let events = gift_wraps
        .iter()
        .map(|wrap| match parse_event_json(&wrap.event_json) {
            Some(event)
                if is_public_key(&wrap.recipient_public_key)
                    && event.kind == GIFT_WRAP_KIND
                    && has_recipient_tag(&event, &wrap.recipient_public_key) =>
            {
                Ok(event)
            }
            _ => Err(RelayError::InvalidGiftWrap),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let distinct = events
        .iter()
        .enumerate()
        .all(|(i, event)| !events[..i].iter().any(|other| other.id == event.id));
    if !distinct {
        return Err(RelayError::InvalidGiftWrapBatch);
    }
    Ok(events)
}

#[cfg(test)]
#[path = "relay_tests.rs"]
mod tests;
