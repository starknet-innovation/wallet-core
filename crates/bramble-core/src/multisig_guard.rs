//! The extension's protection against paying twice (Sherlock 177, mc-wallet 673) and its
//! bookkeeping around a broadcast, on mobile's stores: `recordBroadcastIntent` (approvals, then
//! the activity row), the stale-version checks every propose, sign and submit runs
//! (`refuseResend`, `refuseSigningResend`, `nonceMismatch`), settling a submitted intent from its
//! activity row (`markObservedArgentIntentOutcomes`), and the submit claim. What each chain answer
//! means is `wallet_core::send::multisig_stale`; this module applies it to the stores.
//!
//! The reads are the host's: `multisig_intent_read` asks for the hashes before a sign or submit,
//! and a proposal asks for them in its own steps. Every write here is best effort, as the
//! extension's: the node's answer stands whatever a store says.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use wallet_core::accounts::multisig_records::Record;
use wallet_core::activity::store::{kind_text, parse_status, Entry};
use wallet_core::allowances::calls::{record_submitted, AllowanceOwner};
use wallet_core::allowances::retention::Ranking;
use wallet_core::history::activity::ActivityKind;
use wallet_core::send::call_decode::Known;
use wallet_core::send::multisig_import::{require_current, ImportError};
use wallet_core::send::multisig_intent::{IntentState, SigningIntent, StaleReason};
use wallet_core::send::multisig_invoke::{as_calls, MultisigCall};
use wallet_core::send::multisig_stale::{
    needs_lookup, nonce_moved_past, paid_counterparty, recover_stale, resend, settled,
    stale_versions, Recovered, Resend, Resolution, SignedHash, ALREADY_SENT,
};
use wallet_core::Felt;

use crate::activity::with_log;
use crate::allowances::with_allowance_log;
use crate::multisig_rpc::{IntentLookups, MultisigStateRead};
use crate::networks::NetworkId;
use crate::wallet::WalletCore;
use crate::{felt, multisig_constants, multisig_intents, multisig_store, CoreError};

/// `recordBroadcastIntent`: files `intent`, broadcast (or possibly broadcast) under `hash`, from
/// the multisig `record`: its approvals in the allowance log first (`record_submitted`, the only
/// way they reach it), then its activity row, pending, with a single payment's payee as the
/// counterparty. Mobile watches no collections, so every approval is filed; the log ranks by its
/// default. Best effort: a failed write leaves a row missing and never reaches the caller.
pub(crate) fn journal(
    dir: &Path,
    intent: &SigningIntent,
    record: &Record,
    hash: &str,
    now_ms: u64,
) {
    let (Some(network), Some(address), Some(calls)) = (
        NetworkId::from_catalog_id(&record.network_id),
        felt::parse(&record.address),
        as_calls(&intent.prepared.calls),
    ) else {
        return;
    };
    if let Some(owner) = AllowanceOwner::of(network, &address) {
        let known = Known {
            tokens: &[],
            nft_contracts: &[],
        };
        with_allowance_log(dir, |log| {
            record_submitted(log, &calls, &owner, &known, hash, &Ranking::default())
        });
    }
    let entry = Entry {
        account_id: record.id.clone(),
        amount: None,
        counterparty: paid_counterparty(&calls).map(|payee| felt::canonical(&payee)),
        direction: None,
        failure_notice: None,
        fee_payer: None,
        user_fee: None,
        relay: None,
        fee_paid: None,
        finalized_on_l1: None,
        hash: felt::parse(hash).map_or_else(|| hash.to_string(), |hash| felt::canonical(&hash)),
        kind: kind_text(ActivityKind::Invoke).into(),
        network_id: record.network_id.clone(),
        not_found_count: None,
        // The one writer of this marker (`recordSubmittedActivity`, audit #163).
        origin: Some("wallet".into()),
        status: "pending".into(),
        submitted_at: now_ms,
        submitted_price: None,
        summary: intent.summary.clone(),
        token: None,
        token_address: None,
    };
    with_log(dir, |log| log.record(entry));
}

/// Applies what `recover_stale` decided: journal first (recording a hash again replaces its row,
/// so a retry after a failed state write repeats harmlessly), then the intent. A found intent
/// whose state could not be stored is `Unknown`, as the extension's.
pub(crate) fn apply(
    dir: &Path,
    record: &Record,
    intent: &SigningIntent,
    recovered: Recovered,
    now_ms: u64,
) -> Resolution {
    if recovered.journal {
        journal(
            dir,
            intent,
            record,
            &intent.prepared.transaction_hash,
            now_ms,
        );
    }
    match recovered.store {
        Some(next) => {
            let stored = multisig_intents::upsert(dir, &next).is_ok();
            if recovered.resolution == Resolution::Submitted && !stored {
                Resolution::Unknown
            } else {
                recovered.resolution
            }
        }
        None => recovered.resolution,
    }
}

/// `staleResend` for `calls` about to be proposed, signed or sent at `nonce` from `record`: each
/// stale version is resolved from `chain` (what the host's lookup of its hash said) and the
/// account's nonce, a found one moved to submitted and journaled, and the verdict returned.
pub(crate) fn stale_resend(
    dir: &Path,
    record: &Record,
    calls: &[MultisigCall],
    nonce: &Felt,
    chain: impl Fn(&str) -> SignedHash,
    now_ms: u64,
) -> Result<Option<Resend>, CoreError> {
    let intents = multisig_intents::load(dir)?;
    let resolved: Vec<_> = stale_versions(&intents, &record.id, calls)
        .map(|intent| {
            let seen = if needs_lookup(intent) {
                chain(&intent.prepared.transaction_hash)
            } else {
                SignedHash::Unknown
            };
            let recovered = recover_stale(intent, seen, Some(nonce));
            (intent, apply(dir, record, intent, recovered, now_ms))
        })
        .collect();
    Ok(resend(resolved, &felt::canonical(nonce)))
}

/// The hashes `stale_resend` will look up for `calls` of `account_id`.
pub(crate) fn lookups_for(
    intents: &[SigningIntent],
    account_id: &str,
    calls: &[MultisigCall],
) -> Vec<Felt> {
    stale_versions(intents, account_id, calls)
        .filter(|intent| needs_lookup(intent))
        .filter_map(|intent| felt::parse(&intent.prepared.transaction_hash))
        .collect()
}

/// `markObservedArgentIntentOutcomes`: every submitted intent of `account_id` whose activity row
/// reached a terminal status is stored accepted or rejected, so it can be evicted to make room.
pub(crate) fn settle(dir: &Path, account_id: &str) -> Result<(), CoreError> {
    let rows = with_log(dir, |log| log.list(account_id));
    for intent in multisig_intents::load(dir)? {
        if intent.account_id != account_id || intent.state != IntentState::Submitted {
            continue;
        }
        let Some(broadcast) = intent.broadcast_transaction_hash.as_deref() else {
            continue;
        };
        let status = rows
            .iter()
            .find(|row| wallet_core::js::felt::same_felt(&row.hash, broadcast))
            .and_then(|row| parse_status(&row.status));
        if let Some(next) = status.and_then(|status| settled(&intent, status)) {
            let _ = multisig_intents::upsert(dir, &next);
        }
    }
    Ok(())
}

/// Intents a submission is under way for in this process (`claimOperation("submit", ..)`).
static SUBMITTING: Mutex<Option<HashSet<String>>> = Mutex::new(None);

/// Held while one submission of an intent is live; released when dropped.
pub(crate) struct SubmitClaim(String);

impl SubmitClaim {
    /// Claims `intent_id` in the wallet at `dir`; `None` while another submission of it holds
    /// the claim.
    pub(crate) fn take(dir: &Path, intent_id: &str) -> Option<Self> {
        let key = format!("{}\n{intent_id}", dir.display());
        let mut claims = SUBMITTING.lock().unwrap_or_else(PoisonError::into_inner);
        claims
            .get_or_insert_with(HashSet::new)
            .insert(key.clone())
            .then(|| Self(key))
    }
}

impl Drop for SubmitClaim {
    fn drop(&mut self) {
        let mut claims = SUBMITTING.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(claims) = claims.as_mut() {
            claims.remove(&self.0);
        }
    }
}

fn refused(reason: &str) -> CoreError {
    CoreError::InvalidInput {
        reason: reason.into(),
    }
}

impl WalletCore {
    /// `currentArgentIntentContext` for a local sign or submit: `state` is finished, targets the
    /// intent's account, and (to pass) was made for `intent` (`multisig_intent_read`); the chain
    /// still holds its snapshot (else it is
    /// stored stale, `Config`) and its nonce. A moved nonce stores it stale (`Nonce`) and, when
    /// the nonce is past it, its own hash found on chain moves it to submitted and refuses it as
    /// already sent (`nonceMismatch`). With the nonce current, a stale version of the same calls
    /// found on chain refuses it too (`refuseSigningResend`); a lookup that failed does not.
    pub(crate) fn require_chain(
        &self,
        intent: &SigningIntent,
        record: &Record,
        state: &MultisigStateRead,
    ) -> Result<(), CoreError> {
        let unavailable = || refused(ImportError::AuthorityUnavailable.message());
        let (read, nonce) = state.finished().ok_or_else(unavailable)?;
        // What the read looked up, when it was made for this intent; a plain read looked up
        // nothing, and only gets as far as the chain checks.
        let ours = state
            .intent()
            .filter(|(read_for, ..)| *read_for == intent.id);
        let dir = self.data_dir();
        let class = multisig_constants::argent_multisig_class();
        match require_current(intent, record, &read, &nonce, class) {
            Ok(()) => {
                let (_, now_ms, lookups) = ours
                    .ok_or_else(|| refused("Argent multisig state read is not for this intent"))?;
                let chain = |hash: &str| lookups.result(hash);
                let resend =
                    stale_resend(dir, record, &intent.prepared.calls, &nonce, chain, now_ms)?;
                if resend == Some(Resend::Sent) {
                    return Err(refused(ALREADY_SENT));
                }
                Ok(())
            }
            Err(ImportError::ConfigChanged) => {
                // Best effort: the refusal stands whether or not the mark is stored.
                let _ = multisig_intents::upsert(dir, &intent.marked_stale(StaleReason::Config));
                Err(refused(ImportError::ConfigChanged.message()))
            }
            Err(ImportError::NonceNotCurrent) => {
                let stale = intent.marked_stale(StaleReason::Nonce);
                let _ = multisig_intents::upsert(dir, &stale);
                if let (true, Some((_, now_ms, lookups))) =
                    (nonce_moved_past(&nonce, &intent.prepared.nonce), ours)
                {
                    let seen = lookups.result(&intent.prepared.transaction_hash);
                    let recovered = recover_stale(&stale, seen, Some(&nonce));
                    if apply(dir, record, &stale, recovered, now_ms) == Resolution::Submitted {
                        return Err(refused(ALREADY_SENT));
                    }
                }
                Err(refused(ImportError::NonceNotCurrent.message()))
            }
            Err(error) => Err(refused(error.message())),
        }
    }
}

#[uniffi::export]
impl WalletCore {
    /// The state read a sign or submit of `intent_id` takes: the multisig's authority and nonce,
    /// then the status of every signed hash that says whether this intent, or a stale version
    /// of its calls, was already sent. `now_ms` is the host's wall clock: a version found on
    /// chain is filed in activity at that time.
    pub fn multisig_intent_read(
        &self,
        intent_id: String,
        now_ms: u64,
    ) -> Result<Arc<MultisigStateRead>, CoreError> {
        self.require_unlocked()?;
        let intents = multisig_intents::load(self.data_dir())?;
        let intent = intents
            .iter()
            .find(|intent| intent.id == intent_id)
            .ok_or_else(|| CoreError::InvalidInput {
                reason: "intent not found".into(),
            })?;
        let record = multisig_store::load(self.data_dir())?
            .records
            .into_iter()
            .find(|record| record.id == intent.account_id)
            .ok_or_else(|| CoreError::InvalidInput {
                reason: "Argent multisig not found".into(),
            })?;
        let network = NetworkId::from_catalog_id(&record.network_id).ok_or_else(|| {
            CoreError::InvalidInput {
                reason: "Argent multisig not found".into(),
            }
        })?;
        let full = intent.signatures.len() == intent.config_snapshot.threshold as usize;
        let lookups = IntentLookups {
            intent_id: intent.id.clone(),
            now_ms,
            prepared_nonce: intent.prepared.nonce.clone(),
            own: felt::parse(&intent.prepared.transaction_hash).filter(|_| full),
            // The intent itself among them, when it is a stale version: found, it was sent.
            stale: lookups_for(&intents, &intent.account_id, &intent.prepared.calls),
            lookups: None,
        };
        MultisigStateRead::for_intent(network, &record.address, lookups)
    }
}

#[cfg(test)]
#[path = "multisig_guard_tests.rs"]
mod tests;
