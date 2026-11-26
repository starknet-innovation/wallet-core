//! The notes a proof job may spend: the SDK's `discoverNotes(address, vk, {tokens,
//! blockIdentifier})` against the indexer (`indexer-discovery.ts`), with no cursor, as Bramble's
//! fixed option set calls it (`compiler.ts` `resolveNotes`, for the tokens in deficit).
//!
//! Sans-IO, like `outgoing`. Each page runs `convertIncomingNotes`: a note's channel key comes
//! from this reply's `channels` (matched on the sender as written) or else from an earlier page's
//! cursor (matched by value); notes of tokens not asked for are skipped; `open` is
//! `salt === "1"`. Notes come back by token in first-seen order, each token's in arrival order,
//! which naive selection (S3) depends on. The cursor is echoed unpruned, as the SDK does.
//!
//! Stricter than the SDK, which trusts the reply: on top of the balance read's checks
//! (`notes::read_note`), amounts fit `u128`, `index` is a `u32`, `note_id` and `salt` are felts, a
//! note id appears once, a salt of 1 is spelled `"1"`, a channel key is nonzero and present (the
//! SDK reads a cursor channel without one as key `0n`), one page names a sender once and in one
//! spelling, at most `max_notes` nonzero notes are kept per token, and a later page must answer
//! from the first page's block. Zero-amount notes are checked like any other, then dropped: naive
//! selection takes the largest first, so one only ever joins a selection that is short anyway.

use std::collections::BTreeSet;

/// The path the walk's bodies are posted to, appended to the discovery URL.
pub use super::bodies::INCOMING_PATH;
use super::bodies::{self, BlockRef};
use super::discovery::{
    channel_key, felt_of, felt_str, index, judge_page, pin_block, request_block, AddressMap, Note,
    Walked, Witness,
};
use super::json::{js_bigint, Ordered};
use super::notes::{array, cursor_channels, cursor_complete, page_channels, read_note};
use super::plan::MAX_DISCOVERY_PAGES;
use super::types::PrivacyFailure;
use crate::{Felt, SecretFelt};

/// One `incoming_state` walk.
#[derive(Debug)]
pub struct IncomingWalk {
    pool: Felt,
    account: Felt,
    tokens: Vec<Felt>,
    proving_block: BlockRef,
    max_notes: usize,
    cursor: Option<Ordered>,
    block: Option<BlockRef>,
    pages: u32,
    /// `incomingChannels`: each sender an earlier cursor named, with its channel key if it had one.
    known: AddressMap<Option<SecretFelt>>,
    notes: AddressMap<Vec<Note>>,
    ids: BTreeSet<Felt>,
}

impl IncomingWalk {
    /// A walk for `account` in `pool`, keeping the notes of `tokens` (at most `max_notes` nonzero
    /// ones each), at the proving block.
    pub fn new(
        pool: Felt,
        account: Felt,
        tokens: Vec<Felt>,
        proving_block: BlockRef,
        max_notes: usize,
    ) -> Self {
        Self {
            pool,
            account,
            tokens,
            proving_block,
            max_notes,
            cursor: None,
            block: None,
            pages: 0,
            known: AddressMap::default(),
            notes: AddressMap::default(),
            ids: BTreeSet::new(),
        }
    }

    /// The next `POST /v1/sync/incoming_state` body (`INCOMING_PATH`): `cursor: {}` and
    /// the proving block first, then the last reply's cursor and its block.
    pub fn body(&self, viewing_key: &SecretFelt) -> zeroize::Zeroizing<Vec<u8>> {
        let block = request_block(self.block.as_ref(), &self.proving_block);
        bodies::incoming(
            &self.pool,
            &self.account,
            viewing_key,
            self.cursor.as_ref(),
            Some(block),
        )
    }

    /// Folds the reply to the last body in.
    pub fn on_reply(
        mut self,
        status: u16,
        body: &[u8],
    ) -> Result<Walked<Self, AddressMap<Vec<Note>>>, PrivacyFailure> {
        let (reply, block) = judge_page(status, body)?;
        let invalid = PrivacyFailure::InvalidResponse;
        pin_block(&mut self.block, block).ok_or(invalid)?;
        self.fold(&reply).ok_or(invalid)?;
        let cursor = reply.get("cursor").cloned().ok_or(invalid)?;
        let complete = cursor_complete(&cursor).ok_or(invalid)?;
        self.pages += 1;
        if complete {
            return Ok(Walked::Done(self.notes));
        }
        if self.pages >= MAX_DISCOVERY_PAGES || cursor.has_index_key() {
            return Err(invalid);
        }
        self.cursor = Some(cursor);
        Ok(Walked::More(self))
    }

    fn fold(&mut self, reply: &Ordered) -> Option<()> {
        // `channelKeyMap`, keyed by value here, so one sender may appear once.
        let mut page: AddressMap<(&str, SecretFelt)> = AddressMap::default();
        for (spelling, key) in page_channels(reply)? {
            let sender = felt_of(js_bigint(&Ordered::Str(spelling.to_owned()))?)?;
            if page.contains_key(&sender) {
                return None;
            }
            page.insert(sender, (spelling, channel_key(key)?));
        }
        for note in array(reply.get("notes"))? {
            let fields = read_note(note)?;
            let token = felt_of(fields.token)?;
            if !self.tokens.contains(&token) {
                continue;
            }
            let sender = felt_of(fields.sender)?;
            let channel_key = match page.get(&sender) {
                Some((spelling, _)) if *spelling != fields.sender_text => return None,
                Some((_, key)) => key.clone(),
                None => self.known.get(&sender)?.clone()?,
            };
            let (_, id) = felt_str(note.get("note_id"))?;
            let nonce = index(note.get("index"))?;
            let r = felt_of(fields.salt)?;
            let open = matches!(note.get("salt"), Some(Ordered::Str(salt)) if salt == "1");
            let Some(Ordered::Int(_)) = note.get("block_number") else {
                return None;
            };
            let amount = (fields.amount.high == 0).then_some(fields.amount.low)?;
            if (r == Felt::ONE && !open) || !self.ids.insert(id) {
                return None;
            }
            if amount == 0 {
                continue;
            }
            let notes = self.notes.get_or_insert_with(token, Vec::new);
            if notes.len() >= self.max_notes {
                return None;
            }
            notes.push(Note {
                id,
                amount,
                witness: Witness {
                    channel_key,
                    nonce,
                    r: SecretFelt::new(r),
                },
                sender,
                open,
            });
        }
        // `apiCursorToNotesCursor`, merged after this page's notes, for later pages.
        for (sender, key) in cursor_channels(reply.get("cursor")?)? {
            let key = match key {
                Some(key) => Some(channel_key(key)?),
                None => None,
            };
            self.known.insert(felt_of(sender)?, key);
        }
        Some(())
    }
}

#[cfg(test)]
#[path = "incoming_tests.rs"]
mod tests;
