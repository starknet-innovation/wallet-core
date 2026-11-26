//! What a proof job learns from discovery (P15, the SDK's `channel.ts` and `interfaces.ts` `Note`),
//! and the reply handling both walks share (`outgoing::OutgoingWalk`, `incoming::IncomingWalk`).
//!
//! Channel keys and witnesses are derived from the viewing key, so the ones read out of a reply
//! are held as `SecretFelt` (zeroized on drop, redacted `Debug`). The parsed reply and the cursor
//! a walk echoes hold the same keys as plain text (`privacy`'s module doc). A `Note`'s `Debug`
//! prints nothing at all: amounts and senders are private too. Failures are fixed
//! `PrivacyFailure` variants and carry none of it.

use std::fmt;

use super::bodies::BlockRef;
use super::json::{js_bigint, Ordered, FIELD_PRIME};
use super::judge;
use super::types::PrivacyFailure;
use crate::send::u256::U256;
use crate::{Felt, SecretFelt};

/// The SDK's `AddressMap`: keyed by address value, iterated in insertion order. As with a
/// JavaScript `Map`, setting a key that is already present keeps its position, and removing one
/// forgets it.
///
/// Growing one reallocates, which frees the old block with the moved entries' bytes still in it:
/// `SecretFelt` wipes on drop, not on a move. Where the size is known it is allocated once
/// (`with_capacity`); where the service decides it (the incoming walk's cursor senders and their
/// channel keys) those keys can be left behind in freed memory until it is reused.
// ponytail: linear lookups; the maps hold a job's recipients and tokens, a handful of entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddressMap<V> {
    entries: Vec<(Felt, V)>,
}

impl<V> Default for AddressMap<V> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}

impl<V> AddressMap<V> {
    /// An empty map with room for `capacity` entries, for a caller that knows its size up front.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
        }
    }

    fn position(&self, key: &Felt) -> Option<usize> {
        self.entries.iter().position(|(k, _)| k == key)
    }

    pub fn get(&self, key: &Felt) -> Option<&V> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn get_mut(&mut self, key: &Felt) -> Option<&mut V> {
        self.entries
            .iter_mut()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v)
    }

    pub fn contains_key(&self, key: &Felt) -> bool {
        self.position(key).is_some()
    }

    /// `map.set(key, value)`.
    pub fn insert(&mut self, key: Felt, value: V) {
        match self.get_mut(&key) {
            Some(slot) => *slot = value,
            None => self.entries.push((key, value)),
        }
    }

    /// `map.get(key)` on a map with a default factory, which inserts the default when missing.
    pub fn get_or_insert_with(&mut self, key: Felt, default: impl FnOnce() -> V) -> &mut V {
        let index = match self.position(&key) {
            Some(index) => index,
            None => {
                self.entries.push((key, default()));
                self.entries.len() - 1
            }
        };
        &mut self.entries[index].1
    }

    pub fn remove(&mut self, key: &Felt) -> Option<V> {
        let index = self.position(key)?;
        Some(self.entries.remove(index).1)
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&Felt, &V) -> bool) {
        self.entries.retain(|(k, v)| keep(k, v));
    }

    pub fn iter(&self) -> impl Iterator<Item = (&Felt, &V)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }

    pub fn keys(&self) -> impl Iterator<Item = &Felt> {
        self.entries.iter().map(|(k, _)| k)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// `TokenChannel`: which subchannel a token is (`token_index`) and the next note index in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TokenChannel {
    pub token_index: u32,
    pub note_nonce: u32,
}

/// `Channel`: an outgoing channel to one recipient. `key` is `None` for a channel the service
/// only precomputed (not opened on chain yet); `tokens` are its subchannels, by token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    pub public_key: Felt,
    pub key: Option<SecretFelt>,
    pub tokens: AddressMap<TokenChannel>,
}

/// `Witness`: what proves a note is ours to spend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Witness {
    pub channel_key: SecretFelt,
    pub nonce: u32,
    pub r: SecretFelt,
}

/// `Note`: one unspent incoming note. `open` is the SDK's `salt === "1"`.
#[derive(Clone, PartialEq, Eq)]
pub struct Note {
    pub id: Felt,
    pub amount: u128,
    pub witness: Witness,
    pub sender: Felt,
    pub open: bool,
}

impl fmt::Debug for Note {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Note(***)")
    }
}

/// A walk after one reply: another page to post, or its result.
#[derive(Debug)]
pub enum Walked<W, T> {
    More(W),
    Done(T),
}

/// The SDK's `post`: HTTP 409 is its `ReorgError`, whatever the body says (the job restarts once,
/// as `compile` does, before any retry rule sees it); any other non-2xx is retryable only when the
/// body matches the extension's pattern; a 2xx must be bounded JSON with a `block_ref` in the
/// service's wire format.
pub(super) fn judge_page(status: u16, body: &[u8]) -> Result<(Ordered, BlockRef), PrivacyFailure> {
    if status == 409 {
        return Err(PrivacyFailure::Reorg);
    }
    if !(200..300).contains(&status) {
        let text = String::from_utf8_lossy(&body[..body.len().min(judge::MAX_RESPONSE_BYTES)]);
        return Err(if judge::matches_retry(&text) {
            PrivacyFailure::RateLimited
        } else {
            PrivacyFailure::ServiceUnavailable
        });
    }
    let reply = judge::parse_body(body).ok_or(PrivacyFailure::InvalidResponse)?;
    let block = judge::block_ref(&reply).ok_or(PrivacyFailure::InvalidResponse)?;
    Ok((reply, block))
}

/// Pins the first reply's `block_ref`; a later page answering from another block is refused, as
/// the balance read does (the SDK echoes whatever each reply says).
pub(super) fn pin_block(pinned: &mut Option<BlockRef>, block: BlockRef) -> Option<()> {
    match pinned {
        Some(first) if *first != block => None,
        Some(_) => Some(()),
        None => {
            *pinned = Some(block);
            Some(())
        }
    }
}

/// The `block_ref` a walk sends: `if (blockRef) … else if (blockIdentifier !== undefined)`, so
/// the proving block on the first page, and on later ones too when the pinned reply's block is
/// falsy (`0`; an empty string is not a block the judge accepts).
pub(super) fn request_block<'a>(
    pinned: Option<&'a BlockRef>,
    proving: &'a BlockRef,
) -> &'a BlockRef {
    match pinned {
        Some(BlockRef::Number(0)) | None => proving,
        Some(block) => block,
    }
}

/// A felt below `P`, from a value the SDK reads with `BigInt`.
pub(super) fn felt_of(value: U256) -> Option<Felt> {
    if value >= FIELD_PRIME {
        return None;
    }
    let mut bytes = [0u8; 32];
    bytes[..16].copy_from_slice(&value.high.to_be_bytes());
    bytes[16..].copy_from_slice(&value.low.to_be_bytes());
    Some(Felt::from_bytes_be(&bytes))
}

/// A channel key: a nonzero felt (`0n` is falsy wherever the SDK tests `channel.key`).
pub(super) fn channel_key(value: U256) -> Option<SecretFelt> {
    felt_of(value)
        .filter(|key| *key != Felt::ZERO)
        .map(SecretFelt::new)
}

/// A service felt (an address, a note id): a string `BigInt` reads, below `P`, with its
/// spelling.
pub(super) fn felt_str(value: Option<&Ordered>) -> Option<(&str, Felt)> {
    let Ordered::Str(text) = value? else {
        return None;
    };
    Some((text, felt_of(js_bigint(value?)?)?))
}

/// A `number` field the SDK uses as an index: a `u32`.
pub(super) fn index(value: Option<&Ordered>) -> Option<u32> {
    match value? {
        Ordered::Int(number) => u32::try_from(*number).ok(),
        _ => None,
    }
}

/// The SDK keys some maps by an address's text, so two spellings of one value would be two
/// entries there. Each value must keep the spelling it was first seen with.
pub(super) fn same_spelling(
    spellings: &mut AddressMap<String>,
    text: &str,
    value: Felt,
) -> Option<()> {
    match spellings.get(&value) {
        Some(first) if first != text => None,
        Some(_) => Some(()),
        None => {
            spellings.insert(value, text.to_owned());
            Some(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::privacy::client_actions::ClientAction;
    use crate::privacy::compiler::Compiled;
    use crate::privacy::job::ProofInputs;
    use crate::privacy::proof::{ProofLimits, ReviewedAction};
    use crate::privacy::prover::ProverRetry;

    #[test]
    fn address_map_keeps_javascript_map_order() {
        let mut map = AddressMap::default();
        map.insert(Felt::THREE, "a");
        map.insert(Felt::ONE, "b");
        map.insert(Felt::THREE, "c");
        assert_eq!(map.iter().map(|(_, v)| *v).collect::<Vec<_>>(), ["c", "b"]);
        assert_eq!(*map.get_or_insert_with(Felt::TWO, || "d"), "d");
        assert_eq!(*map.get_or_insert_with(Felt::ONE, || "e"), "b");
        assert_eq!(map.remove(&Felt::THREE), Some("c"));
        map.insert(Felt::THREE, "f");
        assert_eq!(
            map.keys().copied().collect::<Vec<_>>(),
            [Felt::ONE, Felt::TWO, Felt::THREE]
        );
        assert_eq!(map.len(), 3);
    }

    #[test]
    fn secrets_never_reach_debug() {
        let note = Note {
            id: Felt::ONE,
            amount: 123_456,
            witness: Witness {
                channel_key: SecretFelt::new(Felt::from(0xabcdefu64)),
                nonce: 1,
                r: SecretFelt::new(Felt::from(0xfedcbau64)),
            },
            sender: Felt::TWO,
            open: false,
        };
        assert_eq!(format!("{note:?}"), "Note(***)");
        let witness = format!("{:?}", note.witness);
        assert!(!witness.contains("abcdef") && !witness.contains("11259375"));
        let channel = Channel {
            public_key: Felt::ONE,
            key: Some(SecretFelt::new(Felt::from(0xabcdefu64))),
            tokens: AddressMap::default(),
        };
        assert!(!format!("{channel:?}").contains("abcdef"));

        // Everything that holds a service cursor, a reply body, a client action or the review:
        // a recognisable secret (`0xc0ffee`, 12648430) never prints.
        let secret = |debug: String| {
            assert!(
                !debug.contains("c0ffee") && !debug.contains("12648430"),
                "{debug}"
            );
        };
        let cursor = r#""cursor":{"channel_discovery_complete":false,"channels":{"0x5":{"channel_key":"0xc0ffee"}}}"#;
        let incoming =
            format!(r#"{{"block_ref":7000,"channels":[],"subchannels":[],"notes":[],{cursor}}}"#);
        let walk = super::super::incoming::IncomingWalk::new(
            Felt::ONE,
            Felt::TWO,
            vec![Felt::THREE],
            BlockRef::Number(7000),
            8,
        );
        let walked = walk.on_reply(200, incoming.as_bytes()).unwrap();
        assert!(matches!(walked, Walked::More(_)));
        secret(format!("{walked:?}"));
        let read = super::super::shadow_reads::ShadowCommitmentRead::new(Felt::ONE, Felt::TWO, 8);
        secret(format!("{:?}", read.on_reply(200, incoming.as_bytes())));
        let outgoing = format!(
            r#"{{"block_ref":7000,"channels":[{{"recipient_addr":"0x5","recipient_public_key":"0x7","channel_key":"0xc0ffee","precomputed":false}}],"subchannels":[],{cursor}}}"#
        );
        let walk = super::super::outgoing::OutgoingWalk::new(
            Felt::ONE,
            Felt::TWO,
            vec![Felt::from(5u8)],
            BlockRef::Number(7000),
        );
        let walked = walk.on_reply(200, outgoing.as_bytes()).unwrap();
        assert!(matches!(walked, Walked::More(_)));
        secret(format!("{walked:?}"));
        let parsed: Ordered = serde_json::from_str(&incoming).unwrap();
        secret(format!("{parsed:?}"));
        secret(format!(
            "{:?}",
            super::super::types::PrivacyReply::Http {
                id: 1,
                status: 200,
                body: incoming.into_bytes(),
                now_ms: 0,
            }
        ));

        let c0ffee = Felt::from(0xc0ffeeu64);
        let actions = vec![
            ClientAction::CreateEncNote {
                recipient_addr: c0ffee,
                recipient_public_key: c0ffee,
                token: c0ffee,
                amount: 0xc0ffee,
                index: 0xc0ffee,
                salt: 0xc0ffee,
            },
            ClientAction::Withdraw {
                to_addr: c0ffee,
                token: c0ffee,
                amount: 0xc0ffee,
                random: c0ffee,
            },
            ClientAction::OpenChannel {
                recipient_addr: c0ffee,
                index: 1,
                random: c0ffee,
                salt: c0ffee,
            },
            ClientAction::InvokeExternal {
                contract_address: c0ffee,
                calldata: vec![c0ffee; 3],
            },
        ];
        let compiled = Compiled {
            actions,
            user_linkage: false,
        };
        let debug = format!("{compiled:?}");
        secret(debug.clone());
        assert!(
            debug.contains("InvokeExternal { calldata: [3 felts], .. }"),
            "{debug}"
        );

        let inputs = ProofInputs {
            account: Felt::ONE,
            public_key: Felt::TWO,
            actions: vec![ReviewedAction::Transfer {
                token: Felt::THREE,
                recipient: c0ffee,
                amount: c0ffee,
            }],
            proving_block: 7000,
            route: super::super::config::testing::mainnet(false),
            prover_url: "https://p.example".into(),
            anonymizer: None,
            limits: ProofLimits::CEILING,
            max_notes: 8,
            retry: ProverRetry {
                max_retries: 0,
                request_timeout_ms: 1,
                base_delay_ms: 1,
                max_delay_ms: 1,
            },
        };
        secret(format!("{inputs:?}"));
    }

    #[test]
    fn values_and_blocks() {
        assert_eq!(felt_of(FIELD_PRIME), None);
        let below = FIELD_PRIME.checked_sub(U256::from_u128(1)).unwrap();
        assert_eq!(felt_of(below), Some(Felt::MAX));
        assert!(channel_key(U256::ZERO).is_none());
        let proving = BlockRef::Number(7000);
        assert_eq!(request_block(None, &proving), &proving);
        assert_eq!(
            request_block(Some(&BlockRef::Number(0)), &proving),
            &proving
        );
        let pinned = BlockRef::Text("0x5a1f00d".into());
        assert_eq!(request_block(Some(&pinned), &proving), &pinned);
        assert_eq!(
            judge_page(409, b"rate limit").unwrap_err(),
            PrivacyFailure::Reorg
        );
        assert_eq!(
            judge_page(500, b"Too Many Requests").unwrap_err(),
            PrivacyFailure::RateLimited
        );
        assert_eq!(
            judge_page(429, b"").unwrap_err(),
            PrivacyFailure::ServiceUnavailable
        );
        assert_eq!(
            judge_page(200, b"{}").unwrap_err(),
            PrivacyFailure::InvalidResponse
        );
        let mut spellings = AddressMap::default();
        assert!(same_spelling(&mut spellings, "0x5", Felt::from(5u8)).is_some());
        assert!(same_spelling(&mut spellings, "0x5", Felt::from(5u8)).is_some());
        assert!(same_spelling(&mut spellings, "0x05", Felt::from(5u8)).is_none());
    }
}
