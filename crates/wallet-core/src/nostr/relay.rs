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

/// Publish judging for one relay (`RelayConnection.publish`): EVENT frames out, OK frames in. A
/// false OK whose message starts with `duplicate:` counts as accepted; the first OK for an event
/// decides it.
#[derive(Debug, Clone)]
pub struct PublishJudge {
    events: Vec<NostrEvent>,
    acks: Vec<Option<bool>>,
}

impl PublishJudge {
    /// A judge for validated, distinct gift wraps.
    pub fn new(events: Vec<NostrEvent>) -> Self {
        let acks = vec![None; events.len()];
        Self { events, acks }
    }

    /// The EVENT frames to send.
    pub fn event_frames(&self) -> Vec<String> {
        self.events
            .iter()
            .map(|event| json!(["EVENT", event]).to_string())
            .collect()
    }

    /// Judges one frame; `Finish` once every event is acknowledged.
    pub fn accept_frame(&mut self, frame: &str) -> RelayStep {
        if let Some(parts) = parse_frame(frame) {
            let accepted = parts.get(2).and_then(Value::as_bool);
            let duplicate = parts
                .get(3)
                .and_then(Value::as_str)
                .is_some_and(|message| message.to_lowercase().starts_with("duplicate:"));
            if let (Some("OK"), Some(id), Some(accepted)) = (
                parts.first().and_then(Value::as_str),
                parts.get(1).and_then(Value::as_str),
                accepted,
            ) {
                if let Some(index) = self.events.iter().position(|event| event.id == id) {
                    self.acks[index].get_or_insert(accepted || duplicate);
                }
            }
        }
        if self.acks.iter().all(Option::is_some) {
            RelayStep::Finish { close_frame: None }
        } else {
            RelayStep::Continue
        }
    }

    /// Whether each event was accepted; anything unacknowledged (the relay failed, timed out or
    /// closed) was not.
    pub fn accepted(&self) -> Vec<bool> {
        self.acks.iter().map(|ack| ack.unwrap_or(false)).collect()
    }
}

/// Acknowledgement counts across relays, one per event; "Nostr relays are unavailable" when no
/// relay accepted anything, as `publishWithPool`.
pub fn publish_counts(per_relay: &[Vec<bool>]) -> Result<Vec<u32>, RelayError> {
    let count = per_relay.first().map_or(0, Vec::len);
    let counts: Vec<u32> = (0..count)
        .map(|index| {
            let accepted = per_relay
                .iter()
                .filter(|acks| acks.get(index) == Some(&true))
                .count();
            u32::try_from(accepted).unwrap_or(u32::MAX)
        })
        .collect();
    if counts.iter().all(|count| *count == 0) {
        return Err(RelayError::RelaysUnavailable);
    }
    Ok(counts)
}

/// One `publishManyNostrGiftWraps`: the same EVENT frames to every selected relay, one
/// [`PublishJudge`] per relay, then the counts.
#[derive(Debug, Clone)]
pub struct PublishMany {
    relays: Vec<String>,
    judges: Vec<PublishJudge>,
}

/// Starts `publishManyNostrGiftWraps(giftWraps, { relayUrls })`: the batch is checked first, then
/// the relay selection, as `publishWithPool` does.
pub fn publish_many_nostr_gift_wraps(
    gift_wraps: &[NostrGiftWrapDelivery],
    relay_urls: &[String],
    limits: RelayLimits,
) -> Result<PublishMany, RelayError> {
    let events = parsed_gift_wraps(gift_wraps)?;
    valid_relay_urls(relay_urls, limits.max_relays)?;
    Ok(PublishMany {
        relays: relay_urls.to_vec(),
        judges: vec![PublishJudge::new(events); relay_urls.len()],
    })
}

impl PublishMany {
    /// The selected relays, in order; `relay` below indexes this list.
    pub fn relay_urls(&self) -> &[String] {
        &self.relays
    }

    /// The EVENT frames to send to each relay.
    pub fn event_frames(&self) -> Vec<String> {
        self.judges
            .first()
            .map(PublishJudge::event_frames)
            .unwrap_or_default()
    }

    /// Judges one frame from relay `relay`; a frame for a relay outside the list finishes it.
    pub fn accept_frame(&mut self, relay: usize, frame: &str) -> RelayStep {
        match self.judges.get_mut(relay) {
            Some(judge) => judge.accept_frame(frame),
            None => RelayStep::Finish { close_frame: None },
        }
    }

    /// The acknowledgement count of each wrap, in delivery order, once every relay has finished,
    /// failed or timed out.
    pub fn acknowledgements(&self) -> Result<Vec<u32>, RelayError> {
        let per_relay: Vec<Vec<bool>> = self.judges.iter().map(PublishJudge::accepted).collect();
        publish_counts(&per_relay)
    }
}

#[cfg(test)]
#[path = "relay_tests.rs"]
mod tests;
