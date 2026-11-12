//! Sans-IO authority read for shared accounts, mirroring `getArgentMultisigConfig` and
//! `withPinnedBlock` (`packages/features/multisig/src/chain.ts`, `chain-pinned-read.ts`). The core
//! builds each JSON-RPC body and judges each response; the host only moves bytes. Each read first
//! proves the endpoint's chain id.
//!
//! The authority read pins ONE accepted block and asks every question at it, so a threshold and a
//! signer set can never describe two different states. A node that answers "Block not found"
//! (JSON-RPC 24) for a fresh pin is re-pinned, at most three attempts, as the extension does.
//!
//! Replies go through `send::judge::answer`; a node error is read by its numeric code only (an
//! error without one, like a malformed reply, settles `NetworkUnavailable`).

use serde_json::{json, Value};
use starknet_types_core::felt::Felt;

use super::multisig::ArgentMultisigClass;
use crate::js::felt::lenient_hex_felt;
use crate::kms::NetworkId;
use crate::nostr::safe_non_negative_integer;
use crate::rpc::{envelope, Request};
use crate::send::judge::{self, Answer, ChainAnswer};
use crate::{felt, Error};

const GET_THRESHOLD: &str = "0x1753cadb342b30cb76742fe738135a182b5c30e6e9eed2d3ee796b2accd34fd";
const GET_SIGNER_GUIDS: &str = "0x3a9ed885a5e9d921d3c00451660d2f71142bd45d166d07b1414837e911096dd";
const PINNED_READ_ATTEMPTS: u32 = 3;
const CONTRACT_NOT_FOUND: i64 = 20;
const BLOCK_NOT_FOUND: i64 = 24;

/// Outcome of one authority read, as the extension's `ArgentMultisigAuthorityStatus` minus
/// `unsupported-signers`, which needs `SignerLinked` event discovery (deferred).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultisigAuthorityStatus {
    Available,
    Undeployed,
    UnsupportedClass,
    InvalidConfig,
    NetworkUnavailable,
}

/// `isStarkFieldElement` then `canonicalFelt`: lowercase `0x` hex below the prime.
fn strict_felt(value: &Value) -> Option<Felt> {
    felt::parse(value.as_str()?)
}

/// `boundedNumberAt`: a felt no larger than `0xffffffff`.
fn bounded_number(value: Option<&Value>) -> Option<u32> {
    let felt = strict_felt(value?)?;
    if felt > Felt::from(u32::MAX) {
        return None;
    }
    let bytes = felt.to_bytes_be();
    Some(u32::from_be_bytes([
        bytes[28], bytes[29], bytes[30], bytes[31],
    ]))
}

#[derive(Debug, Clone)]
enum Phase {
    ChainId,
    Block {
        attempt: u32,
    },
    ClassHash {
        attempt: u32,
        block: String,
    },
    Threshold {
        attempt: u32,
        block: String,
    },
    Signers {
        attempt: u32,
        block: String,
        threshold: u32,
    },
    Done(MultisigAuthorityStatus),
}

/// Reads one address's class, threshold and signer GUIDs at one pinned block.
#[derive(Debug, Clone)]
pub struct MultisigAuthorityRead {
    network: NetworkId,
    address: Felt,
    class: Felt,
    max_signers: usize,
    phase: Phase,
    next_id: u64,
    pending: Option<u64>,
    authority: Option<(u32, Vec<Felt>)>,
}

impl MultisigAuthorityRead {
    /// Starts a read for a non-zero address, expecting `class`.
    pub fn new(
        network: NetworkId,
        address: &str,
        class: &ArgentMultisigClass,
    ) -> Result<Self, Error> {
        let address = felt::parse_non_zero(address)
            .ok_or_else(|| Error::invalid("invalid multisig address"))?;
        Ok(Self {
            network,
            address,
            class: class.hash(),
            max_signers: class.max_signers(),
            phase: Phase::ChainId,
            next_id: 1,
            pending: None,
            authority: None,
        })
    }

    /// The network this read targets.
    pub fn network(&self) -> NetworkId {
        self.network
    }

    /// The canonical address this read targets.
    pub fn address(&self) -> String {
        felt::canonical(&self.address)
    }

    /// Threshold and ascending signer GUIDs, once the status is `Available`.
    pub fn authority(&self) -> Option<(u32, Vec<Felt>)> {
        self.authority.clone()
    }

    fn call(&self, block: &str, selector: &str) -> (&'static str, Value) {
        (
            "starknet_call",
            json!({
                "block_id": { "block_hash": block },
                "request": {
                    "calldata": [],
                    "contract_address": felt::canonical(&self.address),
                    "entry_point_selector": selector,
                },
            }),
        )
    }

    fn request_for(&self, phase: &Phase) -> Option<(&'static str, Value)> {
        Some(match phase {
            Phase::ChainId => ("starknet_chainId", json!([])),
            Phase::Block { .. } => ("starknet_blockHashAndNumber", json!([])),
            Phase::ClassHash { block, .. } => (
                "starknet_getClassHashAt",
                json!({
                    "block_id": { "block_hash": block },
                    "contract_address": felt::canonical(&self.address),
                }),
            ),
            Phase::Threshold { block, .. } => self.call(block, GET_THRESHOLD),
            Phase::Signers { block, .. } => self.call(block, GET_SIGNER_GUIDS),
            Phase::Done(_) => return None,
        })
    }

    fn repin(attempt: u32) -> Phase {
        if attempt + 1 >= PINNED_READ_ATTEMPTS {
            Phase::Done(MultisigAuthorityStatus::NetworkUnavailable)
        } else {
            Phase::Block {
                attempt: attempt + 1,
            }
        }
    }

    fn advance(&mut self, phase: &Phase, answer: Option<Answer>) -> Phase {
        use MultisigAuthorityStatus as S;
        let result = match answer {
            None | Some(Answer::Error { code: None, .. }) => {
                return Phase::Done(S::NetworkUnavailable)
            }
            Some(Answer::Error {
                code: Some(CONTRACT_NOT_FOUND),
                ..
            }) if !matches!(phase, Phase::ChainId | Phase::Block { .. }) => {
                return Phase::Done(S::Undeployed)
            }
            Some(Answer::Error {
                code: Some(BLOCK_NOT_FOUND),
                ..
            }) => {
                return match phase {
                    Phase::ClassHash { attempt, .. }
                    | Phase::Threshold { attempt, .. }
                    | Phase::Signers { attempt, .. } => Self::repin(*attempt),
                    _ => Phase::Done(S::NetworkUnavailable),
                }
            }
            Some(Answer::Error { .. }) => return Phase::Done(S::NetworkUnavailable),
            Some(Answer::Result(result)) => result,
        };
        match phase.clone() {
            Phase::ChainId => match judge::chain(Some(Answer::Result(result)), self.network) {
                ChainAnswer::Match => Phase::Block { attempt: 0 },
                _ => Phase::Done(S::NetworkUnavailable),
            },
            Phase::Block { attempt } => {
                let hash = result
                    .get("block_hash")
                    .and_then(Value::as_str)
                    .and_then(lenient_hex_felt);
                let number = result
                    .get("block_number")
                    .and_then(safe_non_negative_integer);
                match (hash, number) {
                    (Some(hash), Some(_)) => Phase::ClassHash {
                        attempt,
                        block: felt::canonical(&hash),
                    },
                    _ => Phase::Done(S::InvalidConfig),
                }
            }
            Phase::ClassHash { attempt, block } => {
                if strict_felt(&result) == Some(self.class) {
                    Phase::Threshold { attempt, block }
                } else {
                    Phase::Done(S::UnsupportedClass)
                }
            }
            Phase::Threshold { attempt, block } => match result.as_array() {
                Some(values) if values.len() == 1 => match bounded_number(values.first()) {
                    Some(threshold) => Phase::Signers {
                        attempt,
                        block,
                        threshold,
                    },
                    None => Phase::Done(S::InvalidConfig),
                },
                _ => Phase::Done(S::InvalidConfig),
            },
            Phase::Signers { threshold, .. } => {
                match signer_guids(&result, threshold, self.max_signers) {
                    Some(guids) => {
                        self.authority = Some((threshold, guids));
                        Phase::Done(S::Available)
                    }
                    None => Phase::Done(S::InvalidConfig),
                }
            }
            Phase::Done(status) => Phase::Done(status),
        }
    }

    /// The next request to send, or `None` once the status is decided.
    pub fn next_request(&mut self) -> Option<Request> {
        let (method, params) = self.request_for(&self.phase)?;
        let id = match self.pending {
            Some(id) => id,
            None => {
                let id = self.next_id;
                self.next_id += 1;
                self.pending = Some(id);
                id
            }
        };
        Some(Request::new(id, envelope(id, method, params)))
    }

    /// Judges the response to the pending request. A body for any other id is refused and changes
    /// nothing; a malformed or failed answer settles `NetworkUnavailable`.
    pub fn accept_response(&mut self, id: u64, body: &str) -> Result<(), Error> {
        if self.pending != Some(id) {
            return Err(Error::rpc("response does not answer this request"));
        }
        self.pending = None;
        let phase = self.phase.clone();
        self.phase = self.advance(&phase, judge::answer(body, id));
        Ok(())
    }

    /// The host could not complete the pending request (timeout, TLS, HTTP status).
    pub fn transport_failed(&mut self, id: u64) {
        if self.pending == Some(id) {
            self.pending = None;
            self.phase = Phase::Done(MultisigAuthorityStatus::NetworkUnavailable);
        }
    }

    /// The decided status, once there is one.
    pub fn status(&self) -> Option<MultisigAuthorityStatus> {
        match self.phase {
            Phase::Done(status) => Some(status),
            _ => None,
        }
    }
}

/// `get_signer_guids`: `[n, guid…]` with `1 <= threshold <= n <= max_signers`, unique non-zero
/// GUIDs.
fn signer_guids(result: &Value, threshold: u32, max_signers: usize) -> Option<Vec<Felt>> {
    let values = result.as_array()?;
    let count = bounded_number(values.first())? as usize;
    if count == 0
        || count > max_signers
        || threshold == 0
        || threshold as usize > count
        || values.len() != count + 1
    {
        return None;
    }
    let mut guids = values[1..]
        .iter()
        .map(|value| strict_felt(value).filter(|guid| *guid != Felt::ZERO))
        .collect::<Option<Vec<Felt>>>()?;
    guids.sort();
    guids.dedup();
    (guids.len() == count).then_some(guids)
}

#[cfg(test)]
#[path = "multisig_read_tests.rs"]
mod tests;
