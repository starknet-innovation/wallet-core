//! The channels a proof job sends through: the SDK's `discoverChannels(address, vk, recipients,
//! {blockIdentifier})` against the indexer (`indexer-discovery.ts`), with no cursor, as Bramble's
//! fixed option set calls it (`compiler.ts` `resolveRecipientChannels`).
//!
//! Sans-IO: `body` writes the next request (the viewing key is lent for that one body), and
//! `on_reply` folds the service's reply in, returning the walk for another page or the channels.
//! Every page runs `accumulateOutgoingResponse`; the last builds the map (`buildChannelMap`)
//! keeping only the requested recipients, with `total` from the final cursor's
//! `total_n_channels`.
//!
//! The SDK's fallback when that total is absent is a second, "total-only" walk, which for the
//! indexer throws anyway once it completes without one. It is not ported: `total` is `None` and
//! the compiler (S3) refuses to open a channel without it.
//!
//! Stricter than the SDK, which trusts the reply: every field is read into a fixed shape, an
//! address keeps one spelling for the whole walk (the SDK keys its maps by the text), channel keys
//! are nonzero, indices are `u32`, and a later page must answer from the first page's block.

/// The path the walk's bodies are posted to, appended to the discovery URL.
pub use super::bodies::OUTGOING_PATH;
use super::bodies::{self, BlockRef};
use super::discovery::{
    channel_key, felt_of, felt_str, index, judge_page, pin_block, request_block, same_spelling,
    AddressMap, Channel, TokenChannel, Walked,
};
use super::json::{js_bigint, Ordered};
use super::notes::{array, cursor_complete, flag};
use super::plan::MAX_DISCOVERY_PAGES;
use super::types::PrivacyFailure;
use crate::{Felt, SecretFelt};

/// What the walk found: the requested recipients' channels in the SDK's order, and the sender's
/// outgoing-channel count when the service gave one.
#[derive(Debug)]
pub struct OutgoingChannels {
    pub channels: AddressMap<Channel>,
    pub total: Option<u32>,
}

#[derive(Debug)]
struct Created {
    public_key: Felt,
    key: SecretFelt,
}

#[derive(Debug)]
struct Subchannel {
    spelling: String,
    last_index: Option<u32>,
}

/// One `outgoing_state` walk. Its `Debug` prints progress only: the cursor and the channels hold
/// channel keys in clear.
pub struct OutgoingWalk {
    pool: Felt,
    account: Felt,
    recipients: Vec<Felt>,
    proving_block: BlockRef,
    cursor: Ordered,
    block: Option<BlockRef>,
    pages: u32,
    spellings: AddressMap<String>,
    /// `createdChannelMap`.
    created: AddressMap<Created>,
    /// `nonCreatedChannelMap`; only the public key is ever read from it.
    precomputed: AddressMap<Felt>,
    /// `subchannelsByRecipient`.
    subchannels: AddressMap<AddressMap<Subchannel>>,
}

impl std::fmt::Debug for OutgoingWalk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutgoingWalk")
            .field("recipients", &self.recipients.len())
            .field("pages", &self.pages)
            .finish_non_exhaustive()
    }
}

impl OutgoingWalk {
    /// A walk for `account` in `pool` over `recipients` (sent in this order) at the proving block.
    pub fn new(pool: Felt, account: Felt, recipients: Vec<Felt>, proving_block: BlockRef) -> Self {
        // No registry, so nothing is resolved unless the list is empty (`allResolved` stays true).
        let cursor = Ordered::Object(vec![
            (
                "channel_discovery_complete".into(),
                Ordered::Bool(recipients.is_empty()),
            ),
            ("channels".into(), Ordered::Object(Vec::new())),
        ]);
        Self {
            pool,
            account,
            recipients,
            proving_block,
            cursor,
            block: None,
            pages: 0,
            spellings: AddressMap::default(),
            created: AddressMap::default(),
            precomputed: AddressMap::default(),
            subchannels: AddressMap::default(),
        }
    }

    /// The next `POST /v1/sync/outgoing_state` body (`OUTGOING_PATH`).
    pub fn body(&self, viewing_key: &SecretFelt) -> zeroize::Zeroizing<Vec<u8>> {
        let block = request_block(self.block.as_ref(), &self.proving_block);
        bodies::outgoing(
            &self.pool,
            &self.account,
            viewing_key,
            &self.cursor,
            block,
            &self.recipients,
        )
    }

    /// Folds the reply to the last body in.
    pub fn on_reply(
        mut self,
        status: u16,
        body: &[u8],
    ) -> Result<Walked<Self, OutgoingChannels>, PrivacyFailure> {
        let (reply, block) = judge_page(status, body)?;
        let invalid = PrivacyFailure::InvalidResponse;
        pin_block(&mut self.block, block).ok_or(invalid)?;
        self.fold(&reply).ok_or(invalid)?;
        let mut cursor = reply.get("cursor").cloned().ok_or(invalid)?;
        let total = check_cursor(&cursor).ok_or(invalid)?;
        let complete = cursor_complete(&cursor).ok_or(invalid)?;
        self.pages += 1;
        if complete {
            return self.finish(total).map(Walked::Done).ok_or(invalid);
        }
        if self.pages >= MAX_DISCOVERY_PAGES {
            return Err(invalid);
        }
        prune_complete_cursor(&mut cursor);
        self.cursor = cursor;
        Ok(Walked::More(self))
    }

    /// `accumulateOutgoingResponse`.
    fn fold(&mut self, reply: &Ordered) -> Option<()> {
        for channel in array(reply.get("channels"))? {
            let recipient = self.recipient(channel)?;
            let public_key = felt_of(js_bigint(channel.get("recipient_public_key")?)?)?;
            let key = channel_key(js_bigint(channel.get("channel_key")?)?)?;
            if flag(channel.get("precomputed"))? {
                // A precomputed entry never displaces a created one.
                if !self.created.contains_key(&recipient) {
                    self.precomputed.insert(recipient, public_key);
                }
            } else {
                self.created.insert(recipient, Created { public_key, key });
                self.precomputed.remove(&recipient);
            }
        }
        for sub in array(reply.get("subchannels"))? {
            let recipient = self.recipient(sub)?;
            let (spelling, token) = felt_str(sub.get("token"))?;
            let last_index = match sub.get("last_note_index")? {
                Ordered::Null => None,
                value => Some(index(Some(value))?),
            };
            let tokens = self
                .subchannels
                .get_or_insert_with(recipient, AddressMap::default);
            match tokens.get_mut(&token) {
                Some(existing) if existing.spelling != spelling => return None,
                // Non-null overrides null; null never overrides.
                Some(existing) => existing.last_index = last_index.or(existing.last_index),
                None => tokens.insert(
                    token,
                    Subchannel {
                        spelling: spelling.to_owned(),
                        last_index,
                    },
                ),
            }
        }
        Some(())
    }

    fn recipient(&mut self, entry: &Ordered) -> Option<Felt> {
        let (spelling, recipient) = felt_str(entry.get("recipient_addr"))?;
        same_spelling(&mut self.spellings, spelling, recipient)?;
        Some(recipient)
    }

    /// `buildChannelMap`, then the requested-recipient filter.
    fn finish(self, total: Option<u32>) -> Option<OutgoingChannels> {
        let mut channels = AddressMap::with_capacity(self.created.len() + self.precomputed.len());
        for (recipient, created) in self.created.iter() {
            let mut tokens = AddressMap::default();
            if let Some(subs) = self.subchannels.get(recipient) {
                for (token_index, (token, sub)) in subs.iter().enumerate() {
                    let note_nonce = match sub.last_index {
                        Some(last) => last.checked_add(1)?,
                        None => 0,
                    };
                    let token_index = u32::try_from(token_index).ok()?;
                    tokens.insert(
                        *token,
                        TokenChannel {
                            token_index,
                            note_nonce,
                        },
                    );
                }
            }
            let channel = Channel {
                public_key: created.public_key,
                key: Some(created.key.clone()),
                tokens,
            };
            channels.insert(*recipient, channel);
        }
        for (recipient, public_key) in self.precomputed.iter() {
            if !channels.contains_key(recipient) {
                let channel = Channel {
                    public_key: *public_key,
                    key: None,
                    tokens: AddressMap::default(),
                };
                channels.insert(*recipient, channel);
            }
        }
        channels.retain(|recipient, _| self.recipients.contains(recipient));
        Some(OutgoingChannels { channels, total })
    }
}

/// A reply's cursor, checked before it is echoed or pruned: the flags the SDK tests are booleans
/// (or absent), the channel and subchannel maps are objects, no key is one `JSON.stringify` would
/// reorder, and `total_n_channels` is a `u32` when present.
fn check_cursor(cursor: &Ordered) -> Option<Option<u32>> {
    let Ordered::Object(_) = cursor else {
        return None;
    };
    if cursor.has_index_key() {
        return None;
    }
    flag(cursor.get("channel_discovery_complete"))?;
    let total = match cursor.get("total_n_channels") {
        None | Some(Ordered::Null) => None,
        value => Some(index(value)?),
    };
    let channels = match cursor.get("channels") {
        None | Some(Ordered::Null) => return Some(total),
        Some(Ordered::Object(channels)) => channels,
        Some(_) => return None,
    };
    for (_, channel) in channels {
        let Ordered::Object(_) = channel else {
            return None;
        };
        flag(channel.get("subchannel_discovery_complete"))?;
        let subs = match channel.get("subchannels") {
            None | Some(Ordered::Null) => continue,
            Some(Ordered::Object(subs)) => subs,
            Some(_) => return None,
        };
        for (_, sub) in subs {
            let Ordered::Object(_) = sub else {
                return None;
            };
            flag(sub.get("note_discovery_complete"))?;
        }
    }
    Some(total)
}

fn is_true(value: Option<&Ordered>) -> bool {
    matches!(value, Some(Ordered::Bool(true)))
}

fn member_mut<'a>(value: &'a mut Ordered, key: &str) -> Option<&'a mut Ordered> {
    match value {
        Ordered::Object(members) => members.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v),
        _ => None,
    }
}

/// `pruneCompleteCursor`: in each channel whose subchannel discovery is complete and that has a
/// `subchannels` object, drop the subchannels whose notes are complete, then the channel if none
/// is left.
fn prune_complete_cursor(cursor: &mut Ordered) {
    let Some(Ordered::Object(channels)) = member_mut(cursor, "channels") else {
        return;
    };
    channels.retain_mut(|(_, channel)| {
        if !is_true(channel.get("subchannel_discovery_complete")) {
            return true;
        }
        let Some(Ordered::Object(subs)) = member_mut(channel, "subchannels") else {
            return true;
        };
        subs.retain(|(_, sub)| !is_true(sub.get("note_discovery_complete")));
        !subs.is_empty()
    });
}

#[cfg(test)]
#[path = "outgoing_tests.rs"]
mod tests;
