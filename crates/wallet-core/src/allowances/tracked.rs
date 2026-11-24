//! The live amount of allowances the wallet already tracks (`allowance-scan.ts
//! collectTrackedAllowances`, and `readTrackedAmounts` in `background/portfolio/
//! allowance-ops.ts`): one `allowance(owner, spender)` read per pair, one at a time, at most
//! `max_tracked_pairs`. It needs no event scan and no token registry, so it reaches a dapp's own
//! token and a multisig owner. A node's JSON-RPC error skips that pair; a transport failure is
//! `ScanStep::Transport`, and the host replays on the next endpoint (`restart`), or, with none
//! left, lists no readings (records keep their stored amounts).

use serde_json::json;

use super::discovery::{parse_allowance_amount, AllowanceReading};
use super::record::AllowanceRecord;
use super::scan::ScanStep;
use super::AllowancePolicy;
use crate::js::felt::{canonical_felt, is_non_zero_felt};
use crate::js::json::JsValue;
use crate::portfolio::wire::{classify, rpc_call, Reply, RpcReply, RpcRequest};
use crate::Error;

/// `starknet_keccak("allowance")`; checked against `kms::selector` in the tests.
pub const ALLOWANCE_SELECTOR: &str =
    "0x1e888a1026b19c8c0b57c72d63ed1737106aa10034105b980ba117bd0c29fe1";

/// The one `allowance(owner, spender)` call at the latest block every read shares
/// (`readAllowanceAmount`), so discovery and the tracked read never disagree on calldata.
pub(super) fn allowance_call(id: u64, token: &str, owner: &str, spender: &str) -> RpcRequest {
    rpc_call(
        id,
        "starknet_call",
        json!({
            "block_id": "latest",
            "request": {
                "calldata": [owner, spender],
                "contract_address": token,
                "entry_point_selector": ALLOWANCE_SELECTOR,
            },
        }),
    )
}

/// One (owner, token, spender) the log holds, with that record's id (`TrackedAllowancePair`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackedAllowancePair {
    pub id: String,
    pub owner: String,
    pub token: String,
    pub spender: String,
}

/// The pairs to read for `records`, lower bounds first (a reading can turn them back into exact
/// figures), each owned by the address `owner_of` gives its account id; a record whose scope has
/// no address is left out.
pub fn tracked_pairs(
    records: &[AllowanceRecord],
    owner_of: impl Fn(&str) -> Option<String>,
) -> Vec<TrackedAllowancePair> {
    let mut ordered: Vec<&AllowanceRecord> = records.iter().collect();
    ordered.sort_by_key(|record| !record.lower_bound);
    ordered
        .into_iter()
        .filter_map(|record| {
            Some(TrackedAllowancePair {
                id: record.id.clone(),
                owner: owner_of(&record.account_id)?,
                token: record.token.clone(),
                spender: record.spender.clone(),
            })
        })
        .collect()
}

#[derive(Debug, Clone, Copy)]
enum ReadState {
    Ready(usize),
    Awaiting { index: usize, id: u64 },
    RestartRequired,
    Done,
}

/// The reads for one listing. A wallet that shares one across threads wraps it in its own lock.
#[derive(Debug)]
pub struct TrackedAllowanceRead {
    pairs: Vec<TrackedAllowancePair>,
    state: ReadState,
    next_id: u64,
    first_id: u64,
    readings: Vec<AllowanceReading>,
}

impl TrackedAllowanceRead {
    /// Reads for the first `max_tracked_pairs` of `pairs` (in order), skipping any whose owner,
    /// token or spender is not a non-zero felt; each is read in `canonicalFelt`'s spelling.
    pub fn new(pairs: &[TrackedAllowancePair], policy: AllowancePolicy) -> Self {
        let canonical = |value: &str| canonical_felt(value).filter(|felt| is_non_zero_felt(felt));
        let pairs: Vec<_> = pairs
            .iter()
            .take(policy.max_tracked_pairs)
            .filter_map(|pair| {
                Some(TrackedAllowancePair {
                    id: pair.id.clone(),
                    owner: canonical(&pair.owner)?,
                    token: canonical(&pair.token)?,
                    spender: canonical(&pair.spender)?,
                })
            })
            .collect();
        let mut read = Self {
            pairs,
            state: ReadState::Done,
            next_id: 1,
            first_id: 1,
            readings: Vec::new(),
        };
        read.restart();
        read
    }

    /// Discards all progress so the reads replay on another endpoint.
    pub fn restart(&mut self) {
        self.state = if self.pairs.is_empty() {
            ReadState::Done
        } else {
            ReadState::Ready(0)
        };
        self.first_id = self.next_id;
        self.readings.clear();
    }

    /// The call to send now (at most one); empty while waiting, after failure, or when done.
    pub fn next_calls(&mut self) -> Vec<RpcRequest> {
        let ReadState::Ready(index) = self.state else {
            return Vec::new();
        };
        let pair = &self.pairs[index];
        let id = self.next_id;
        self.next_id += 1;
        self.state = ReadState::Awaiting { index, id };
        vec![allowance_call(id, &pair.token, &pair.owner, &pair.spender)]
    }

    /// Judges one reply, ignoring replies from an earlier attempt.
    pub fn accept(&mut self, reply: RpcReply) -> ScanStep {
        if (1..self.first_id).contains(&reply.id) {
            return ScanStep::Continue;
        }
        let (index, id) = match self.state {
            ReadState::Awaiting { index, id } => (index, id),
            ReadState::Ready(_) => {
                self.state = ReadState::RestartRequired;
                return ScanStep::Transport;
            }
            ReadState::RestartRequired | ReadState::Done => return ScanStep::Transport,
        };
        let amount = match classify(&reply, id) {
            Reply::Transport | Reply::Result(JsValue::Null) => {
                self.state = ReadState::RestartRequired;
                return ScanStep::Transport;
            }
            Reply::Semantic => None,
            Reply::Result(JsValue::Array(items)) => parse_allowance_amount(&items),
            Reply::Result(_) => None,
        };
        if let Some(amount) = amount {
            self.readings.push(AllowanceReading {
                id: self.pairs[index].id.clone(),
                amount,
            });
        }
        let next = index + 1;
        self.state = if next == self.pairs.len() {
            ReadState::Done
        } else {
            ReadState::Ready(next)
        };
        ScanStep::Continue
    }

    /// The readings, in pair order, once every call has been answered.
    pub fn finish(&self) -> Result<Vec<AllowanceReading>, Error> {
        if !matches!(self.state, ReadState::Done) {
            return Err(Error::rpc("allowance reads are not finished"));
        }
        Ok(self.readings.clone())
    }
}
