//! One gift-wrap sync across the selected relays, judged frame by frame and paged as the
//! extension's `syncWithPool` and `unseenRelayEvents` page (`nostr-relay.ts`).
//!
//! Each relay is asked for the newest `MAX_BATCH_ITEMS` wraps. While an answer comes back full,
//! older wraps may hide behind newer ones (a burst published to push them out of the window), so
//! the relay is asked again below the oldest wrap it returned, up to `max_sync_pages` pages, until
//! enough wraps the caller has not seen are found. Seen wraps are skipped before that bound, so
//! wraps the wallet already finished with never crowd out ones it has not opened.
//!
//! As the extension: an oversized, non-JSON, non-array or over-long frame is ignored and the
//! subscription stays open; every EVENT frame counts toward the 50-frame close, accepted or not;
//! a page that ends early (CLOSED, timeout, socket error, close) keeps the events it already
//! delivered and ends that relay, and one that ends with none fails the relay, pages before it
//! included.

use std::collections::HashSet;

use serde_json::{json, Map, Value};

use super::relay::{parse_frame, valid_relay_urls, RelayError, RelayLimits, RelayStep};
use super::{
    event_from_value, has_recipient_tag, is_public_key, NostrEvent, GIFT_WRAP_KIND,
    MAX_BATCH_ITEMS, MAX_SAFE_INTEGER,
};

#[derive(Debug, Clone, PartialEq, Eq)]
enum RelayState {
    Open,
    Finished,
    Failed,
}

#[derive(Debug, Clone)]
struct RelaySync {
    state: RelayState,
    /// The connection's subscription counter; the open page is `mcw-<sequence in base 36>`.
    sequence: u64,
    /// Pages requested so far.
    pages: usize,
    until: Option<i64>,
    /// EVENT frames on the open page.
    frames: usize,
    /// Wraps the open page delivered for the recipient.
    events: Vec<NostrEvent>,
    /// `unseen`: unseen wraps across pages, first sighting kept, in arrival order.
    unseen: Vec<NostrEvent>,
}

/// One sync, from [`sync_nostr_gift_wraps`].
#[derive(Debug, Clone)]
pub struct SyncJudge {
    recipient: String,
    since: i64,
    relays: Vec<String>,
    seen: HashSet<String>,
    max_sync_pages: usize,
    syncs: Vec<RelaySync>,
}

/// Starts the transport's `sync(recipientPublicKey, since, relayUrls, seenEventIds)`: the
/// recipient must be canonical and `since` a non-negative safe integer ("Invalid Nostr sync
/// request"), then the relay selection is checked.
pub fn sync_nostr_gift_wraps(
    recipient_public_key: &str,
    since: i64,
    relay_urls: Vec<String>,
    seen_event_ids: HashSet<String>,
    limits: RelayLimits,
) -> Result<SyncJudge, RelayError> {
    let safe_since = u64::try_from(since).is_ok_and(|since| since <= MAX_SAFE_INTEGER);
    if !is_public_key(recipient_public_key) || !safe_since {
        return Err(RelayError::InvalidSyncRequest);
    }
    valid_relay_urls(&relay_urls, limits.max_relays)?;
    let first = RelaySync {
        state: RelayState::Open,
        sequence: 1,
        pages: 1,
        until: None,
        frames: 0,
        events: Vec::new(),
        unseen: Vec::new(),
    };
    Ok(SyncJudge {
        recipient: recipient_public_key.to_string(),
        since,
        syncs: vec![first; relay_urls.len()],
        relays: relay_urls,
        seen: seen_event_ids,
        max_sync_pages: limits.max_sync_pages,
    })
}

/// `Number.prototype.toString(36)` for a counter.
fn base36(mut value: u64) -> String {
    let mut digits = Vec::new();
    loop {
        digits.push(b"0123456789abcdefghijklmnopqrstuvwxyz"[(value % 36) as usize]);
        value /= 36;
        if value == 0 {
            break;
        }
    }
    digits
        .iter()
        .rev()
        .map(|&digit| char::from(digit))
        .collect()
}

fn subscription_id(sequence: u64) -> String {
    format!("mcw-{}", base36(sequence))
}

fn close_frame(sequence: u64) -> String {
    json!(["CLOSE", subscription_id(sequence)]).to_string()
}

/// `nextPageUntil`: the `until` for the next page, or `None` when no older page can be read. A
/// full page from one second would repeat forever, so it steps below it.
fn next_page_until(events: &[NostrEvent], until: Option<i64>, since: i64) -> Option<i64> {
    let oldest = events
        .iter()
        .filter_map(|event| i64::try_from(event.created_at).ok())
        .min()?;
    let next = match until {
        Some(until) if oldest >= until => until - 1,
        _ => oldest,
    };
    (next >= since).then_some(next)
}

impl SyncJudge {
    /// The selected relays, in order; `relay` below indexes this list.
    pub fn relay_urls(&self) -> &[String] {
        &self.relays
    }

    fn request_frame(&self, sequence: u64, until: Option<i64>) -> String {
        let mut filter = Map::new();
        filter.insert("#p".into(), json!([self.recipient]));
        filter.insert("kinds".into(), json!([GIFT_WRAP_KIND]));
        filter.insert("limit".into(), json!(MAX_BATCH_ITEMS));
        filter.insert("since".into(), json!(self.since));
        if let Some(until) = until {
            filter.insert("until".into(), json!(until));
        }
        json!(["REQ", subscription_id(sequence), Value::Object(filter)]).to_string()
    }

    /// The first page's REQ frame, the same for every relay on a fresh connection.
    pub fn open_frame(&self) -> String {
        self.request_frame(1, None)
    }

    fn add_unseen(seen: &HashSet<String>, sync: &mut RelaySync) {
        for event in std::mem::take(&mut sync.events) {
            if !seen.contains(&event.id) && !sync.unseen.iter().any(|kept| kept.id == event.id) {
                sync.unseen.push(event);
            }
        }
    }

    // The open page ended in full (EOSE or the frame budget): keep its unseen wraps and ask for
    // the next page while the answer was full, too few unseen wraps were found, the page cap
    // allows and an older page exists.
    fn page_ended(&mut self, relay: usize) -> RelayStep {
        let (since, max_pages) = (self.since, self.max_sync_pages);
        let sync = &mut self.syncs[relay];
        let close = close_frame(sync.sequence);
        let full = sync.events.len() >= MAX_BATCH_ITEMS;
        let next = next_page_until(&sync.events, sync.until, since);
        Self::add_unseen(&self.seen, sync);
        let next =
            next.filter(|_| full && sync.unseen.len() < MAX_BATCH_ITEMS && sync.pages < max_pages);
        let Some(until) = next else {
            sync.state = RelayState::Finished;
            return RelayStep::Finish {
                close_frame: Some(close),
            };
        };
        sync.until = Some(until);
        sync.sequence += 1;
        sync.pages += 1;
        sync.frames = 0;
        let sequence = sync.sequence;
        RelayStep::NextPage {
            close_frame: close,
            request: self.request_frame(sequence, Some(until)),
        }
    }

    /// Judges one frame from relay `relay`.
    pub fn accept_frame(&mut self, relay: usize, frame: &str) -> RelayStep {
        let Some(sync) = self.syncs.get_mut(relay) else {
            return RelayStep::Finish { close_frame: None };
        };
        if sync.state != RelayState::Open {
            return RelayStep::Finish { close_frame: None };
        }
        let Some(parts) = parse_frame(frame) else {
            return RelayStep::Continue;
        };
        if parts.get(1).and_then(Value::as_str) != Some(subscription_id(sync.sequence).as_str()) {
            return RelayStep::Continue;
        }
        match parts.first().and_then(Value::as_str) {
            Some("EOSE") => self.page_ended(relay),
            Some("CLOSED") => {
                self.end(relay);
                RelayStep::Finish { close_frame: None }
            }
            Some("EVENT") => {
                sync.frames += 1;
                if let Some(event) = parts.get(2).and_then(event_from_value) {
                    if event.kind == GIFT_WRAP_KIND && has_recipient_tag(&event, &self.recipient) {
                        sync.events.push(event);
                    }
                }
                if sync.frames >= MAX_BATCH_ITEMS {
                    return self.page_ended(relay);
                }
                RelayStep::Continue
            }
            _ => RelayStep::Continue,
        }
    }

    fn end(&mut self, relay: usize) {
        let Some(sync) = self.syncs.get_mut(relay) else {
            return;
        };
        if sync.state == RelayState::Open {
            if sync.events.is_empty() {
                sync.state = RelayState::Failed;
            } else {
                Self::add_unseen(&self.seen, sync);
                sync.state = RelayState::Finished;
            }
        }
    }

    /// The relay timed out, errored, or closed: the open page keeps its events if it has any,
    /// and the relay fails if it has none.
    pub fn relay_ended(&mut self, relay: usize) {
        self.end(relay);
    }

    /// The sync's result: each relay's unseen wraps, round-robin up to `MAX_BATCH_ITEMS`, then
    /// newest first and by id. A relay still open is ended first. Fails only when every relay
    /// failed ("Nostr relays are unavailable").
    pub fn candidates(&mut self) -> Result<Vec<NostrEvent>, RelayError> {
        for relay in 0..self.syncs.len() {
            self.end(relay);
        }
        let successful: Vec<&Vec<NostrEvent>> = self
            .syncs
            .iter()
            .filter(|sync| sync.state == RelayState::Finished)
            .map(|sync| &sync.unseen)
            .collect();
        if successful.is_empty() {
            return Err(RelayError::RelaysUnavailable);
        }
        let mut candidates = Vec::new();
        'rounds: for index in 0.. {
            let mut added = false;
            for events in &successful {
                if let Some(event) = events.get(index) {
                    candidates.push(event.clone());
                    added = true;
                    if candidates.len() == MAX_BATCH_ITEMS {
                        break 'rounds;
                    }
                }
            }
            if !added {
                break;
            }
        }
        candidates.sort_by(|left, right| {
            right
                .created_at
                .cmp(&left.created_at)
                .then_with(|| left.id.cmp(&right.id))
        });
        Ok(candidates)
    }
}

#[cfg(test)]
#[path = "sync_tests.rs"]
mod tests;
