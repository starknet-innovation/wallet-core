//! Stale Argent intents that may have been sent anyway (Sherlock 177, mc-wallet 673): the
//! extension's `multisig-stale-intents.ts` (`refuseResend`, `refuseSigningResend`,
//! `markFeeRefusedStale`), `recoverStaleArgentIntent` and `markRejected` in
//! `multisig-operation-recovery.ts`, the `nonceMismatch` rule in `multisig-intent-context.ts`, and
//! `markObservedArgentIntentOutcomes` in `multisig-config-reconcile.ts`.
//!
//! A fully signed intent goes stale when the chain nonce moves past it or the node refuses its
//! fee; either can happen after a co-signer (or the node itself) sent it. Before the same calls
//! are built, signed or sent again, the wallet asks the chain about each stale version's exact
//! signed hash. Sans-IO: the wallet sends the reads (`HashLookups`, the account's nonce) and these
//! functions judge what came back. Which intents are stored and how a row is journaled are the
//! wallet's.

use serde_json::Value;
use starknet_types_core::felt::Felt;

use super::call_decode::recipient_for_flags;
use super::calls::Call;
use super::judge::{self, Answer, Observation, TxStatus};
use super::multisig_intent::{IntentState, SigningIntent, StaleOutcome, StaleReason};
use super::multisig_invoke::MultisigCall;
use super::rpc_bodies;
use crate::felt;
use crate::js::felt::same_felt;

/// `ARGENT_INTENT_ALREADY_SENT`.
pub const ALREADY_SENT: &str = "Argent multisig intent was already sent";
/// `ARGENT_INTENT_RESEND_UNCONFIRMED`.
pub const RESEND_UNCONFIRMED: &str =
    "confirm resending calls an earlier Argent multisig intent may have sent";

/// One signed hash's status (`TransactionStatusReport`, reduced to what recovery reads).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignedHash {
    /// The node answered for the hash. `seen` only for a real finality or a revert: an odd
    /// answer is found-and-pending but proves nothing.
    Found { status: TxStatus, seen: bool },
    /// The node says it has no record of the hash.
    Absent,
    /// No usable answer (transport failure, unreadable body, or never asked).
    Unknown,
}

/// `classifyTransactionStatus` and `isTransactionNotFoundError` over one
/// `starknet_getTransactionStatus` reply.
pub fn signed_hash(answer: Option<Answer>) -> SignedHash {
    let seen = match &answer {
        Some(Answer::Result(result)) => {
            let text = |key: &str| result.get(key).and_then(Value::as_str).unwrap_or("");
            text("execution_status") == "REVERTED"
                || matches!(
                    text("finality_status"),
                    "RECEIVED"
                        | "CANDIDATE"
                        | "PRE_CONFIRMED"
                        | "ACCEPTED_ON_L2"
                        | "ACCEPTED_ON_L1"
                        | "REJECTED"
                )
        }
        _ => false,
    };
    match judge::status(answer) {
        Observation::Found { status, .. } => SignedHash::Found { status, seen },
        Observation::NotFound => SignedHash::Absent,
        Observation::Unknown => SignedHash::Unknown,
    }
}

/// A `starknet_getNonce` reply as one felt, `None` for anything else (an error included).
pub fn read_nonce(answer: Option<Answer>) -> Option<Felt> {
    match answer? {
        Answer::Result(Value::String(text)) => felt::parse(&text),
        _ => None,
    }
}

/// Status reads of a list of signed hashes, one at a time, numbered from `first_id`.
#[derive(Debug, Clone)]
pub struct HashLookups {
    first_id: u64,
    hashes: Vec<Felt>,
    answers: Vec<SignedHash>,
    pending: bool,
}

impl HashLookups {
    pub fn new(first_id: u64, hashes: Vec<Felt>) -> Self {
        Self {
            first_id,
            hashes,
            answers: Vec::new(),
            pending: false,
        }
    }

    fn next_id(&self) -> u64 {
        self.first_id + self.answers.len() as u64
    }

    /// The next `(id, body)` to send, or `None` once every hash was answered. A request already
    /// handed out is handed out again until it is answered.
    pub fn next_request(&mut self) -> Option<(u64, String)> {
        let hash = self.hashes.get(self.answers.len())?;
        self.pending = true;
        let id = self.next_id();
        Some((id, rpc_bodies::transaction_status(id, hash)))
    }

    /// Takes the reply to the pending request (`None` for a transport failure). `false`, and
    /// nothing changes, when `id` is not the pending request's.
    pub fn accept(&mut self, id: u64, answer: Option<Answer>) -> bool {
        if !self.pending || id != self.next_id() {
            return false;
        }
        self.pending = false;
        self.answers.push(signed_hash(answer));
        true
    }

    pub fn done(&self) -> bool {
        self.answers.len() == self.hashes.len()
    }

    /// What the chain said about `hash`; `Unknown` when it was not asked or not answered.
    pub fn result(&self, hash: &str) -> SignedHash {
        let Some(hash) = felt::parse(hash) else {
            return SignedHash::Unknown;
        };
        self.hashes
            .iter()
            .zip(&self.answers)
            .find(|(asked, _)| **asked == hash)
            .map_or(SignedHash::Unknown, |(_, answer)| *answer)
    }
}

/// `ArgentSubmitResolution`'s kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// On chain (accepted or still pending): it may only move to submitted.
    Submitted,
    Absent,
    /// The sequencer refused this exact hash: it never landed.
    Rejected,
    /// Included and reverted: its nonce is spent and nothing was transferred.
    Reverted,
    Unknown,
}

/// What [`recover_stale`] decided: the resolution, the intent to store (if it changed), and
/// whether the hash is filed as broadcast (its approvals, then its activity row) first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recovered {
    pub resolution: Resolution,
    pub store: Option<SigningIntent>,
    pub journal: bool,
}

fn full(intent: &SigningIntent) -> bool {
    intent.signatures.len() == intent.config_snapshot.threshold as usize
}

/// Whether [`recover_stale`] reads `intent`'s signed hash: a fully signed stale intent not
/// already settled as reverted.
pub fn needs_lookup(intent: &SigningIntent) -> bool {
    intent.state == IntentState::Stale
        && full(intent)
        && intent.stale_outcome != Some(StaleOutcome::Reverted)
}

/// `recoverStaleArgentIntent`, given what the chain said about the signed hash and the account's
/// nonce now (`markRejected`'s read; `None` when it could not be read). Only a positive sighting
/// moves the intent to submitted; a rejected hash records whether its nonce is still unused (never
/// relaxing a recorded `Rejected`); a reverted one is journaled and settled as stale.
pub fn recover_stale(intent: &SigningIntent, chain: SignedHash, nonce: Option<&Felt>) -> Recovered {
    let nothing = |resolution| Recovered {
        resolution,
        store: None,
        journal: false,
    };
    if intent.state != IntentState::Stale || !full(intent) {
        return nothing(Resolution::Absent);
    }
    if intent.stale_outcome == Some(StaleOutcome::Reverted) {
        return nothing(Resolution::Reverted);
    }
    let status = match chain {
        SignedHash::Found { seen: false, .. } | SignedHash::Unknown => {
            return nothing(Resolution::Unknown)
        }
        SignedHash::Absent => return nothing(Resolution::Absent),
        SignedHash::Found { status, .. } => status,
    };
    match status {
        TxStatus::Rejected => {
            let unused = intent.stale_outcome != Some(StaleOutcome::Rejected)
                && nonce.is_some_and(|nonce| {
                    same_felt(&felt::canonical(nonce), &intent.prepared.nonce)
                });
            let outcome = if unused {
                StaleOutcome::RejectedNonceUnused
            } else {
                StaleOutcome::Rejected
            };
            Recovered {
                resolution: Resolution::Rejected,
                store: (intent.stale_outcome != Some(outcome)).then(|| SigningIntent {
                    stale_outcome: Some(outcome),
                    ..intent.clone()
                }),
                journal: false,
            }
        }
        TxStatus::Reverted => Recovered {
            resolution: Resolution::Reverted,
            store: Some(SigningIntent {
                stale_outcome: Some(StaleOutcome::Reverted),
                ..intent.clone()
            }),
            journal: true,
        },
        TxStatus::Accepted | TxStatus::Pending | TxStatus::Preconfirmed => Recovered {
            resolution: Resolution::Submitted,
            store: Some(SigningIntent {
                broadcast_transaction_hash: Some(intent.prepared.transaction_hash.clone()),
                state: IntentState::Submitted,
                ..intent.clone()
            }),
            journal: true,
        },
    }
}

/// `nonceMismatch`'s rule: only a nonce past the prepared one can have been spent by this
/// intent; one behind it says nothing about it.
pub fn nonce_moved_past(nonce: &Felt, prepared_nonce: &str) -> bool {
    felt::parse(prepared_nonce).is_some_and(|prepared| *nonce > prepared)
}

/// `markFeeRefusedStale`'s reason: `Fee` only while the nonce read again now is still the
/// prepared one; moved or unreadable is `Nonce`, since a co-signer may have sent this version.
/// ([`SigningIntent::marked_stale`] keeps a recorded `Nonce` or `Config` either way.)
pub fn fee_refused_reason(nonce: Option<&Felt>, prepared_nonce: &str) -> StaleReason {
    if nonce.is_some_and(|nonce| same_felt(&felt::canonical(nonce), prepared_nonce)) {
        StaleReason::Fee
    } else {
        StaleReason::Nonce
    }
}

/// `sameCalls`: equal entrypoints, and contract and calldata equal as felts.
pub fn same_calls(left: &[MultisigCall], right: &[MultisigCall]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(call, other)| {
            call.entrypoint == other.entrypoint
                && same_felt(&call.contract_address, &other.contract_address)
                && call.calldata.len() == other.calldata.len()
                && call
                    .calldata
                    .iter()
                    .zip(&other.calldata)
                    .all(|(value, other)| same_felt(value, other))
        })
}

/// `staleIntents` filtered to these calls: every stale version of `calls` for `account_id`.
pub fn stale_versions<'a>(
    intents: &'a [SigningIntent],
    account_id: &'a str,
    calls: &'a [MultisigCall],
) -> impl Iterator<Item = &'a SigningIntent> {
    intents.iter().filter(move |intent| {
        intent.account_id == account_id
            && intent.state == IntentState::Stale
            && same_calls(&intent.prepared.calls, calls)
    })
}

/// What the stale versions of some calls say about sending them at a nonce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resend {
    /// One is on chain: refuse outright.
    Sent,
    /// One at another nonce is not known to be absent from it: only the person can say.
    Maybe,
}

/// `staleResend`, over each stale version of the calls and how [`recover_stale`] resolved it,
/// for calls about to be sent at `nonce`. A reverted version spent its own nonce and moved
/// nothing; at the same nonce only one version can execute; at another, anything short of
/// "sent" or "reverted" leaves a payment possible.
pub fn resend<'a>(
    resolved: impl IntoIterator<Item = (&'a SigningIntent, Resolution)>,
    nonce: &str,
) -> Option<Resend> {
    let mut maybe = false;
    for (intent, resolution) in resolved {
        match resolution {
            Resolution::Submitted => return Some(Resend::Sent),
            Resolution::Reverted => continue,
            _ => maybe |= !same_felt(&intent.prepared.nonce, nonce),
        }
    }
    maybe.then_some(Resend::Maybe)
}

/// `paidCounterparty`: the payee of a single-call payment, the counterparty its activity row
/// records (audit #163); `None` for a batch or a call that names none.
pub fn paid_counterparty(calls: &[Call]) -> Option<Felt> {
    let [call] = calls else {
        return None;
    };
    recipient_for_flags(&call.entrypoint, &call.calldata).filter(|payee| *payee != Felt::ZERO)
}

/// `markObservedArgentIntentOutcomes` for one submitted intent whose activity row (same hash)
/// reached a terminal status: accepted, or rejected for a dropped or reverted transaction.
pub fn settled(intent: &SigningIntent, status: TxStatus) -> Option<SigningIntent> {
    let state = match status {
        TxStatus::Accepted => IntentState::Accepted,
        TxStatus::Rejected | TxStatus::Reverted => IntentState::Rejected,
        TxStatus::Pending | TxStatus::Preconfirmed => return None,
    };
    (intent.state == IntentState::Submitted).then(|| SigningIntent {
        state,
        ..intent.clone()
    })
}

#[cfg(test)]
#[path = "multisig_stale_tests.rs"]
mod tests;
