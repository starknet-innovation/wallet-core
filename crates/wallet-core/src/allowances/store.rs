//! The allowance log (`allowance-store.ts`, and the log half of `allowance-revoke.ts`): the
//! grants this wallet submitted, kept outside the vault so the manager can list them. One JSON
//! array of records under `AllowancePolicy::store_key`, oldest first, the extension's stored
//! shape field for field (`record::AllowanceRecord::to_json`), so the extension can adopt the
//! core over the `bramble:allowances` value it already holds. A value that is not a JSON array
//! reads as empty and an invalid record is dropped (`docs/WALLET_STORE.md`: discard invalid
//! rows); the next write keeps only what loaded.
//!
//! Every write is one read-modify-write of the value, so a wallet keeps one log per storage and
//! serialises calls on it; `&mut self` on every writer is that contract (the extension's
//! mutation queue). ERC-20's relative changes are applied here (`apply_delta`) for that reason.
//! A writer is given the `Ranking` of the tokens the account trusts at that moment.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::discovery::AllowanceReading;
use super::record::{is_canonical_hash, AllowanceInput, AllowanceRecord};
use super::retention::{keep_largest, trim_to_total, Ranking};
use super::AllowancePolicy;
use crate::host::Storage;
use crate::js::felt::canonical_felt;
use crate::send::u256::U256;
use crate::Error;

/// `U256::MAX`: a sum saturates at the ceiling the token itself enforces.
const UINT_256_MAX: U256 = U256 {
    high: u128::MAX,
    low: u128::MAX,
};

/// How a relative change moves the standing allowance (`AllowanceDelta.direction`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeltaDirection {
    /// `increase_allowance`.
    Add,
    /// `decrease_allowance`.
    Subtract,
}

/// A relative allowance change (`AllowanceDelta`). `transaction_hash` is the transaction that
/// carried it, kept only if the change turns out to be a revoke.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowanceDelta {
    pub direction: DeltaDirection,
    pub input: AllowanceInput,
    pub transaction_hash: Option<String>,
}

/// How a revoke transaction settled (`RevokeOutcome.status`), as the wallet's own activity
/// reconciliation observed it; one still pending has no outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevokeStatus {
    Accepted,
    Failed,
}

/// The outcome of one revoke, by its canonical transaction hash (`RevokeOutcome`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevokeOutcome {
    pub hash: String,
    pub status: RevokeStatus,
}

/// Whether the manager must say the log may have dropped approvals (the listing's
/// `retentionFull`): the whole log, or `account_id`'s records, at their cap.
pub fn retention_full(
    stored: &[AllowanceRecord],
    account_id: &str,
    policy: &AllowancePolicy,
) -> bool {
    stored.len() >= policy.entries_total
        || stored.iter().filter(|r| r.account_id == account_id).count() >= policy.entries_per_scope
}

// `recordedEntry`: the record an upsert writes, or `None` for nothing. An exact zero does not
// remove the pair (Sherlock 192): the grant is kept at its last amount, marked `revoking` with
// the revoke's hash, until a zero reading confirms it. A zero for a pair the log never held
// writes nothing.
fn recorded_entry(
    entries: &[AllowanceRecord],
    next: AllowanceRecord,
    transaction_hash: Option<&str>,
) -> Option<AllowanceRecord> {
    if next.amount != U256::ZERO || next.lower_bound {
        return Some(next);
    }
    let tracked = entries.iter().find(|stored| stored.id == next.id)?;
    let hash = transaction_hash
        .and_then(canonical_felt)
        .filter(|hash| is_canonical_hash(hash));
    Some(AllowanceRecord {
        revoke_hash: hash,
        revoking: true,
        ..tracked.clone()
    })
}

// `entriesAfterRecord`: the upsert, then the writer's own scope trimmed to its cap, then the
// whole log to the total; the incoming entry competes like any other.
fn after_record(
    entries: Vec<AllowanceRecord>,
    next: AllowanceRecord,
    transaction_hash: Option<&str>,
    ranking: &Ranking,
    policy: &AllowancePolicy,
) -> Vec<AllowanceRecord> {
    let Some(recorded) = recorded_entry(&entries, next, transaction_hash) else {
        return entries;
    };
    let scope = recorded.scope();
    let mut with_next: Vec<AllowanceRecord> = entries
        .into_iter()
        .filter(|stored| stored.id != recorded.id)
        .collect();
    with_next.push(recorded);
    let members: Vec<usize> = (0..with_next.len())
        .filter(|&i| with_next[i].scope() == scope)
        .collect();
    let mine: Vec<&AllowanceRecord> = members.iter().map(|&i| &with_next[i]).collect();
    let mut keep = vec![true; with_next.len()];
    for (&index, kept) in members
        .iter()
        .zip(keep_largest(&mine, policy.entries_per_scope, ranking))
    {
        keep[index] = kept;
    }
    let trimmed = with_next
        .into_iter()
        .zip(keep)
        .filter_map(|(record, keep)| keep.then_some(record))
        .collect();
    trim_to_total(trimmed, policy.entries_total, ranking)
}

// `appliedDeltaInput`: the change applied to the tracked figure, saturating at the uint256
// ceiling and flooring at zero. With no tracked base the result is a lower bound (the increment
// over an unseen base); a lower bound stays one; a decrease that floors a lower bound is a zero
// lower bound, still listed and still read, not a removal.
fn applied_delta(tracked: Option<&AllowanceRecord>, delta: &AllowanceDelta) -> AllowanceInput {
    let base = tracked.map_or(U256::ZERO, |record| record.amount);
    let amount = match delta.direction {
        DeltaDirection::Add => base.checked_add(delta.input.amount).unwrap_or(UINT_256_MAX),
        DeltaDirection::Subtract => base.checked_sub(delta.input.amount).unwrap_or(U256::ZERO),
    };
    AllowanceInput {
        amount,
        lower_bound: tracked.is_none_or(|record| record.lower_bound),
        ..delta.input.clone()
    }
}

enum Revocation<'a> {
    Unmarked,
    AwaitingOutcome(&'a str),
    AwaitingZero,
}

impl<'a> From<&'a AllowanceRecord> for Revocation<'a> {
    fn from(record: &'a AllowanceRecord) -> Self {
        match (record.revoking, record.revoke_hash.as_deref()) {
            (false, _) => Self::Unmarked,
            (true, Some(hash)) => Self::AwaitingOutcome(hash),
            (true, None) => Self::AwaitingZero,
        }
    }
}

// `settledEntry`: a zero reading drops a pending revoke or a zero lower bound; a failed revoke
// clears its mark; an accepted one with a non-zero reading takes that reading as its exact
// amount and keeps the mark (the outcome is spent). Nothing else changes a record.
fn settled_entry(
    stored: &AllowanceRecord,
    zeroed: &HashSet<&str>,
    non_zero: &HashMap<&str, U256>,
    outcomes: &HashMap<&str, RevokeStatus>,
) -> Option<AllowanceRecord> {
    let revocation = Revocation::from(stored);
    if zeroed.contains(stored.id.as_str()) {
        let remove = match revocation {
            Revocation::Unmarked => stored.amount == U256::ZERO && stored.lower_bound,
            Revocation::AwaitingOutcome(_) | Revocation::AwaitingZero => true,
        };
        if remove {
            return None;
        }
    }
    let outcome = match revocation {
        Revocation::AwaitingOutcome(hash) => outcomes.get(hash),
        Revocation::Unmarked | Revocation::AwaitingZero => None,
    };
    match (outcome, non_zero.get(stored.id.as_str())) {
        (Some(RevokeStatus::Failed), _) => Some(AllowanceRecord {
            revoking: false,
            revoke_hash: None,
            ..stored.clone()
        }),
        (Some(RevokeStatus::Accepted), Some(&amount)) => Some(AllowanceRecord {
            amount,
            lower_bound: false,
            revoke_hash: None,
            ..stored.clone()
        }),
        _ => Some(stored.clone()),
    }
}

/// The allowance log over the host's storage.
pub struct AllowanceLog<'a> {
    storage: &'a dyn Storage,
    policy: AllowancePolicy,
}

impl<'a> AllowanceLog<'a> {
    /// The log stored under `policy.store_key` in `storage`.
    pub fn new(storage: &'a dyn Storage, policy: AllowancePolicy) -> Self {
        Self { storage, policy }
    }

    // `readEntries`: oldest first, invalid records dropped; a storage failure is an error.
    fn load(&self) -> Result<Vec<AllowanceRecord>, Error> {
        let Some(bytes) = self.storage.read(self.policy.store_key)? else {
            return Ok(Vec::new());
        };
        Ok(match serde_json::from_slice::<Value>(&bytes) {
            Ok(Value::Array(items)) => items
                .iter()
                .filter_map(AllowanceRecord::from_json)
                .collect(),
            _ => Vec::new(),
        })
    }

    fn save(&self, entries: &[AllowanceRecord]) -> Result<(), Error> {
        let items: Vec<Value> = entries.iter().map(AllowanceRecord::to_json).collect();
        self.storage.write(
            self.policy.store_key,
            Value::Array(items).to_string().as_bytes(),
        )
    }

    /// Every valid record, newest first.
    pub fn list(&self) -> Result<Vec<AllowanceRecord>, Error> {
        let mut entries = self.load()?;
        entries.reverse();
        Ok(entries)
    }

    /// Upserts one allowance (`record`). An exact zero over a tracked grant marks it `revoking`
    /// with `transaction_hash`'s canonical spelling; an input that fails the field rules writes
    /// nothing.
    pub fn record(
        &mut self,
        input: &AllowanceInput,
        transaction_hash: Option<&str>,
        ranking: &Ranking,
    ) -> Result<(), Error> {
        let Some(next) = AllowanceRecord::from_input(input) else {
            return Ok(());
        };
        let entries = after_record(self.load()?, next, transaction_hash, ranking, &self.policy);
        self.save(&entries)
    }

    /// Applies an `increase_allowance` or `decrease_allowance` relative to the tracked figure
    /// (`applyDelta`), in the same read-modify-write.
    pub fn apply_delta(&mut self, delta: &AllowanceDelta, ranking: &Ranking) -> Result<(), Error> {
        let entries = self.load()?;
        let tracked = super::record::allowance_id(
            &delta.input.account_id,
            &delta.input.token,
            &delta.input.spender,
        )
        .and_then(|id| entries.iter().find(|stored| stored.id == id));
        let Some(next) = AllowanceRecord::from_input(&applied_delta(tracked, delta)) else {
            return Ok(());
        };
        let hash = delta.transaction_hash.as_deref();
        let entries = after_record(entries, next, hash, ranking, &self.policy);
        self.save(&entries)
    }

    /// Removes the record with `id`, if any.
    pub fn remove(&mut self, id: &str) -> Result<(), Error> {
        let mut entries = self.load()?;
        entries.retain(|stored| stored.id != id);
        self.save(&entries)
    }

    /// Settles pending revokes and zero lower bounds on live readings and on revoke outcomes
    /// (`settleRevocations`): only a zero reading deletes a record; nothing is written when
    /// nothing changed.
    pub fn settle_revocations(
        &mut self,
        readings: &[AllowanceReading],
        outcomes: &[RevokeOutcome],
    ) -> Result<(), Error> {
        // Indexed once (`entriesAfterSettle`), so the work is linear in the log and the
        // readings: a later non-zero reading for an id wins, and conflicting outcomes for one
        // hash resolve to failed, which only clears a mark.
        let zeroed: HashSet<&str> = readings
            .iter()
            .filter(|r| r.amount == U256::ZERO)
            .map(|r| r.id.as_str())
            .collect();
        let non_zero: HashMap<&str, U256> = readings
            .iter()
            .filter(|r| r.amount != U256::ZERO)
            .map(|r| (r.id.as_str(), r.amount))
            .collect();
        let mut by_hash: HashMap<&str, RevokeStatus> = HashMap::new();
        for outcome in outcomes {
            if by_hash.get(outcome.hash.as_str()) != Some(&RevokeStatus::Failed) {
                by_hash.insert(outcome.hash.as_str(), outcome.status);
            }
        }
        let entries = self.load()?;
        let settled: Vec<AllowanceRecord> = entries
            .iter()
            .filter_map(|stored| settled_entry(stored, &zeroed, &non_zero, &by_hash))
            .collect();
        if settled == entries {
            return Ok(());
        }
        self.save(&settled)
    }
}
