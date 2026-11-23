//! Argent multisig transactions over the FFI (P1): the records mobile's screens read and the two
//! sans-IO plans, `MultisigProposal` (`prepareIntent`) and `MultisigSubmission`
//! (`submitIntent`), around `wallet_core::send::multisig_invoke_plan`. Neither plan signs or
//! holds a key: a proposal freezes the transaction and stores the collecting intent, and a
//! submission broadcasts the threshold signature the intent already holds. The entry points that
//! start them, co-sign and import are `multisig_tx_ops`.
//!
//! Every refusal is a fixed variant or one of the extension's fixed sentences; none carries a
//! key, a call's calldata or node text.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

pub use wallet_core::accounts::multisig_change::ConfigChange;
use wallet_core::send::fees::{fee_amount, FeeAmount};
pub use wallet_core::send::multisig_intent::IntentState;
use wallet_core::send::multisig_intent::{ConfigSnapshot, SigningIntent, StaleReason};
pub use wallet_core::send::multisig_invoke::MultisigCall;
use wallet_core::send::multisig_invoke_plan::{
    MultisigFailure, MultisigStep, ProposePlan, SubmitPlan,
};

use crate::argent_multisig::ArgentMultisigConfig;
use crate::multisig_intents;
use crate::send::types::{RpcPurpose, SendError, StepReply};
use crate::wallet::WalletCore;

/// One call of a multisig transaction, as the extension's `PreparedArgentMultisigCall`.
#[uniffi::remote(Record)]
pub struct MultisigCall {
    pub calldata: Vec<String>,
    pub contract_address: String,
    pub entrypoint: String,
}

/// Where a signing intent is in its life (`ArgentMultisigIntentState`).
#[uniffi::remote(Enum)]
pub enum IntentState {
    Accepted,
    Collecting,
    Ready,
    Rejected,
    Stale,
    Submitted,
}

/// One signer-management operation (`ArgentMultisigConfigChange`). Keys are public keys.
#[uniffi::remote(Enum)]
pub enum ConfigChange {
    AddSigners {
        signer_public_keys: Vec<String>,
        threshold: u32,
    },
    RemoveSigners {
        signer_public_keys: Vec<String>,
        threshold: u32,
    },
    ChangeThreshold {
        threshold: u32,
    },
    ReplaceSigner {
        add_signer_public_key: String,
        remove_signer_public_key: String,
    },
}

/// A stored signing intent as the screens show it. Everything here is public: the frozen
/// transaction, the snapshot the co-signers sign against, and who has signed.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MultisigIntent {
    /// `<multisig id>:<transaction hash>`.
    pub id: String,
    pub multisig_id: String,
    pub state: IntentState,
    /// This wallet's own review of the calls, never a co-signer's text.
    pub summary: String,
    pub transaction_hash: String,
    pub nonce: String,
    pub threshold: u32,
    pub signer_guids: Vec<String>,
    /// The public keys that have signed, in GUID order.
    pub signed_public_keys: Vec<String>,
    /// The most the signed bounds authorise (`preparedArgentMultisigFee`), never an estimate.
    pub max_fee: Option<FeeAmount>,
    pub calls: Vec<MultisigCall>,
    pub broadcast_transaction_hash: Option<String>,
    /// The package a co-signer imports (`argentMultisigIntentPackage`), compact JSON.
    pub package: String,
}

pub(crate) fn intent_view(intent: &SigningIntent) -> MultisigIntent {
    let prepared = &intent.prepared;
    MultisigIntent {
        id: intent.id.clone(),
        multisig_id: intent.account_id.clone(),
        state: intent.state,
        summary: intent.summary.clone(),
        transaction_hash: prepared.transaction_hash.clone(),
        nonce: prepared.nonce.clone(),
        threshold: intent.config_snapshot.threshold,
        signer_guids: intent.config_snapshot.signer_guids.clone(),
        signed_public_keys: intent
            .signatures
            .iter()
            .map(|signature| signature.public_key.clone())
            .collect(),
        max_fee: prepared.fee().ok().map(|quote| fee_amount(&quote)),
        calls: prepared.calls.clone(),
        broadcast_transaction_hash: intent.broadcast_transaction_hash.clone(),
        package: intent.package().to_json(),
    }
}

/// The wallet's review of a proposal's calls, before anything is read
/// (`ArgentMultisigCallReview`): a signer change, with the configuration it leaves, or the
/// summary of ordinary calls.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MultisigCallReview {
    pub summary: String,
    pub change: Option<ConfigChange>,
    pub next_config: Option<ArgentMultisigConfig>,
}

/// Why a multisig plan ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum MultisigTxFailure {
    WrongChain,
    /// A read failed or answered with something unreadable. Nothing was sent.
    PreparationUnavailable,
    NodeError,
    BoundsOutOfRange,
    /// The wallet locked before the intent was stored or the write left.
    Locked,
    /// The node refused the frozen fee ceiling; the intent is now stale. Not submitted.
    FeeExpired,
    /// The node answered with another hash: it may have accepted something; check the chain.
    HashMismatch,
    /// The write's outcome is unknown: check the chain (the intent's hash) before retrying.
    SubmissionIndeterminate,
    /// The proposal could not be stored (the store is full, or the session moved).
    NotStored,
}

impl From<MultisigFailure> for MultisigTxFailure {
    fn from(failure: MultisigFailure) -> Self {
        match failure {
            MultisigFailure::WrongChain => Self::WrongChain,
            MultisigFailure::PreparationUnavailable => Self::PreparationUnavailable,
            MultisigFailure::NodeError => Self::NodeError,
            MultisigFailure::BoundsOutOfRange => Self::BoundsOutOfRange,
            MultisigFailure::Locked => Self::Locked,
            MultisigFailure::FeeExpired => Self::FeeExpired,
            MultisigFailure::HashMismatch => Self::HashMismatch,
            MultisigFailure::SubmissionIndeterminate => Self::SubmissionIndeterminate,
        }
    }
}

/// The next thing the host must do for a multisig plan.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum MultisigTxStep {
    /// POST `body` and reply `StepReply::Rpc` (same `id`) or `StepReply::TransportFailure`. A
    /// write is sent exactly once and never redirected.
    Rpc {
        id: u64,
        body: String,
        purpose: RpcPurpose,
        is_write: bool,
    },
    /// Terminal: the proposal is frozen and stored, collecting signatures; the proposal's
    /// `intent()` is the stored intent.
    Proposed { intent_id: String },
    /// Terminal: the node accepted exactly the frozen hash.
    Submitted { hash: String },
    /// Terminal. The plan is closed.
    Failed { failure: MultisigTxFailure },
}

fn rpc_step(step: MultisigStep) -> Result<MultisigTxStep, MultisigStep> {
    match step {
        MultisigStep::Rpc {
            id,
            body,
            purpose,
            is_write,
        } => Ok(MultisigTxStep::Rpc {
            id,
            body,
            purpose,
            is_write,
        }),
        MultisigStep::Failed(failure) => Ok(MultisigTxStep::Failed {
            failure: failure.into(),
        }),
        other => Err(other),
    }
}

/// A plan, closed once it reaches a terminal step, is cancelled, or its wallet is gone.
struct Driven<P> {
    plan: P,
    closed: bool,
}

/// What every plan shares: the wallet, the session generation it was made in, and the plan.
struct Bound<P> {
    core: Weak<WalletCore>,
    generation: u64,
    plan: Mutex<Driven<P>>,
}

impl<P> Bound<P> {
    fn new(core: &Arc<WalletCore>, generation: u64, plan: P) -> Self {
        Self {
            core: Arc::downgrade(core),
            generation,
            plan: Mutex::new(Driven {
                plan,
                closed: false,
            }),
        }
    }

    // A poisoned lock means a panic mid-step: close the plan rather than continue.
    fn lock(&self) -> MutexGuard<'_, Driven<P>> {
        self.plan.lock().unwrap_or_else(|poisoned| {
            let mut guard = poisoned.into_inner();
            guard.closed = true;
            guard
        })
    }

    /// The wallet, while its session is still the one the plan was made in.
    fn current(&self) -> Option<Arc<WalletCore>> {
        self.core
            .upgrade()
            .filter(|core| core.session_generation() == Some(self.generation))
    }

    /// Runs one step, closing the plan when it ends or the wallet is gone.
    fn step(
        &self,
        run: impl FnOnce(&mut P, &Self) -> Result<MultisigTxStep, SendError>,
    ) -> Result<MultisigTxStep, SendError> {
        let mut driven = self.lock();
        if driven.closed || self.core.upgrade().is_none() {
            driven.closed = true;
            return Err(SendError::PlanClosed);
        }
        let step = run(&mut driven.plan, self);
        if !matches!(step, Ok(MultisigTxStep::Rpc { .. })) {
            driven.closed = true;
        }
        step
    }
}

/// A proposal (`prepareIntent`): chain id, nonce and one validated estimate, then the frozen
/// transaction, stored as a collecting intent under this wallet's own summary.
#[derive(uniffi::Object)]
pub struct MultisigProposal {
    bound: Bound<ProposePlan>,
    multisig_id: String,
    snapshot: ConfigSnapshot,
    review: MultisigCallReview,
    stored: Mutex<Option<MultisigIntent>>,
}

impl MultisigProposal {
    pub(crate) fn new(
        core: &Arc<WalletCore>,
        generation: u64,
        plan: ProposePlan,
        multisig_id: String,
        snapshot: ConfigSnapshot,
        review: MultisigCallReview,
    ) -> Arc<Self> {
        Arc::new(Self {
            bound: Bound::new(core, generation, plan),
            multisig_id,
            snapshot,
            review,
            stored: Mutex::new(None),
        })
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
        self.bound.step(|plan, bound| {
            let prepared = match rpc_step(plan.advance(reply)?) {
                Ok(step) => return Ok(step),
                Err(MultisigStep::Prepared(prepared)) => *prepared,
                Err(_) => return Err(SendError::OutOfOrder),
            };
            let failure = |failure| Ok(MultisigTxStep::Failed { failure });
            // `authorized.isCurrent()` before the store, as the extension checks it.
            let Some(core) = bound.current() else {
                return failure(MultisigTxFailure::Locked);
            };
            let Some(intent) = SigningIntent::proposed(
                &self.multisig_id,
                self.snapshot.clone(),
                prepared,
                &self.review.summary,
            ) else {
                return failure(MultisigTxFailure::NotStored);
            };
            match multisig_intents::upsert(core.data_dir(), &intent) {
                Ok(()) => {
                    *self.stored.lock().unwrap_or_else(PoisonError::into_inner) =
                        Some(intent_view(&intent));
                    Ok(MultisigTxStep::Proposed {
                        intent_id: intent.id,
                    })
                }
                Err(_) => failure(MultisigTxFailure::NotStored),
            }
        })
    }

    /// Stops the plan. Idempotent; nothing is stored after it.
    #[uniffi::method(name = "cancel")]
    pub fn close(&self) {
        self.bound.lock().closed = true;
    }
}

/// A submission (`submitIntent`): proves the chain, then sends the frozen transaction with the
/// intent's threshold signature exactly once. On acceptance the intent is stored submitted; on
/// the node's fee refusal it is stored stale.
#[derive(uniffi::Object)]
pub struct MultisigSubmission {
    bound: Bound<SubmitPlan>,
    intent: SigningIntent,
}

impl MultisigSubmission {
    pub(crate) fn new(
        core: &Arc<WalletCore>,
        generation: u64,
        plan: SubmitPlan,
        intent: SigningIntent,
    ) -> Arc<Self> {
        Arc::new(Self {
            bound: Bound::new(core, generation, plan),
            intent,
        })
    }
}

#[uniffi::export]
impl MultisigSubmission {
    /// The intent being submitted, as it was when the plan was made.
    pub fn intent(&self) -> MultisigIntent {
        intent_view(&self.intent)
    }

    /// Feeds the last reply (`Start` first). The session is checked once more after the chain is
    /// proven and before the write leaves; a lock ends the plan `Locked`.
    pub fn advance(&self, reply: StepReply) -> Result<MultisigTxStep, SendError> {
        self.bound.step(|plan, bound| {
            let step = plan.advance(reply, &|| bound.current().is_some())?;
            // Best effort, as the extension's: the node's answer stands whatever the store says.
            let store = |next: SigningIntent| {
                if let Some(core) = bound.core.upgrade() {
                    let _ = multisig_intents::upsert(core.data_dir(), &next);
                }
            };
            match rpc_step(step) {
                Ok(step) => {
                    if step
                        == (MultisigTxStep::Failed {
                            failure: MultisigTxFailure::FeeExpired,
                        })
                    {
                        store(self.intent.marked_stale(StaleReason::Fee));
                    }
                    Ok(step)
                }
                Err(MultisigStep::Submitted { hash }) => {
                    store(SigningIntent {
                        broadcast_transaction_hash: Some(hash.clone()),
                        state: IntentState::Submitted,
                        ..self.intent.clone()
                    });
                    Ok(MultisigTxStep::Submitted { hash })
                }
                Err(_) => Err(SendError::OutOfOrder),
            }
        })
    }

    /// Stops the plan. Idempotent; once the write was handed out its outcome is the chain's.
    #[uniffi::method(name = "cancel")]
    pub fn close(&self) {
        self.bound.lock().closed = true;
    }
}
