//! Argent multisig transactions over the FFI (P1): the records mobile's screens read and the two
//! sans-IO plans, `MultisigProposal` (`prepareIntent`) and `MultisigSubmission`
//! (`submitIntent`), around `wallet_core::send::multisig_invoke_plan`. Neither plan signs or
//! holds a key: a proposal freezes the transaction and stores the collecting intent, and a
//! submission broadcasts the threshold signature the intent already holds. The entry points that
//! start them, co-sign and import are `multisig_tx_ops`.
//!
//! Every refusal is a fixed variant or one of the extension's fixed sentences; none carries a
//! key, a call's calldata or node text.

use std::sync::{Arc, Mutex, MutexGuard, Weak};

pub use wallet_core::accounts::multisig_change::ConfigChange;
use wallet_core::send::fees::{fee_amount, FeeAmount};
use wallet_core::send::multisig_intent::SigningIntent;
pub use wallet_core::send::multisig_intent::{IntentState, StaleReason};
pub use wallet_core::send::multisig_invoke::MultisigCall;
use wallet_core::send::multisig_invoke_plan::{MultisigFailure, MultisigStep};

use wallet_core::accounts::multisig_change::{review_config_change, ConfigReview};
use wallet_core::accounts::multisig_records::Record;
use wallet_core::js::felt::same_felt;
use wallet_core::send::call_decode::Known;
use wallet_core::send::invoke::{batch_flags, verify_calls};
use wallet_core::send::multisig_import::{held_config, ImportError};
use wallet_core::send::multisig_intent::ConfigSnapshot;
use wallet_core::send::multisig_invoke::as_calls;
use wallet_core::send::review_flags::ReviewContext;
use wallet_core::Felt;

use crate::allowances::{token_infos, trusted_tokens};
use crate::argent_multisig::ArgentMultisigConfig;
use crate::multisig_rpc::MultisigAuthorityRead;
use crate::networks::NetworkId;
use crate::portfolio::tokens::TokenPreferences;
use crate::send::types::{ReviewedCall, RiskFlag, RpcPurpose, SendError};
use crate::send::wallet_ext::known_token_addresses;
use crate::wallet::WalletCore;
use crate::{felt, multisig_constants, CoreError};

pub use crate::multisig_tx_plans::{MultisigProposal, MultisigSubmission};

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

/// Why an intent went stale (`ArgentMultisigStaleReason`): the signer set changed (`Config`),
/// the chain nonce moved past it (`Nonce`: a co-signer may have sent it), or the node refused its
/// fee while the nonce held (`Fee`). `Proof` is the extension's privacy pool, which mobile never
/// writes.
#[uniffi::remote(Enum)]
pub enum StaleReason {
    Config,
    Fee,
    Nonce,
    Proof,
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

/// What a proposal freezes. The host names a transfer or a signer change and the core builds
/// the call, so no export signs calls the host supplies (P9's rule, `ffi_surface.rs`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum MultisigProposalRequest {
    /// An ERC-20 `transfer` from the multisig: one of the person's trusted tracked tokens on its
    /// network by id (`TokenConfig::id`), any recipient spelling `validate_recipient` accepts, and
    /// decimal text as typed.
    Transfer {
        token_id: String,
        recipient: String,
        amount: String,
    },
    /// `argentMultisigConfigChangeCall`: the self-call that makes `change`.
    ChangeSigners { change: ConfigChange },
    /// "Update network fee" and "Propose again" (`data-ms-refresh-intent`, `openUpdatedIntent`):
    /// the stored calls of this multisig's stale intent `intent_id`, proposed again at the
    /// current nonce and fee and reviewed afresh. The host names the intent, never its calls.
    /// Open `multisig_fee_replacement` first, as the extension does, rather than propose twice.
    /// The calls may be a co-signer's imported batch: the proposal's `review()` is a summary
    /// only, so open the new intent's card (`multisig_intent_review`) before anyone signs it.
    Repropose { intent_id: String },
}

/// A stored signing intent as the screens show it. Everything here is public: the frozen
/// transaction, the snapshot the co-signers sign against, and who has signed.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MultisigIntent {
    /// `<multisig id>:<transaction hash>`.
    pub id: String,
    pub multisig_id: String,
    pub state: IntentState,
    /// Why it went stale, once it is.
    pub stale_reason: Option<StaleReason>,
    /// This wallet's own review of the calls, never a co-signer's text.
    pub summary: String,
    pub transaction_hash: String,
    /// The multisig the frozen transaction is from (`prepared.senderAddress`), canonical; the
    /// card shows it as `address_label` and copies it as `address_text`.
    pub sender: String,
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
    /// `configReviewFor`: the signer change the lone call makes, decoded against the multisig's
    /// stored configuration while that is still the intent's snapshot. `None` for anything else:
    /// more than one call, an ordinary call, a snapshot the stored configuration no longer
    /// holds, or a call the strict parser refuses.
    pub change: Option<ConfigChange>,
    /// The configuration `change` leaves; set exactly when `change` is.
    pub next_config: Option<ArgentMultisigConfig>,
}

/// `configReviewFor`: the change an intent's one call makes to the record's stored configuration,
/// only while that configuration's threshold and GUIDs (in order) are the intent's snapshot. The
/// stored configuration is `held_config`'s, as the proposal's review reads it: a record mobile
/// deployed keeps only its `deployment`.
fn config_review_for(intent: &SigningIntent, record: &Record) -> Option<ConfigReview> {
    let class = multisig_constants::argent_multisig_class();
    let [call] = intent.prepared.calls.as_slice() else {
        return None;
    };
    let current = held_config(record, &intent.config_snapshot, class)?;
    review_config_change(&record.address, &current, call, class).ok()
}

/// The screens' view of a stored intent. `record` is the multisig's stored record, read for
/// `change`; without it (a listing whose record is gone) no change is decoded.
pub(crate) fn intent_view(intent: &SigningIntent, record: Option<&Record>) -> MultisigIntent {
    let prepared = &intent.prepared;
    let review = record.and_then(|record| config_review_for(intent, record));
    MultisigIntent {
        id: intent.id.clone(),
        multisig_id: intent.account_id.clone(),
        state: intent.state,
        stale_reason: intent.stale_reason,
        summary: intent.summary.clone(),
        transaction_hash: prepared.transaction_hash.clone(),
        sender: prepared.sender_address.clone(),
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
        change: review.as_ref().map(|review| review.change.clone()),
        next_config: review.map(|review| ArgentMultisigConfig {
            threshold: review.next_config.threshold,
            signer_public_keys: review.next_config.signer_public_keys,
        }),
    }
}

/// What the extension's intent card (`intentCard`) shows of an intent beyond the stored record,
/// read at render time against the person's tokens and a fresh authority read
/// (`WalletCore::multisig_intent_review`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MultisigIntentReview {
    /// `verifyCalls(..).calls`: each frozen call, in order, with its `decodeCall` summary and the
    /// flags it raises.
    pub calls: Vec<ReviewedCall>,
    /// `verifyCalls(..).flags`: every call's flags, each line once, as the card's panel lists them.
    pub risk_flags: Vec<RiskFlag>,
    /// `configMatchesIntent`: the read is `Available` and the chain's threshold and signer GUIDs
    /// are the intent's snapshot. When false the card's panel adds
    /// `review.multisig.card.signersChanged` ("The on-chain signers no longer match this
    /// transaction. Signing and submission are disabled.") after the flags, and offers neither.
    pub has_fresh_authority: bool,
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
    /// A stale version of these calls is on chain: proposing them again would pay twice.
    AlreadySent,
    /// A stale version of these calls at another nonce may have sent them; propose again with
    /// `confirm_resend` once the person confirms.
    ResendUnconfirmed,
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

pub(crate) fn rpc_step(step: MultisigStep) -> Result<MultisigTxStep, MultisigStep> {
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
pub(crate) struct Driven<P> {
    pub(crate) plan: P,
    pub(crate) closed: bool,
}

/// What every plan shares: the wallet, the session generation it was made in, and the plan.
pub(crate) struct Bound<P> {
    pub(crate) core: Weak<WalletCore>,
    generation: u64,
    plan: Mutex<Driven<P>>,
}

impl<P> Bound<P> {
    pub(crate) fn new(core: &Arc<WalletCore>, generation: u64, plan: P) -> Self {
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
    pub(crate) fn lock(&self) -> MutexGuard<'_, Driven<P>> {
        self.plan.lock().unwrap_or_else(|poisoned| {
            let mut guard = poisoned.into_inner();
            guard.closed = true;
            guard
        })
    }

    /// The wallet, while its session is still the one the plan was made in.
    pub(crate) fn current(&self) -> Option<Arc<WalletCore>> {
        self.core
            .upgrade()
            .filter(|core| core.session_generation() == Some(self.generation))
    }

    /// Runs one step, closing the plan when it ends or the wallet is gone.
    pub(crate) fn step(
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

#[uniffi::export]
impl WalletCore {
    /// The intent card's review of the stored intent `intent_id` (`intentCard`): its calls and
    /// flags under the person's trusted tracked `tokens` on the multisig's network (as the
    /// extension's `verificationContext`, with no allowance log and no watched collections), and
    /// whether `read`, a fresh authority read of the multisig driven to its end, still holds the
    /// intent's snapshot. A signer change the stored configuration still reviews pins the
    /// multisig itself (`knownContracts`), as the extension's card does. Refuses a read of another
    /// account; reads nothing and writes nothing.
    pub fn multisig_intent_review(
        &self,
        intent_id: String,
        tokens: Arc<TokenPreferences>,
        read: Arc<MultisigAuthorityRead>,
    ) -> Result<MultisigIntentReview, CoreError> {
        self.require_unlocked()?;
        let intent = self.stored_intent(&intent_id)?;
        let record = self.multisig_record(&intent.account_id)?;
        let invalid = |reason: &str| CoreError::InvalidInput {
            reason: reason.into(),
        };
        let network = NetworkId::from_catalog_id(&record.network_id)
            .ok_or_else(|| invalid("Argent multisig not found"))?;
        if !same_felt(&read.address(), &record.address) || read.network() != network {
            return Err(invalid(ImportError::ReadForAnotherAccount.message()));
        }
        let (sender, calls) = felt::parse(&record.address)
            .zip(as_calls(&intent.prepared.calls))
            .ok_or_else(|| invalid("invalid Argent multisig intent"))?;
        let class = multisig_constants::argent_multisig_class();
        let config_change = config_review_for(&intent, &record).is_some();
        let trusted = trusted_tokens(&tokens, network);
        let infos = token_infos(&trusted);
        let token_contracts: Vec<Felt> = known_token_addresses(network)
            .into_iter()
            .chain(trusted.iter().filter_map(|t| felt::parse(&t.address)))
            .collect();
        let pinned = [sender];
        let context = ReviewContext {
            known: Known {
                tokens: &infos,
                nft_contracts: &[],
            },
            trusted: &[],
            known_contracts: if config_change { &pinned } else { &[] },
            token_contracts: &token_contracts,
            standing: &[],
        };
        let reviewed = verify_calls(network, &sender, &calls, &context);
        let has_fresh_authority = read.authority().is_some_and(|(threshold, guids)| {
            ConfigSnapshot::new(class, threshold, &guids) == intent.config_snapshot
        });
        Ok(MultisigIntentReview {
            calls: calls
                .iter()
                .zip(&reviewed)
                .map(|(call, reviewed)| ReviewedCall::new(call, reviewed))
                .collect(),
            risk_flags: batch_flags(&reviewed)
                .into_iter()
                .map(RiskFlag::from)
                .collect(),
            has_fresh_authority,
        })
    }
}
