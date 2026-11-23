//! Starknet ID names, one direction: a typed `name.stark` to the address the naming contract
//! answers for it (the extension's `resolveStarkName`, `packages/platform/starknet/src/rpc/
//! stark-name.ts`, over starknet.js 10.8.0's `starknetId.isStarkDomain` and `useEncoded`).
//!
//! Sans-IO: the core builds the `domain_to_address` call and judges one endpoint's reply. The
//! host moves the bytes and, as `resolveStarkName` does, reads each endpoint's chain id first
//! (`rpc::verify_chain_id`) and never sends this call to one on another chain, then fails over
//! on `TryNextEndpoint` or a transport failure; with no endpoint left the name is unreachable.
//! The naming contract is the wallet's catalog data, so the caller passes it in.

use serde_json::{json, Value};
use starknet_types_core::felt::Felt;

use crate::js::big::Big;
use crate::js::felt::{canonical_felt, is_non_zero_felt};
use crate::js::num::{js_trim, utf16_len};
use crate::rpc::envelope;
use crate::send::judge::{self, Answer};
use crate::{felt, Error};

const STARK_SUFFIX: &str = ".stark";
/// `isStarkDomain`'s longest label.
const MAX_LABEL_CHARS: usize = 48;
/// `MAX_STARK_NAME_CHARACTERS` (`readStarkNameInput`, `apps/extension/src/background/routing/
/// message-inputs.ts`): the extension refuses longer typed text, in UTF-16 units before trimming.
const MAX_NAME_UNITS: usize = 128;
/// `useEncoded`'s basic alphabet, by digit; `isStarkDomain` lets a label hold nothing else.
const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789-";
/// `starknet_keccak("domain_to_address")`, checked against `kms::selector` in the tests.
const DOMAIN_TO_ADDRESS_SELECTOR: &str =
    "0x2e269d930f6d7ab92b15ce8ff9f5e63709391617e3465fff79ba6baf278ce60";
/// JSON-RPC `CONTRACT_NOT_FOUND`.
const CONTRACT_NOT_FOUND: i64 = 20;

/// One endpoint's answer to `domain_to_address`: the extension's `StarkNameResolution` per
/// endpoint. Its `unreachable` is the host's, once every endpoint gave `TryNextEndpoint` or
/// failed in transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StarkNameAnswer {
    /// The contract answered a nonzero address, in canonical spelling.
    Resolved { address: String },
    /// The name is well formed but unregistered: the contract's zero answer.
    NotFound,
    /// The naming contract is not deployed on the endpoint's chain (JSON-RPC 20, or a node error
    /// saying "contract not found").
    UnsupportedNetwork,
    /// The endpoint malfunctioned: an unreadable reply, an answer that is not a felt, or an error
    /// without a numeric code. The extension fails these over like a transport fault.
    TryNextEndpoint,
    /// The node refused the call with another JSON-RPC error. The extension throws rather than
    /// fail over or fake a result, so this fails closed.
    Failed,
}

/// `normalizeStarkName`: the typed text trimmed and lower-cased when it is then a Starknet name
/// (`isStarkDomain`: the `.stark` suffix, required and never added, after one or more labels of
/// 1–48 characters from a-z, 0-9 and `-`), else `None`.
pub fn normalize(name: &str) -> Option<String> {
    let trimmed = js_trim(name).to_lowercase();
    is_stark_domain(&trimmed).then_some(trimmed)
}

fn is_stark_domain(domain: &str) -> bool {
    domain.strip_suffix(STARK_SUFFIX).is_some_and(|name| {
        !name.is_empty()
            && name.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= MAX_LABEL_CHARS
                    && label.bytes().all(|b| ALPHABET.contains(&b))
            })
    })
}

/// `useEncoded` for a label `isStarkDomain` admits: base 38, first character lowest, a final `a`
/// written as digit 37. Its other branches need characters no such label holds. `None` when the
/// value is not below the field prime (only a 48-character label gets there), where the
/// extension's `CallData.compile` throws before any call.
fn encode_label(label: &str) -> Option<Felt> {
    let last = label.len().checked_sub(1)?;
    let mut value = Big::zero();
    for (i, byte) in label.bytes().enumerate().rev() {
        let digit = ALPHABET.iter().position(|c| *c == byte)? as u32;
        value.mul_add(38, if i == last && digit == 0 { 37 } else { digit });
    }
    felt::parse(&format!("0x{}", value.to_hex()))
}

/// The `starknet_call` body asking `naming_contract`'s `domain_to_address` for `name`'s address at
/// the latest block, as starknet.js sends it: calldata is the label count, each label's
/// `useEncoded` felt, then an empty hint array. Refuses typed text the extension would not look
/// up (`readStarkNameInput`, `normalizeStarkName`, the encoder's field bound).
pub fn address_request(id: u64, name: &str, naming_contract: &Felt) -> Result<String, Error> {
    let invalid = || Error::invalid("invalid Starknet name");
    if utf16_len(name) > MAX_NAME_UNITS {
        return Err(invalid());
    }
    let domain = normalize(name).ok_or_else(invalid)?;
    let labels = domain
        .strip_suffix(STARK_SUFFIX)
        .ok_or_else(invalid)?
        .split('.')
        .map(|label| encode_label(label).map(|felt| felt::canonical(&felt)))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(invalid)?;
    let mut calldata = vec![format!("{:#x}", labels.len())];
    calldata.extend(labels);
    calldata.push("0x0".into());
    Ok(envelope(
        id,
        "starknet_call",
        json!({
            "block_id": "latest",
            "request": {
                "calldata": calldata,
                "contract_address": felt::canonical(naming_contract),
                "entry_point_selector": DOMAIN_TO_ADDRESS_SELECTOR,
            },
        }),
    ))
}

/// Judges one endpoint's reply to [`address_request`]. The first result entry is read as
/// `canonicalFelt` reads it (any spelling `BigInt` takes); a nonzero field element resolves, any
/// other number (zero, negative, past the prime) is `NotFound`, as `isNonZeroStarkFieldElement`
/// decides.
pub fn judge_address(body: &str, id: u64) -> StarkNameAnswer {
    match judge::answer(body, id) {
        Some(Answer::Result(Value::Array(values))) => {
            match values
                .first()
                .and_then(Value::as_str)
                .and_then(canonical_felt)
            {
                Some(address) if is_non_zero_felt(&address) => {
                    StarkNameAnswer::Resolved { address }
                }
                Some(_) => StarkNameAnswer::NotFound,
                None => StarkNameAnswer::TryNextEndpoint,
            }
        }
        Some(Answer::Error {
            code: Some(code),
            text,
        }) => {
            if code == CONTRACT_NOT_FOUND || text.contains("contract not found") {
                StarkNameAnswer::UnsupportedNetwork
            } else {
                StarkNameAnswer::Failed
            }
        }
        _ => StarkNameAnswer::TryNextEndpoint,
    }
}

#[cfg(test)]
#[path = "stark_name_tests.rs"]
mod tests;
