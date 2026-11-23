//! `Plan`: one reviewed transfer or call batch (or standalone deployment, or status refresh) as a core-owned
//! state machine. The host only relays request bodies and replies; it cannot reorder steps, skip
//! the re-estimate, substitute bounds, or hand in a hash. The step order is the extension's
//! recorded wire order (`extension-transactions-*.json`).
//!
//! What a wallet owns comes in from outside (`types`): its numbers as a `Policy`, its account
//! catalog inside `Authorization`, and its session, key custody and activity log through `PlanHost`,
//! which every `advance` borrows. Review steps live in `plan_review`, account-state reads in
//! `plan_probe`, the submit path in `plan_submit` and the deployment in `plan_deploy`.

use super::fees::{Bounds, FeeQuote, TransactionFee};
pub use super::invoke::{ActivityAsset, Invoke, InvokeRefusal};
use super::judge::{self, Observation, TxStatus};
use super::review_flags::RiskFlag;
pub use super::types::{
    Account, ArgentClass, Authorization, Kind, Policy, Token, Transfer, TransferRefusal,
};
use super::types::{
    RpcPurpose, SendError, SendFailure, SendFailureCode, SendStep, StepReply, TransferReview,
};
use crate::allowances::calls::AllowanceChange;
use crate::kms::StarkSignature;
use crate::{felt, Felt};

/// starknet.js `waitForTransaction`: poll interval and attempt budget for a deployment.
pub const DEPLOY_POLL_INTERVAL_MS: u32 = 5_000;
pub const DEPLOY_POLL_ATTEMPTS: u32 = 200;

/// Why the host would not sign.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignRefusal {
    /// The session the plan was prepared in has ended.
    Locked,
    Failed,
}

/// What a pending activity record describes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingKind {
    Transfer {
        amount: String,
        recipient: Felt,
        symbol: String,
        token_address: Felt,
    },
    /// A call batch; the record's summary is its review's, or the text its builder named.
    Invoke {
        /// The amount and token the batch's builder named for the row (a swap, a stake).
        asset: Option<ActivityAsset>,
        /// Every allowance the batch changes. The host files them under the record's hash
        /// (`allowances::calls::record_changes`) before it writes the row, as the extension
        /// does for any hash that may be on chain: a row that fails to write must not lose a
        /// grant from the list a person revokes from.
        allowances: Vec<AllowanceChange>,
    },
    Deployment,
}

/// The record a wallet writes before a signed write leaves the core, keyed by its hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRecord {
    pub hash: Felt,
    pub kind: PendingKind,
    pub summary: String,
    pub submitted_at: u64,
}

/// The wallet behind a plan: its session, its key custody for the plan's account, and its
/// activity log. Lent to every `advance`, so a plan holds no reference to the wallet.
pub trait PlanHost {
    /// Whether the session the plan was prepared in is still current. Checked at Confirm.
    fn session_current(&self) -> bool;
    /// Signs `hash` with the plan's account key, still under that session. `renew` is false for
    /// a review's signature (Braavos's deployment estimate), which must not renew an idle timer.
    fn sign(&self, hash: &Felt, renew: bool) -> Result<StarkSignature, SignRefusal>;
    /// Persists a pending record before its signed write leaves the core. Best effort: a failure
    /// never blocks or rewrites a send. The plan never removes one: whatever the node answers, a
    /// signed write may land, so the record settles only through `observe`.
    fn record_pending(&self, record: PendingRecord);
    /// Merges a status observation into the record for `hash`; its settled status, if recorded.
    fn observe(&self, hash: &Felt, observation: Observation) -> Option<TxStatus>;
    /// Whether the record for `hash` still wants the fee its receipt reports (`wantsFee`: this
    /// wallet's row, no fee yet, an outcome with a receipt). A host that keeps no fee says no.
    fn wants_fee(&self, _hash: &Felt) -> bool {
        false
    }
    /// Records the receipt's fee on the record for `hash`. Best effort, like `record_pending`.
    fn record_fee(&self, _hash: &Felt, _fee: &TransactionFee) {}
    /// The explorer page for a transaction hash, if the wallet has one for the plan's network.
    fn explorer_url(&self, hash: &str) -> Option<String>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeploymentFor {
    Standalone,
    Invoke(Bounds),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeploymentPoll {
    pub hash: Felt,
    pub bounds: Bounds,
    pub attempts: u32,
}

/// Which reply the plan is waiting for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Expect {
    ReviewChain,
    /// The account-state reads (`plan_probe`): the accepted block, the class, and the signer.
    Block(Probe),
    Class(Probe),
    Owner(Probe),
    OwnerType(Probe),
    Guardian(Probe),
    /// Braavos: `get_signers`, then `get_deferred_remove_signers` and the block's timestamp
    /// only when a 2FA signer exists.
    Signers(Probe),
    DeferredRemoval(Probe),
    BlockTime(Probe),
    ReviewNonce,
    ReviewEstimate,
    ReviewDeployNonce,
    ReviewDeployEstimate,
    SubmitChain,
    SubmitNonce,
    SubmitEstimate,
    DeployNonce(Bounds),
    DeployEstimate(DeploymentFor),
    DeployWrite {
        hash: Felt,
        purpose: DeploymentFor,
    },
    DeployStatus(DeploymentPoll),
    DeployReceipt(DeploymentPoll),
    InvokeNonce {
        bounds: Bounds,
        deployment: Option<Felt>,
    },
    InvokeWrite {
        hash: Felt,
        deployment: Option<Felt>,
    },
    ConfirmStatus,
    RefreshStatus,
    /// The receipt of a transaction that settled with this status, read for its fee.
    FeeReceipt(TxStatus),
}

pub(crate) use super::plan_probe::Probe;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AfterWait {
    DeployStatus(DeploymentPoll),
    DeployReceipt(DeploymentPoll),
    Checkpoint,
}

enum Stage {
    Fresh,
    Awaiting { id: u64, expect: Expect },
    Waiting(AfterWait),
    Reviewed,
    Broadcasted,
    Finished(TxStatus),
    Closed,
}

/// Values gathered while the plan runs.
#[derive(Default)]
pub(crate) struct Work {
    pub now_ms: u64,
    pub next_id: u64,
    /// Review: `Some(deployed)` once `ClassHashAt` answered.
    pub deployed: Option<bool>,
    pub nonce: Felt,
    /// The accepted block the current account-state reads are pinned to.
    pub block: Felt,
    /// `get_owner`, held until `get_owner_type` answers (the extension reads both at once).
    pub owner: Option<judge::CallAnswer>,
    /// Braavos: when the requested 2FA removal expires, held until the block's timestamp answers.
    pub expire_at: Felt,
    pub fee: Option<FeeQuote>,
    pub deploy_fee: Option<FeeQuote>,
    pub requires_deployment: bool,
    pub already_deployed: bool,
    pub reviewed_fee: Option<FeeQuote>,
    pub reviewed_deploy_fee: Option<FeeQuote>,
    pub submit_deployed: bool,
    /// Signed deploy hash retained for standalone confirmation and interruption reporting.
    pub deploy_hash: Option<Felt>,
    /// Re-pins used for the account-state read in progress (`atAcceptedBlock`, three tries).
    pub pin_attempts: u8,
    /// Signed invoke hash retained for confirmation and interruption reporting.
    pub invoke_hash: Option<Felt>,
    pub checkpoint: usize,
}

/// One plan. Dropped on any edit, lock or dismissal; a new plan means a new review.
pub struct Plan {
    pub(crate) account: Account,
    pub(crate) kind: Kind,
    pub(crate) policy: Policy,
    stage: Stage,
    pub(crate) work: Work,
    pub(crate) signatures: u32,
    review: Option<TransferReview>,
}

impl Plan {
    /// A plan for `kind` from `account`. A deployment that does not reproduce the account's
    /// address is dropped, so such an account is never deployed from here.
    pub fn new(mut account: Account, kind: Kind, policy: Policy) -> Self {
        let address = account.address;
        account.deployment = account
            .deployment
            .filter(|deployment| deployment.address() == Some(address));
        Self {
            account,
            kind,
            policy,
            stage: Stage::Fresh,
            work: Work::default(),
            signatures: 0,
            review: None,
        }
    }

    /// Feeds the last reply and returns the next thing the host must do. A reply whose id is not
    /// the one outstanding, or a reply kind the current step does not expect, returns
    /// `OutOfOrder` and closes the plan (fail closed). A `Failed` step also closes it.
    pub fn advance(
        &mut self,
        host: &dyn PlanHost,
        reply: StepReply,
    ) -> Result<SendStep, SendError> {
        if matches!(self.stage, Stage::Closed) {
            return Err(SendError::PlanClosed);
        }
        let result = self.step(host, reply);
        if matches!(result, Ok(SendStep::Failed { .. }) | Err(_)) {
            self.stage = Stage::Closed;
        }
        result
    }

    /// The review the last `Review` step announced; `None` before the first review.
    pub fn current_review(&self) -> Option<&TransferReview> {
        self.review.as_ref()
    }

    /// The flags to show beside `current_review`: a batch's `Invoke::flags`, read from the batch
    /// this plan signs, so the card confirmed and the calls signed are one object; empty for any
    /// other kind (a transfer's are `TransferReview::flags`). Kept off `TransferReview`, whose
    /// UniFFI record would otherwise export `RiskFlag`.
    pub fn risk_flags(&self) -> Vec<&RiskFlag> {
        match &self.kind {
            Kind::Invoke(invoke) => invoke.flags(),
            _ => Vec::new(),
        }
    }

    /// How many hashes this plan has had signed, so a test can prove a refusal reached no signer.
    pub fn signatures_made(&self) -> u32 {
        self.signatures
    }

    /// What to report if the plan stops after a signed write may have left the device: the
    /// invoke as `SubmissionIndeterminate`, an automatic deployment as `DeploymentUnconfirmed`,
    /// each with its hash. `None` while no write has been handed out, when "not sent" is true.
    pub fn interrupted_failure(&self) -> Option<SendFailure> {
        let work = &self.work;
        let (code, hash) = match (work.invoke_hash, work.deploy_hash) {
            (Some(hash), _) => (SendFailureCode::SubmissionIndeterminate, hash),
            (None, Some(hash)) if self.is_invoke() => {
                (SendFailureCode::DeploymentUnconfirmed, hash)
            }
            (None, Some(hash)) => (SendFailureCode::SubmissionIndeterminate, hash),
            (None, None) => return None,
        };
        Some(SendFailure {
            code,
            hash: Some(felt::canonical(&hash)),
            detail: None,
        })
    }

    /// Stops the plan: no further step, and so no further signing point, can be reached.
    /// Idempotent. It never erases the pending record of a write the plan already handed out.
    pub fn close(&mut self) {
        self.stage = Stage::Closed;
    }

    fn step(&mut self, host: &dyn PlanHost, reply: StepReply) -> Result<SendStep, SendError> {
        let stage = std::mem::replace(&mut self.stage, Stage::Closed);
        match (stage, reply) {
            (Stage::Fresh, StepReply::Start) | (Stage::Reviewed, StepReply::RetryEstimate) => {
                Ok(self.start())
            }
            (
                Stage::Awaiting { id, expect },
                StepReply::Rpc {
                    id: got,
                    body,
                    now_ms,
                },
            ) if id == got => {
                self.work.now_ms = now_ms;
                Ok(self.on_reply(host, expect, judge::answer(&body, id)))
            }
            (Stage::Awaiting { id, expect }, StepReply::TransportFailure { id: got, now_ms })
                if id == got =>
            {
                self.work.now_ms = now_ms;
                Ok(self.on_reply(host, expect, None))
            }
            (Stage::Waiting(after), StepReply::WaitElapsed) => Ok(self.after_wait(after)),
            (Stage::Reviewed, StepReply::Confirm) => self.confirm(host),
            (Stage::Broadcasted, StepReply::Proceed)
            | (Stage::Finished(TxStatus::Pending), StepReply::CheckAgain) => {
                Ok(self.checkpoint(host, 0))
            }
            _ => Err(SendError::OutOfOrder),
        }
    }

    fn start(&mut self) -> SendStep {
        let (now_ms, next_id) = (self.work.now_ms, self.work.next_id);
        self.work = Work {
            now_ms,
            next_id,
            ..Work::default()
        };
        // A multisig authorises a send with its members' collected signatures, never one key's:
        // this plan only deploys it.
        if self.account.is_multisig() && self.is_invoke() {
            return self.fail(SendFailureCode::AccountClassUnsupported, None);
        }
        // The plan holds a batch to its own account, network and policy, whatever the host built
        // it with: reviewed for another account, a call to this one would not have been refused;
        // on another network, its known tokens and trusted addresses are not this one's.
        if let Kind::Invoke(invoke) = &self.kind {
            let checked = super::invoke::check(&self.account.address, &invoke.calls, &self.policy);
            let own =
                invoke.sender == self.account.address && invoke.network == self.account.network;
            if !own || checked.is_err() {
                return self.fail(SendFailureCode::PreparationUnavailable, None);
            }
        }
        if let Kind::Refresh { hash } = self.kind {
            return self.rpc(
                Expect::RefreshStatus,
                RpcPurpose::TransactionStatus,
                false,
                |_, id| super::rpc_bodies::transaction_status(id, &hash),
            );
        }
        self.rpc(Expect::ReviewChain, RpcPurpose::ChainId, false, |_, id| {
            crate::rpc::chain_id_request(id)
        })
    }

    fn on_reply(
        &mut self,
        host: &dyn PlanHost,
        expect: Expect,
        answer: Option<judge::Answer>,
    ) -> SendStep {
        match expect {
            Expect::Block(_)
            | Expect::Class(_)
            | Expect::Owner(_)
            | Expect::OwnerType(_)
            | Expect::Guardian(_)
            | Expect::Signers(_)
            | Expect::DeferredRemoval(_)
            | Expect::BlockTime(_) => self.on_probe_reply(expect, answer),
            Expect::ReviewChain
            | Expect::ReviewNonce
            | Expect::ReviewEstimate
            | Expect::ReviewDeployNonce
            | Expect::ReviewDeployEstimate => self.on_review_reply(host, expect, answer),
            _ => self.on_submit_reply(host, expect, answer),
        }
    }

    fn after_wait(&mut self, after: AfterWait) -> SendStep {
        match after {
            AfterWait::DeployStatus(poll) => self.poll_deployment(poll),
            AfterWait::DeployReceipt(poll) => self.read_deploy_receipt(poll),
            AfterWait::Checkpoint => {
                let Some(hash) = self.target_hash() else {
                    return self.fail(SendFailureCode::SubmissionIndeterminate, None);
                };
                self.rpc(
                    Expect::ConfirmStatus,
                    RpcPurpose::TransactionStatus,
                    false,
                    |_, id| super::rpc_bodies::transaction_status(id, &hash),
                )
            }
        }
    }

    /// The hash the confirmation checkpoints follow.
    pub(crate) fn target_hash(&self) -> Option<Felt> {
        match self.kind {
            Kind::Transfer(_) | Kind::Invoke(_) => self.work.invoke_hash,
            Kind::Deployment => self.work.deploy_hash,
            Kind::Refresh { hash } => Some(hash),
        }
    }

    /// Emits one JSON-RPC step under a fresh id.
    pub(crate) fn rpc(
        &mut self,
        expect: Expect,
        purpose: RpcPurpose,
        is_write: bool,
        body: impl FnOnce(&Self, u64) -> String,
    ) -> SendStep {
        self.work.next_id += 1;
        let id = self.work.next_id;
        let body = body(self, id);
        self.stage = Stage::Awaiting { id, expect };
        SendStep::Rpc {
            id,
            body,
            purpose,
            is_write,
        }
    }

    pub(crate) fn wait(&mut self, delay_ms: u32, after: AfterWait) -> SendStep {
        self.stage = Stage::Waiting(after);
        SendStep::Wait { delay_ms }
    }

    pub(crate) fn fail(&mut self, code: SendFailureCode, hash: Option<Felt>) -> SendStep {
        self.stage = Stage::Closed;
        SendStep::Failed {
            failure: SendFailure {
                code,
                hash: hash.map(|h| felt::canonical(&h)),
                detail: None,
            },
        }
    }

    pub(crate) fn reviewed(&mut self, review: TransferReview) -> SendStep {
        self.stage = Stage::Reviewed;
        self.review = Some(review);
        SendStep::Review
    }

    pub(crate) fn broadcast(&mut self, hash: Felt) -> SendStep {
        self.stage = Stage::Broadcasted;
        SendStep::Broadcast {
            hash: felt::canonical(&hash),
        }
    }

    pub(crate) fn finish(&mut self, host: &dyn PlanHost, status: TxStatus, hash: Felt) -> SendStep {
        self.stage = Stage::Finished(status);
        let hash = felt::canonical(&hash);
        SendStep::Finished {
            status,
            explorer_url: host.explorer_url(&hash),
            hash,
        }
    }

    /// Starts (or restarts) the confirmation checkpoints at `index`.
    pub(crate) fn checkpoint(&mut self, host: &dyn PlanHost, index: usize) -> SendStep {
        let Some(hash) = self.target_hash() else {
            return self.fail(SendFailureCode::SubmissionIndeterminate, None);
        };
        match self.policy.confirmation_delays_ms.get(index) {
            Some(delay) => {
                self.work.checkpoint = index;
                self.wait(*delay, AfterWait::Checkpoint)
            }
            None => self.finish(host, TxStatus::Pending, hash),
        }
    }
}
