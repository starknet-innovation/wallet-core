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

/// The reads for one listing. A wallet that shares one across threads wraps it in its own lock.
#[derive(Debug)]
pub struct TrackedAllowanceRead {
    pairs: Vec<TrackedAllowancePair>,
    next: usize,
    next_id: u64,
    outstanding: Option<u64>,
    readings: Vec<AllowanceReading>,
}

impl TrackedAllowanceRead {
    /// Reads for the first `max_tracked_pairs` of `pairs` (in order), skipping any whose owner,
    /// token or spender is not a non-zero felt; each is read in `canonicalFelt`'s spelling.
    pub fn new(pairs: &[TrackedAllowancePair], policy: AllowancePolicy) -> Self {
        let canonical = |value: &str| canonical_felt(value).filter(|felt| is_non_zero_felt(felt));
        let pairs = pairs
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
        Self {
            pairs,
            next: 0,
            next_id: 1,
            outstanding: None,
            readings: Vec::new(),
        }
    }

    /// Discards all progress so the reads replay on another endpoint.
    pub fn restart(&mut self) {
        self.next = 0;
        self.next_id = 1;
        self.outstanding = None;
        self.readings.clear();
    }

    /// The call to send now (at most one); empty while waiting for a reply or when done.
    pub fn next_calls(&mut self) -> Vec<RpcRequest> {
        let Some(pair) = self
            .pairs
            .get(self.next)
            .filter(|_| self.outstanding.is_none())
        else {
            return Vec::new();
        };
        let id = self.next_id;
        self.next_id += 1;
        self.outstanding = Some(id);
        vec![allowance_call(id, &pair.token, &pair.owner, &pair.spender)]
    }

    /// Judges one reply: a reply for another call is a transport failure.
    pub fn accept(&mut self, reply: RpcReply) -> ScanStep {
        let Some(id) = self.outstanding.filter(|id| *id == reply.id) else {
            return ScanStep::Transport;
        };
        self.outstanding = None;
        let amount = match classify(&reply, id) {
            Reply::Transport | Reply::Result(JsValue::Null) => return ScanStep::Transport,
            Reply::Semantic => None,
            Reply::Result(JsValue::Array(items)) => parse_allowance_amount(&items),
            Reply::Result(_) => None,
        };
        if let Some(amount) = amount {
            self.readings.push(AllowanceReading {
                id: self.pairs[self.next].id.clone(),
                amount,
            });
        }
        self.next += 1;
        ScanStep::Continue
    }

    /// The readings, in pair order, once every call has been answered.
    pub fn finish(&self) -> Result<Vec<AllowanceReading>, Error> {
        if self.next < self.pairs.len() {
            return Err(Error::rpc("allowance reads are not finished"));
        }
        Ok(self.readings.clone())
    }
}
