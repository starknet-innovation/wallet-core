//! The two sans-IO multisig plans mobile drives, around `wallet_core::send::multisig_invoke_plan`:
//! `MultisigProposal` (`prepareIntent`) and `MultisigSubmission` (`submitIntent`). Neither signs
//! or holds a key. A proposal freezes the transaction, then looks up every stale version of its
//! calls (`refuseResend`) before it stores the collecting intent; a submission sends the
//! threshold signature the intent already holds, files the broadcast (`recordBroadcastIntent`),
//! and on the node's fee refusal reads the nonce again to say why the intent is stale
//! (`markFeeRefusedStale`).

use std::sync::{Arc, Mutex, PoisonError};

use wallet_core::accounts::multisig_records::Record;
use wallet_core::send::judge;
use wallet_core::send::multisig_intent::{ConfigSnapshot, IntentState, SigningIntent};
use wallet_core::send::multisig_invoke::PreparedInvoke;
use wallet_core::send::multisig_invoke_plan::{MultisigStep, ProposePlan, SubmitPlan};
use wallet_core::send::multisig_stale::{fee_refused_reason, read_nonce, HashLookups, Resend};

use crate::felt;
use crate::multisig_guard::{journal, lookups_for, stale_resend, OperationClaim};
use crate::multisig_intents;
use crate::multisig_tx::{
    intent_view, rpc_step, Bound, MultisigCallReview, MultisigIntent, MultisigTxFailure,
    MultisigTxStep,
};
use crate::send::types::{RpcPurpose, SendError, StepReply};
use crate::wallet::WalletCore;

/// The first id of a proposal's status lookups, past any its plan numbers.
const LOOKUP_ID: u64 = 2_000;
/// The id of a submission's nonce read after a fee refusal.
const NONCE_ID: u64 = 3_000;

/// The host's wall clock on a reply, when it carries one.
fn reply_time(reply: &StepReply) -> Option<u64> {
    match reply {
        StepReply::Rpc { now_ms, .. } | StepReply::TransportFailure { now_ms, .. } => Some(*now_ms),
        _ => None,
    }
}

/// The JSON-RPC answer a reply to request `id` carries; `Err` for a reply to another request.
fn answer_to(reply: StepReply, id: u64) -> Result<Option<judge::Answer>, SendError> {
    match reply {
        StepReply::Rpc { id: got, body, .. } if got == id => Ok(judge::answer(&body, id)),
        StepReply::TransportFailure { id: got, .. } if got == id => Ok(None),
        _ => Err(SendError::OutOfOrder),
    }
}

/// A proposal's plan, then the frozen transaction while its calls' stale versions are looked up.
pub(crate) struct Proposing {
    plan: ProposePlan,
    looking: Option<(PreparedInvoke, HashLookups)>,
    now_ms: u64,
}

/// A proposal (`prepareIntent`): chain id, nonce and one validated estimate, then the frozen
/// transaction, then the status of every stale version of its calls, and the collecting intent
/// stored under this wallet's own summary.
#[derive(uniffi::Object)]
pub struct MultisigProposal {
    bound: Bound<Proposing>,
    record: Record,
    snapshot: ConfigSnapshot,
    review: MultisigCallReview,
    confirm_resend: bool,
    stored: Mutex<Option<MultisigIntent>>,
}

impl MultisigProposal {
    pub(crate) fn new(
        core: &Arc<WalletCore>,
        generation: u64,
        plan: ProposePlan,
        record: Record,
        snapshot: ConfigSnapshot,
        review: MultisigCallReview,
        confirm_resend: bool,
    ) -> Arc<Self> {
        let proposing = Proposing {
            plan,
            looking: None,
            now_ms: 0,
        };
        Arc::new(Self {
            bound: Bound::new(core, generation, proposing),
            record,
            snapshot,
            review,
            confirm_resend,
            stored: Mutex::new(None),
        })
    }

    /// `refuseResend`, then the store, once every lookup was answered.
    fn finish(
        &self,
        core: &WalletCore,
        prepared: PreparedInvoke,
        lookups: &HashLookups,
        now_ms: u64,
        current: impl Fn() -> bool,
    ) -> MultisigTxStep {
        let failure = |failure| MultisigTxStep::Failed { failure };
        let Some(nonce) = felt::parse(&prepared.nonce) else {
            return failure(MultisigTxFailure::NotStored);
        };
        let chain = |hash: &str| lookups.result(hash);
        let resend = match stale_resend(
            core.data_dir(),
            &self.record,
            &prepared.calls,
            &nonce,
            chain,
            now_ms,
        ) {
            Ok(resend) => resend,
            Err(_) => return failure(MultisigTxFailure::NotStored),
        };
        match resend {
            Some(Resend::Sent) => return failure(MultisigTxFailure::AlreadySent),
            Some(Resend::Maybe) if !self.confirm_resend => {
                return failure(MultisigTxFailure::ResendUnconfirmed)
            }
            _ => {}
        }
        // `authorized.isCurrent()` before the store, as the extension checks it.
        if !current() {
            return failure(MultisigTxFailure::Locked);
        }
        let Some(intent) = SigningIntent::proposed(
            &self.record.id,
            self.snapshot.clone(),
            prepared,
            &self.review.summary,
        ) else {
            return failure(MultisigTxFailure::NotStored);
        };
        match multisig_intents::upsert(core.data_dir(), &intent) {
            Ok(()) => {
                *self.stored.lock().unwrap_or_else(PoisonError::into_inner) =
                    Some(intent_view(&intent, Some(&self.record)));
                MultisigTxStep::Proposed {
                    intent_id: intent.id,
                }
            }
            Err(_) => failure(MultisigTxFailure::NotStored),
        }
    }
}

#[uniffi::export]
impl MultisigProposal {
    /// What the person confirms before the plan starts.
    pub fn review(&self) -> MultisigCallReview {
        self.review.clone()
    }

    /// The intent this proposal stored, once it reached `Proposed`.
    pub fn intent(&self) -> Option<MultisigIntent> {
        self.stored
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Feeds the last reply (`Start` first) and returns the next step. A reply that does not
    /// answer the outstanding request is `OutOfOrder`; it and every terminal step close the plan.
    pub fn advance(&self, reply: StepReply) -> Result<MultisigTxStep, SendError> {
        self.bound.step(|proposing, bound| {
            if let Some(now_ms) = reply_time(&reply) {
                proposing.now_ms = now_ms;
            }
            let Some(core) = bound.core.upgrade() else {
                return Err(SendError::PlanClosed);
            };
            match proposing.looking.as_mut() {
                Some((_, lookups)) => {
                    let Some((id, _)) = lookups.next_request() else {
                        return Err(SendError::OutOfOrder);
                    };
                    lookups.accept(id, answer_to(reply, id)?);
                }
                None => {
                    let prepared = match rpc_step(proposing.plan.advance(reply)?) {
                        Ok(step) => return Ok(step),
                        Err(MultisigStep::Prepared(prepared)) => *prepared,
                        Err(_) => return Err(SendError::OutOfOrder),
                    };
                    let Ok(intents) = multisig_intents::load(core.data_dir()) else {
                        return Ok(MultisigTxStep::Failed {
                            failure: MultisigTxFailure::NotStored,
                        });
                    };
                    let hashes = lookups_for(&intents, &self.record.id, &prepared.calls);
                    proposing.looking = Some((prepared, HashLookups::new(LOOKUP_ID, hashes)));
                }
            }
            let Some((prepared, lookups)) = proposing.looking.as_mut() else {
                return Err(SendError::OutOfOrder);
            };
            if let Some((id, body)) = lookups.next_request() {
                return Ok(MultisigTxStep::Rpc {
                    id,
                    body,
                    purpose: RpcPurpose::TransactionStatus,
                    is_write: false,
                });
            }
            let current = || bound.current().is_some();
            Ok(self.finish(&core, prepared.clone(), lookups, proposing.now_ms, current))
        })
    }

    /// Stops the plan. Idempotent; nothing is stored after it.
    #[uniffi::method(name = "cancel")]
    pub fn close(&self) {
        self.bound.lock().closed = true;
    }
}

/// A submission's plan, and whether it is reading the nonce again after a fee refusal.
pub(crate) struct Submitting {
    plan: SubmitPlan,
    rereading: bool,
}

/// A submission (`submitIntent`): proves the chain, then sends the frozen transaction with the
/// intent's threshold signature exactly once. On acceptance the intent is stored submitted and
/// the broadcast filed; on the node's fee refusal the nonce is read again and the intent stored
/// stale. While it runs it holds the intent's submit claim.
#[derive(uniffi::Object)]
pub struct MultisigSubmission {
    bound: Bound<Submitting>,
    intent: SigningIntent,
    record: Record,
    claim: Mutex<Option<OperationClaim>>,
}

impl MultisigSubmission {
    pub(crate) fn new(
        core: &Arc<WalletCore>,
        generation: u64,
        plan: SubmitPlan,
        intent: SigningIntent,
        record: Record,
        claim: OperationClaim,
    ) -> Arc<Self> {
        let submitting = Submitting {
            plan,
            rereading: false,
        };
        Arc::new(Self {
            bound: Bound::new(core, generation, submitting),
            intent,
            record,
            claim: Mutex::new(Some(claim)),
        })
    }

    fn release(&self) {
        self.claim
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
    }
}

#[uniffi::export]
impl MultisigSubmission {
    /// The intent being submitted, as it was when the plan was made.
    pub fn intent(&self) -> MultisigIntent {
        intent_view(&self.intent, Some(&self.record))
    }

    /// Feeds the last reply (`Start` first). The session is checked once more after the chain is
    /// proven and before the write leaves; a lock ends the plan `Locked`.
    pub fn advance(&self, reply: StepReply) -> Result<MultisigTxStep, SendError> {
        let step = self.bound.step(|submitting, bound| {
            let now_ms = reply_time(&reply).unwrap_or(0);
            // Best effort, as the extension's: the node's answer stands whatever the store says.
            let dir = bound.core.upgrade();
            let dir = dir.as_ref().map(|core| core.data_dir());
            let store = |next: SigningIntent| {
                if let Some(dir) = dir {
                    let _ = multisig_intents::upsert(dir, &next);
                }
            };
            let filed = |hash: &str| {
                if let Some(dir) = dir {
                    journal(dir, &self.intent, &self.record, hash, now_ms);
                }
            };
            if submitting.rereading {
                let nonce = read_nonce(answer_to(reply, NONCE_ID)?);
                let reason = fee_refused_reason(nonce.as_ref(), &self.intent.prepared.nonce);
                store(self.intent.marked_stale(reason));
                return Ok(MultisigTxStep::Failed {
                    failure: MultisigTxFailure::FeeExpired,
                });
            }
            let step = submitting
                .plan
                .advance(reply, &|| bound.current().is_some())?;
            match rpc_step(step) {
                Ok(MultisigTxStep::Failed {
                    failure: MultisigTxFailure::FeeExpired,
                }) => {
                    submitting.rereading = true;
                    let address = felt::parse(&self.record.address).ok_or(SendError::PlanClosed)?;
                    Ok(MultisigTxStep::Rpc {
                        id: NONCE_ID,
                        body: wallet_core::send::rpc_bodies::nonce(NONCE_ID, &address),
                        purpose: RpcPurpose::Nonce,
                        is_write: false,
                    })
                }
                Ok(step) => {
                    if step
                        == (MultisigTxStep::Failed {
                            failure: MultisigTxFailure::SubmissionIndeterminate,
                        })
                    {
                        // The reply was lost: file it under the hash it would carry.
                        filed(&self.intent.prepared.transaction_hash);
                    }
                    Ok(step)
                }
                Err(MultisigStep::Submitted { hash }) => {
                    store(SigningIntent {
                        broadcast_transaction_hash: Some(hash.clone()),
                        state: IntentState::Submitted,
                        ..self.intent.clone()
                    });
                    filed(&hash);
                    Ok(MultisigTxStep::Submitted { hash })
                }
                Err(_) => Err(SendError::OutOfOrder),
            }
        });
        if !matches!(step, Ok(MultisigTxStep::Rpc { .. })) {
            self.release();
        }
        step
    }

    /// Stops the plan and releases its claim. Idempotent; once the write was handed out its
    /// outcome is the chain's.
    #[uniffi::method(name = "cancel")]
    pub fn close(&self) {
        self.bound.lock().closed = true;
        self.release();
    }
}
