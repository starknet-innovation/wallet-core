//! The notes a proof job may spend: the SDK's `discoverNotes(address, vk, {tokens,
//! blockIdentifier})` against the indexer (`indexer-discovery.ts`), with no cursor, as Bramble's
//! fixed option set calls it (`compiler.ts` `resolveNotes`, for the tokens in deficit).
//!
//! The shadow-account commitment read runs the same walk as `discoverNotes()` with no options
//! (`IncomingWalk::unfiltered`): no proving block, every token's notes.
//!
//! Sans-IO, like `outgoing`. Each page runs `convertIncomingNotes`: a note's channel key comes
//! from this reply's `channels` (matched on the sender as written) or else from an earlier page's
//! cursor (matched by value); notes of tokens not asked for are skipped; `open` is
//! `salt === "1"`. Notes come back by token in first-seen order, each token's in arrival order,
//! which naive selection (S3) depends on. The cursor is echoed unpruned, as the SDK does.
//!
//! Stricter than the SDK, which trusts the reply: on top of the balance read's checks
//! (`notes::read_note`), amounts fit `u128`, `index` is a `u32`, `note_id` and `salt` are felts, a
//! note id appears once in a page and never again beside a kept note, a salt of 1 is spelled
//! `"1"`, a channel key is nonzero and present (the SDK reads a cursor channel without one as key
//! `0n`), one page names a sender once and in one spelling, and a later page must answer from the
//! first page's block. Zero-amount notes are checked like any other, then dropped: naive
//! selection takes the largest first, so one only ever joins a selection that is short anyway.
//!
//! Dust. Anyone can send the account notes, so a token's notes are not capped but culled: the walk
//! keeps the `max_notes` largest nonzero ones (ties to the earlier arrival, at most
//! `MAX_ACTIONS`, what one batch could spend) and adds up the rest as `dropped`. Naive selection
//! over the kept notes, largest first in arrival order, is the SDK's selection over all of them
//! whenever that one fits in `max_notes`; when it does not, the compiler sees `dropped` and refuses
//! `TooManyNotes` (consolidate) rather than `InsufficientBalance`. Ceiling: a note id is checked
//! against this page and the kept notes, not against notes dropped on earlier pages, so that a
//! service repeating a dropped note can only inflate `dropped`.

use std::cmp::Reverse;
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
use super::pool_actions::MAX_ACTIONS;
use super::types::PrivacyFailure;
use crate::send::u256::U256;
use crate::{Felt, SecretFelt};

/// What the walk found: per token (first-seen order), its kept notes in arrival order, and the
/// total of the nonzero notes it dropped for not being among the `max_notes` largest.
#[derive(Debug, Default)]
pub struct SpendableNotes {
    pub notes: AddressMap<Vec<Note>>,
    pub dropped: AddressMap<U256>,
}

impl From<AddressMap<Vec<Note>>> for SpendableNotes {
    /// Notes with none dropped.
    fn from(notes: AddressMap<Vec<Note>>) -> Self {
        Self {
            notes,
            dropped: AddressMap::default(),
        }
    }
}

/// One `incoming_state` walk. Its `Debug` prints progress only: the cursor holds channel keys in
/// clear, and the notes are private.
pub struct IncomingWalk {
    pool: Felt,
    account: Felt,
    /// `None`: every token's notes (no `tokens` filter).
    tokens: Option<Vec<Felt>>,
    /// `None`: no `blockIdentifier`, so the first page names no block.
    proving_block: Option<BlockRef>,
    max_notes: usize,
    cursor: Option<Ordered>,
    block: Option<BlockRef>,
    pages: u32,
    /// `incomingChannels`: each sender an earlier cursor named, with its channel key if it had one.
    /// Sized by the service, so it grows by reallocation (see `AddressMap`).
    known: AddressMap<Option<SecretFelt>>,
    notes: AddressMap<Vec<Note>>,
    dropped: AddressMap<U256>,
    /// The kept notes' ids.
    ids: BTreeSet<Felt>,
}

impl std::fmt::Debug for IncomingWalk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IncomingWalk")
            .field("pages", &self.pages)
            .finish_non_exhaustive()
    }
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
            tokens: Some(tokens),
            proving_block: Some(proving_block),
            max_notes: max_notes.min(MAX_ACTIONS),
            cursor: None,
            block: None,
            pages: 0,
            known: AddressMap::default(),
            notes: AddressMap::default(),
            dropped: AddressMap::default(),
            ids: BTreeSet::new(),
        }
    }

    /// `discoverNotes(address, vk)` with no options: every token's notes (at most `max_notes`
    /// nonzero ones each), at whatever block the service answers from.
    pub fn unfiltered(pool: Felt, account: Felt, max_notes: usize) -> Self {
        Self {
            tokens: None,
            proving_block: None,
            ..Self::new(pool, account, Vec::new(), BlockRef::Number(0), max_notes)
        }
    }

    /// The next `POST /v1/sync/incoming_state` body (`INCOMING_PATH`): `cursor: {}` and
    /// the proving block first, then the last reply's cursor and its block. With no proving
    /// block, the first page names none and later ones the pinned block when it is truthy.
    pub fn body(&self, viewing_key: &SecretFelt) -> zeroize::Zeroizing<Vec<u8>> {
        let block = match &self.proving_block {
            Some(proving) => Some(request_block(self.block.as_ref(), proving)),
            None => self
                .block
                .as_ref()
                .filter(|block| **block != BlockRef::Number(0)),
        };
        bodies::incoming(
            &self.pool,
            &self.account,
            viewing_key,
            self.cursor.as_ref(),
            block,
        )
    }

    /// Folds the reply to the last body in.
    pub fn on_reply(
        mut self,
        status: u16,
        body: &[u8],
    ) -> Result<Walked<Self, SpendableNotes>, PrivacyFailure> {
        let (reply, block) = judge_page(status, body)?;
        let invalid = PrivacyFailure::InvalidResponse;
        pin_block(&mut self.block, block).ok_or(invalid)?;
        self.fold(&reply).ok_or(invalid)?;
        let cursor = reply.get("cursor").cloned().ok_or(invalid)?;
        let complete = cursor_complete(&cursor).ok_or(invalid)?;
        self.pages += 1;
        if complete {
            return Ok(Walked::Done(SpendableNotes {
                notes: self.notes,
                dropped: self.dropped,
            }));
        }
        if self.pages >= MAX_DISCOVERY_PAGES || cursor.has_index_key() {
            return Err(invalid);
        }
        self.cursor = Some(cursor);
        Ok(Walked::More(self))
    }

    fn fold(&mut self, reply: &Ordered) -> Option<()> {
        // `channelKeyMap`, keyed by value here, so one sender may appear once.
        let channels = page_channels(reply)?;
        let mut page: AddressMap<(&str, SecretFelt)> = AddressMap::with_capacity(channels.len());
        for (spelling, key) in channels {
            let sender = felt_of(js_bigint(&Ordered::Str(spelling.to_owned()))?)?;
            if page.contains_key(&sender) {
                return None;
            }
            page.insert(sender, (spelling, channel_key(key)?));
        }
        let mut page_ids = BTreeSet::new();
        for note in array(reply.get("notes"))? {
            let fields = read_note(note)?;
            let token = felt_of(fields.token)?;
            if self
                .tokens
                .as_ref()
                .is_some_and(|tokens| !tokens.contains(&token))
            {
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
            if (r == Felt::ONE && !open) || !page_ids.insert(id) || self.ids.contains(&id) {
                return None;
            }
            if amount == 0 {
                continue;
            }
            let note = Note {
                id,
                amount,
                witness: Witness {
                    channel_key,
                    nonce,
                    r: SecretFelt::new(r),
                },
                sender,
                open,
            };
            self.keep(token, note)?;
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

    /// Keeps `note` if it is among the token's `max_notes` largest so far (an equal amount loses
    /// to the earlier arrival), dropping the smallest, latest kept one to make room; whatever is
    /// dropped adds to `dropped`. The token's list is allocated once, at `max_notes`.
    fn keep(&mut self, token: Felt, note: Note) -> Option<()> {
        let max = self.max_notes;
        let notes = self
            .notes
            .get_or_insert_with(token, || Vec::with_capacity(max));
        let out = if notes.len() < max {
            None
        } else {
            let smallest = notes
                .iter()
                .enumerate()
                .min_by_key(|(at, kept)| (kept.amount, Reverse(*at)))
                .map(|(at, kept)| (at, kept.amount));
            match smallest {
                Some((at, amount)) if amount < note.amount => {
                    let evicted = notes.remove(at);
                    self.ids.remove(&evicted.id);
                    Some(evicted.amount)
                }
                _ => {
                    let amount = note.amount;
                    if notes.is_empty() {
                        self.notes.remove(&token);
                    }
                    let total = self.dropped.get_or_insert_with(token, || U256::ZERO);
                    *total = total.checked_add(U256::from_u128(amount))?;
                    return Some(());
                }
            }
        };
        self.ids.insert(note.id);
        notes.push(note);
        if let Some(amount) = out {
            let total = self.dropped.get_or_insert_with(token, || U256::ZERO);
            *total = total.checked_add(U256::from_u128(amount))?;
        }
        Some(())
    }
}

#[cfg(test)]
#[path = "incoming_tests.rs"]
mod tests;
