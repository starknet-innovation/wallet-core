//! `TransferPlan`: mobile's FFI object around `wallet_core::send::plan::Plan`, the core-owned
//! state machine for one reviewed transfer (or standalone deployment, or status refresh). The
//! step order is the extension's recorded wire order; `mobile/docs/transactions.md` §3.
//!
//! The wrapper holds what ties a plan to this wallet: the vault record it signs for, the session
//! generation it was prepared in, and the plan's `PlanHost` (`impl PlanHost for TransferPlan`
//! below), which maps the core's session check, signing, pending records, observations and
//! explorer links onto `WalletCore`, the activity store and the explorer catalog.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use wallet_core::activity::detail::submitted_price;
use wallet_core::allowances::calls::record_changes;
use wallet_core::allowances::retention::Ranking;
use wallet_core::history::activity_rules::price_json;
use wallet_core::send::fees::TransactionFee;
use wallet_core::send::judge::{Observation, TxStatus};
use wallet_core::send::plan::{Kind, PendingKind, PendingRecord, Plan, PlanHost, SignRefusal};
use wallet_core::{Felt, StarkSignature};

use super::types::{
    ActivityKind, ReviewedCall, RiskFlag, SendError, SendFailure, SendStep, StepReply,
    TransferReview,
};
use super::wallet_ext::PlanAccount;
use crate::allowances::with_allowance_log;
use wallet_core::activity::store::{kind_text, Entry};
use wallet_core::history::activity::TransferDirection;
use wallet_core::history::activity_rules::direction_text;

use crate::activity::with_log;
use crate::networks::NetworkId;
use crate::portfolio::model::DisplayCurrency;
use crate::portfolio::prices::{PriceBook, PricePlan};
use crate::wallet::WalletCore;
use crate::{felt, CoreError};

/// Bramble's send numbers, until 2.7 moves them to `bramble-core`: the same values as its
/// `FEE_MARGIN_PERCENT`, `REVIEWED_FEE_HEADROOM` and `CONFIRMATION_DELAYS_MS`, each checked
/// against `extension-transactions-rules.json` (`fixture_tests.rs`).
pub(crate) const FEE_MARGIN_PERCENT: u32 = 50;
pub(crate) const REVIEWED_FEE_HEADROOM: wallet_core::send::fees::Headroom =
    wallet_core::send::fees::Headroom {
        numerator: 3,
        denominator: 2,
    };
/// The confirmation checkpoints of the extension's `CONFIRMATION_DELAYS_MS`.
pub const CONFIRMATION_DELAYS_MS: [u32; 12] = [
    0, 250, 500, 900, 1_500, 2_500, 4_000, 5_000, 7_500, 10_000, 10_000, 10_000,
];
pub(crate) const SEND_POLICY: wallet_core::send::plan::Policy = wallet_core::send::plan::Policy {
    fee_margin_percent: FEE_MARGIN_PERCENT,
    reviewed_fee_headroom: REVIEWED_FEE_HEADROOM,
    confirmation_delays_ms: &CONFIRMATION_DELAYS_MS,
    max_invoke_calls: crate::MAX_INVOKE_CALLS,
    max_call_calldata_felts: crate::MAX_CALL_CALLDATA_FELTS,
};

/// One plan. Dropped on any edit, lock or dismissal; a new plan means a new review.
#[derive(uniffi::Object)]
pub struct TransferPlan {
    core: Weak<WalletCore>,
    /// The vault record's id, `<networkId>:<address>` (the extension's account id). It scopes
    /// activity and is what every signature for this plan is requested by.
    account_id: String,
    network: NetworkId,
    /// The session generation (`lock_epoch`) the plan was prepared in.
    generation: u64,
    plan: Mutex<Plan>,
    /// Where a public send's price at submission comes from (`price_with`).
    prices: Mutex<Option<(Arc<PriceBook>, DisplayCurrency)>>,
    /// What a batch's allowance changes are evicted by (`allowances::ranking`).
    ranking: Ranking,
}

impl TransferPlan {
    /// A plan for `kind` from `acting`. A batch's allowance changes are filed ranked by
    /// `ranking`: the trusted tokens its review read.
    pub(crate) fn new(
        core: &Arc<WalletCore>,
        acting: PlanAccount,
        kind: Kind,
        ranking: Ranking,
    ) -> Arc<Self> {
        let plan = Plan::new(acting.account.clone(), kind, SEND_POLICY);
        Self::with_plan(core, acting, plan, ranking)
    }

    /// A plan built elsewhere (a multisig deployment), signed for and recorded under `acting`.
    pub(crate) fn with_plan(
        core: &Arc<WalletCore>,
        acting: PlanAccount,
        plan: Plan,
        ranking: Ranking,
    ) -> Arc<Self> {
        Arc::new(Self {
            core: Arc::downgrade(core),
            account_id: acting.account_id,
            network: acting.network,
            generation: acting.generation,
            plan: Mutex::new(plan),
            prices: Mutex::new(None),
            ranking,
        })
    }

    fn lock_plan(&self) -> MutexGuard<'_, Plan> {
        // A poisoned lock means a panic mid-step: close the plan rather than continue.
        self.plan.lock().unwrap_or_else(|poisoned| {
            let mut guard = poisoned.into_inner();
            guard.close();
            guard
        })
    }

    /// How many hashes this plan has signed; for tests that prove a refusal reached no signer.
    #[doc(hidden)]
    pub fn signatures_made(&self) -> u32 {
        self.lock_plan().signatures_made()
    }
}

#[uniffi::export]
impl TransferPlan {
    /// Feeds the last reply and returns the next thing the host must do. A reply whose id is not
    /// the one outstanding, or a reply kind the current step does not expect, returns
    /// `OutOfOrder` and closes the plan (fail closed). A `Failed` step also closes it.
    // An error never means "nothing was sent": `interrupted_failure` says whether a signed write
    // left (the hosts call it on every error; `transactions_lifecycle.rs` pins it).
    pub fn advance(&self, reply: StepReply) -> Result<SendStep, SendError> {
        let mut plan = self.lock_plan();
        if self.core.upgrade().is_none() {
            plan.close();
            return Err(SendError::PlanClosed);
        }
        plan.advance(self, reply).map_err(SendError::from)
    }

    /// The review the last `Review` step announced; `None` before the first review.
    pub fn current_review(&self) -> Option<TransferReview> {
        self.lock_plan().current_review().cloned()
    }

    /// What the journey must report if the plan stops (closed by a lock, backgrounding, or
    /// dismissal) after a signed write may have left the device: the invoke as
    /// `SubmissionIndeterminate`, an automatic deployment as `DeploymentUnconfirmed` (the
    /// extension's "Account deployment was broadcast as <hash>, but confirmation is unknown"),
    /// each with its hash. `None` while no write has been handed out, when "not sent" is true.
    pub fn interrupted_failure(&self) -> Option<SendFailure> {
        self.lock_plan().interrupted_failure()
    }

    /// Stops the plan: no further step, and so no further signing point, can be reached.
    /// Idempotent. It never erases the pending activity record of a write the plan already
    /// handed out; that record is settled by status reconciliation.
    ///
    /// Exported as `cancel`: Kotlin's generated `AutoCloseable.close()` (which frees the handle)
    /// would otherwise clash with it.
    #[uniffi::method(name = "cancel")]
    pub fn close(&self) {
        self.lock_plan().close();
    }

    /// Prices the send's activity record as it is written, as the extension's
    /// `withSubmittedPrice`: the unit price `prices` holds for the token in `currency` (the
    /// display currency), read from its cache only and only while fresh. Without this call, or
    /// without a fresh price, the record carries none; a price never holds up a send.
    pub fn price_with(&self, prices: Arc<PriceBook>, currency: DisplayCurrency) {
        *self.prices.lock().unwrap_or_else(PoisonError::into_inner) = Some((prices, currency));
    }

    /// A batch's calls as its review shows them, in the order signed (`verifyCalls`'s `calls`),
    /// each with its flags; empty for a transfer, deployment or refresh.
    pub fn reviewed_calls(&self) -> Vec<ReviewedCall> {
        let plan = self.lock_plan();
        let Some(invoke) = plan.invoke() else {
            return Vec::new();
        };
        invoke
            .signed_calls()
            .iter()
            .zip(invoke.calls())
            .map(|(call, reviewed)| ReviewedCall {
                contract_address: felt::canonical(&call.contract_address),
                entrypoint: call.entrypoint.clone(),
                kind: reviewed.kind,
                summary: reviewed.summary.clone(),
                flags: reviewed.flags.iter().map(RiskFlag::from).collect(),
            })
            .collect()
    }

    /// A batch's flags as its card lists them, each once (`Plan::risk_flags`); empty for any
    /// other kind (a transfer's are `TransferReview::flags`).
    pub fn risk_flags(&self) -> Vec<RiskFlag> {
        let plan = self.lock_plan();
        plan.risk_flags().into_iter().map(RiskFlag::from).collect()
    }
}

impl TransferPlan {
    // The cached-only read the extension's `capturePrice` makes, at the record's own time.
    fn price_at_submission(
        &self,
        entry: &Entry,
    ) -> Option<wallet_core::history::activity::SubmittedPrice> {
        let (book, currency) = self
            .prices
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()?;
        let address = entry.token_address.clone()?;
        let now_ms = entry.submitted_at as f64;
        match book.plan(self.network, vec![address], currency, now_ms, true) {
            PricePlan::Serve { view } => submitted_price(entry, &view),
            PricePlan::Fetch { .. } => None,
        }
    }
}

/// What a pending record's row names: its kind, amount, counterparty, token symbol, token
/// address, and whether it moves value out. Only a public send does (`transfer-ops.ts`); a
/// batch's row names a token without a direction, as the extension's swap and staking rows do.
fn row(kind: PendingKind) -> (ActivityKind, Text, Text, Text, Text, bool) {
    match kind {
        PendingKind::Transfer {
            amount,
            recipient,
            symbol,
            token_address,
        } => (
            ActivityKind::Transfer,
            Some(amount),
            Some(felt::canonical(&recipient)),
            Some(symbol),
            Some(felt::canonical(&token_address)),
            true,
        ),
        // Its `allowances` were filed in the allowance log before this row (`record_pending`).
        PendingKind::Invoke { asset, .. } => match asset {
            Some(asset) => (
                ActivityKind::Invoke,
                asset.amount,
                None,
                Some(asset.symbol),
                Some(felt::canonical(&asset.token_address)),
                false,
            ),
            None => (ActivityKind::Invoke, None, None, None, None, false),
        },
        PendingKind::Deployment => (ActivityKind::Deploy, None, None, None, None, false),
    }
}

type Text = Option<String>;

/// The wallet behind the plan. Every call re-reads the wallet through the weak reference; a
/// dropped wallet is a moved session.
impl PlanHost for TransferPlan {
    fn session_current(&self) -> bool {
        self.core
            .upgrade()
            .is_some_and(|core| core.session_generation() == Some(self.generation))
    }

    /// `plan_sign` checks the generation under the session mutex on every call, so a lock after
    /// Confirm refuses the deployment, the invoke after a deployment wait, and Braavos's review
    /// estimate alike, not only Confirm.
    fn sign(&self, hash: &Felt, renew: bool) -> Result<StarkSignature, SignRefusal> {
        let core = self.core.upgrade().ok_or(SignRefusal::Locked)?;
        core.plan_sign(self.generation, &self.account_id, hash, renew)
            .map_err(|error| match error {
                CoreError::Locked | CoreError::NoWallet => SignRefusal::Locked,
                _ => SignRefusal::Failed,
            })
    }

    /// Best effort, as the extension's `recordSubmittedActivity`: a storage failure never blocks
    /// or rewrites a send.
    fn record_pending(&self, record: PendingRecord) {
        let Some(core) = self.core.upgrade() else {
            return;
        };
        // A batch's grants first, as the extension's `recordRevoke` files them: the log is what
        // keeps a residual grant listed and revokable, so it never waits on the row's write.
        if let PendingKind::Invoke { allowances, .. } = &record.kind {
            if !allowances.is_empty() {
                let hash = felt::canonical(&record.hash);
                with_allowance_log(&core.data_dir, |log| {
                    record_changes(log, allowances, &hash, &self.ranking)
                });
            }
        }
        let (kind, amount, counterparty, token, token_address, outgoing) = row(record.kind);
        let mut entry = Entry {
            account_id: self.account_id.clone(),
            amount,
            counterparty,
            direction: outgoing.then(|| direction_text(TransferDirection::Out).into()),
            failure_notice: None,
            fee_payer: None,
            user_fee: None,
            fee_paid: None,
            finalized_on_l1: None,
            hash: felt::canonical(&record.hash),
            kind: kind_text(kind).into(),
            network_id: self.network.catalog_id().into(),
            not_found_count: None,
            // The one writer of this marker (`recordSubmittedActivity`, audit #163).
            origin: Some("wallet".into()),
            status: "pending".into(),
            submitted_at: record.submitted_at,
            submitted_price: None,
            summary: record.summary,
            token,
            token_address,
        };
        entry.submitted_price = self.price_at_submission(&entry).map(|p| price_json(&p));
        with_log(&core.data_dir, |log| log.record(entry));
    }

    fn wants_fee(&self, hash: &Felt) -> bool {
        self.core.upgrade().is_some_and(|core| {
            with_log(&core.data_dir, |log| log.wants_fee(&self.account_id, hash))
        })
    }

    fn record_fee(&self, hash: &Felt, fee: &TransactionFee) {
        if let Some(core) = self.core.upgrade() {
            with_log(&core.data_dir, |log| {
                log.record_fee(&self.account_id, hash, fee)
            });
        }
    }

    fn observe(&self, hash: &Felt, observation: Observation) -> Option<TxStatus> {
        let core = self.core.upgrade()?;
        with_log(&core.data_dir, |log| {
            log.observe(&self.account_id, hash, observation)
        })
    }

    fn explorer_url(&self, hash: &str) -> Option<String> {
        crate::history::explorer::explorer_transaction_url(self.network, hash.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wallet_core::send::plan::ActivityAsset;

    #[test]
    fn a_batchs_row_names_its_token_without_a_direction_and_a_send_moves_value_out() {
        let token = Felt::from(0x51u8);
        let swap = PendingKind::Invoke {
            asset: Some(ActivityAsset {
                amount: Some("0.01".into()),
                symbol: "ETH".into(),
                token_address: token,
            }),
            allowances: Vec::new(),
        };
        let address = Some(felt::canonical(&token));
        assert_eq!(
            row(swap),
            (
                ActivityKind::Invoke,
                Some("0.01".into()),
                None,
                Some("ETH".into()),
                address.clone(),
                false
            )
        );
        let send = PendingKind::Transfer {
            amount: "1.5".into(),
            recipient: Felt::from(0x7u8),
            symbol: "STRK".into(),
            token_address: token,
        };
        let (kind, .., sent_token, outgoing) = row(send);
        assert_eq!(
            (kind, sent_token, outgoing),
            (ActivityKind::Transfer, address, true)
        );
        let bare = PendingKind::Invoke {
            asset: None,
            allowances: Vec::new(),
        };
        assert_eq!(
            row(bare),
            (ActivityKind::Invoke, None, None, None, None, false)
        );
    }
}
