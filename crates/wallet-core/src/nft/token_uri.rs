//! A token's metadata URI (`rpc-assets.ts getNftTokenUri`): `token_uri`, then `tokenURI`, then
//! ERC-1155's `uri`, each reply read as a Cairo `ByteArray` or, failing that, an older contract's
//! felt array of short strings. Nothing is fetched here; `metadata::resolve_metadata_uri` takes
//! the URI from there.

use starknet_types_core::felt::Felt;

use super::read::{Ladder, ReadStep, Rung};
use super::reproof::u256_calldata;
use crate::js::big::parse_bigint;
use crate::js::felt::is_stark_field_element;
use crate::js::json::JsValue;
use crate::portfolio::token_meta::hex_pairs_to_chars;
use crate::portfolio::wire::{RpcReply, RpcRequest};
use crate::send::u256::U256;
use crate::Error;

/// The longest pending word a `ByteArray` may declare.
const MAX_PENDING_WORD_LEN: u128 = 30;
/// Bytes in a full `ByteArray` word.
const WORD_BYTES: usize = 31;
/// Index of `uri`, the ERC-1155 spelling whose `{id}` is substituted.
const ERC1155_URI: usize = 2;

/// One token's metadata-URI read. `ReadStep::Failed` is the extension's "Could not read NFT
/// metadata URI" (the card then says the art is unavailable).
#[derive(Debug, Clone)]
pub struct TokenUriRead {
    token_id: U256,
    max_uri_bytes: usize,
    ladder: Ladder,
}

impl TokenUriRead {
    /// A read whose URI may be up to `max_uri_bytes` long (`MAX_NFT_URI_BYTES`). Refuses a zero
    /// contract ("Invalid NFT contract address").
    pub fn new(contract: Felt, token_id: U256, max_uri_bytes: usize) -> Result<Self, Error> {
        if contract == Felt::ZERO {
            return Err(Error::invalid("Invalid NFT contract address"));
        }
        let token = u256_calldata(token_id).to_vec();
        Ok(Self {
            token_id,
            max_uri_bytes,
            ladder: Ladder::new(
                contract,
                vec![
                    ("token_uri", token.clone()),
                    ("tokenURI", token.clone()),
                    ("uri", token),
                ],
            ),
        })
    }

    /// The first call, under `id`; again on each new endpoint after `ReadStep::Transport`.
    pub fn start(&mut self, id: u64) -> RpcRequest {
        self.ladder.start(id)
    }

    /// Judges the reply to the last call; a next call, if any, goes under `next_id`. An empty URI
    /// counts as no answer.
    pub fn judge(&mut self, reply: &RpcReply, next_id: u64) -> ReadStep<String> {
        let answer = match self.ladder.rung(reply) {
            // `decodeShortStringArray` calls `response.every` on it and throws: next endpoint.
            Rung::Transport | Rung::NotArray => return ReadStep::Transport,
            Rung::Skip => return self.ladder.next(next_id),
            Rung::Answer(answer) => answer,
        };
        let uri = decode_byte_array(&answer, self.max_uri_bytes)
            .or_else(|| decode_short_string_array(&answer, self.max_uri_bytes))
            .filter(|uri| !uri.is_empty());
        match uri {
            Some(uri) if self.ladder.at() == ERC1155_URI => {
                let id = format!("{:032x}{:032x}", self.token_id.high, self.token_id.low);
                ReadStep::Done(uri.replace("{id}", &id))
            }
            Some(uri) => ReadStep::Done(uri),
            None => self.ladder.next(next_id),
        }
    }
}

// `BigInt(text)` as a non-negative integer.
fn non_negative(value: Option<&JsValue>) -> Option<crate::js::big::JsBigInt> {
    let parsed = parse_bigint(value?.as_str()?)?;
    (!parsed.negative || parsed.magnitude.is_zero()).then_some(parsed)
}

// starknet.js `stringFromByteArray` over one word: zero is empty, anything else is
// `decodeShortString(toHex(word))`, which throws on a negative.
fn word_text(value: Option<&JsValue>) -> Option<String> {
    let word = non_negative(value)?;
    if word.magnitude.is_zero() {
        return Some(String::new());
    }
    Some(hex_pairs_to_chars(&word.magnitude.to_hex()))
}

/// `decodeByteArray`: `[words, …words, pending_word, pending_word_len]` with the declared counts
/// checked against the reply and the URI's UTF-8 length bounded. `pending_word_len` is checked
/// but, as in starknet.js, not used to cut the pending word.
pub(super) fn decode_byte_array(answer: &[JsValue], max_bytes: usize) -> Option<String> {
    let words = non_negative(answer.first())?.magnitude.to_u128()?;
    let max_words = max_bytes.div_ceil(WORD_BYTES) as u128;
    if words > max_words || answer.len() as u128 != words + 3 {
        return None;
    }
    let words = words as usize;
    let pending_len = non_negative(answer.get(words + 2))?.magnitude.to_u128()?;
    if pending_len > MAX_PENDING_WORD_LEN {
        return None;
    }
    let mut uri = String::new();
    for index in 1..=words + 1 {
        uri.push_str(&word_text(answer.get(index))?);
    }
    (uri.len() <= max_bytes).then_some(uri)
}

// One non-empty, printable-ASCII Cairo short string (at most 31 bytes).
fn short_string_piece(felt: &Felt) -> Option<String> {
    let bytes = felt.to_bytes_be();
    let start = bytes.iter().position(|b| *b != 0)?;
    if start == 0 {
        return None;
    }
    let piece = &bytes[start..];
    piece
        .iter()
        .all(|b| (0x20..=0x7e).contains(b))
        .then(|| piece.iter().map(|b| char::from(*b)).collect())
}

/// `decodeShortStringArray`: the legacy `Array<felt252>` URI, a count then one short string per
/// felt; every element a field element, the count exact, every piece non-empty printable ASCII.
pub(super) fn decode_short_string_array(answer: &[JsValue], max_bytes: usize) -> Option<String> {
    if answer.len() < 2 || answer.len() > max_bytes + 1 {
        return None;
    }
    let felts: Vec<Felt> = answer
        .iter()
        .map(|item| {
            item.as_str()
                .filter(|text| is_stark_field_element(text))
                .and_then(crate::felt::parse)
        })
        .collect::<Option<_>>()?;
    if felts.first() != Some(&Felt::from(answer.len() as u64 - 1)) {
        return None;
    }
    let mut uri = String::new();
    for felt in felts.iter().skip(1) {
        uri.push_str(&short_string_piece(felt)?);
        if uri.len() > max_bytes {
            return None;
        }
    }
    Some(uri)
}

#[cfg(test)]
#[path = "token_uri_tests.rs"]
mod tests;
