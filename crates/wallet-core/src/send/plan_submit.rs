//! The submit half of a plan, in the extension's order (`submitInvokeTransaction`,
//! `ensureAccountDeployed`, `submitAccountDeployment`): prove the chain again, re-read deployment
//! state and nonce, re-estimate against the reviewed ceilings, deploy when still needed, sign,
//! broadcast once, then confirm. Every write is preceded by a persisted pending activity record
//! keyed by its signed hash.

use crate::Felt;

use super::calls::ReviewFlag;
use super::fees::{self, Bounds, FeeQuote, FeeRefusal};
use super::judge::TxStatus;
use super::judge::{self, Answer, ChainAnswer, EstimateAnswer, Observation, WriteAnswer};
use super::plan::{
    AfterWait, Expect, Kind, PendingKind, PendingRecord, Plan, PlanHost, SignRefusal,
};
use super::plan_probe::{Outcome, Probe};
use super::plan_review::DEPLOY_SUMMARY;
use super::rpc_bodies::{self, InvokeTx};
use super::sign;
use super::types::{RpcPurpose, SendError, SendFailureCode as Code, SendStep};
use crate::felt;
use crate::kms::StarkSignature;

/// What a re-estimate at submit produced.
pub(super) enum Settled {
    Bounds(Bounds),
    Refused(Code),
}

pub(super) fn refusal_code(refusal: FeeRefusal) -> Code {
    match refusal {
        FeeRefusal::BoundsOutOfRange => Code::BoundsOutOfRange,
        FeeRefusal::UnrecognisedUnit => Code::UnrecognisedFeeUnit,
        // A malformed estimate throws a non-RPC error in starknet.js, which `withRpcWrite`
        // reports as a preparation failure.
        FeeRefusal::Malformed => Code::PreparationUnavailable,
    }
}

/// `settledResourceBounds` / `checkedDeploymentBounds`: the entry's bounds, only when the spend
/// they authorise is within the policy's headroom over the reviewed spend in the same unit.
pub(super) fn settle(
    plan: &Plan,
    entry: Option<&serde_json::Value>,
    reviewed: &FeeQuote,
    exceeded: Code,
    missing: Code,
) -> Settled {
    let Some(entry) = entry else {
        return Settled::Refused(missing);
    };
    let policy = plan.policy;
    match fees::quote(entry, policy.fee_margin_percent) {
        Ok((_, quoted))
            if fees::exceeds_reviewed(reviewed, &quoted, policy.reviewed_fee_headroom) =>
        {
            Settled::Refused(exceeded)
        }
        Ok((bounds, _)) => match fees::krusty_bounds(&bounds) {
            Ok(_) => Settled::Bounds(bounds),
            Err(refusal) => Settled::Refused(refusal_code(refusal)),
        },
        Err(FeeRefusal::BoundsOutOfRange) => Settled::Refused(exceeded),
        Err(refusal) => Settled::Refused(refusal_code(refusal)),
    }
}

impl Plan {
    pub(super) fn is_transfer(&self) -> bool {
        matches!(self.kind, Kind::Transfer(_))
    }

    /// The refusal for a class this wallet does not deploy, in the family's own words.
    fn not_deployable(&self) -> Code {
        if self.account.is_braavos() {
            Code::BraavosNotDeployable
        } else {
            Code::AccountNotDeployable
        }
    }

    /// `Confirm`: refuse a poisoned recipient and a moved session before anything else.
    pub(crate) fn confirm(&mut self, host: &dyn PlanHost) -> Result<SendStep, SendError> {
        if let Kind::Transfer(transfer) = &self.kind {
            if transfer.flags.contains(&ReviewFlag::AddressPoisoning) {
                return Ok(self.fail(Code::PoisonedRecipient, None));
            }
        }
        if !host.session_current() {
            return Ok(self.fail(Code::Locked, None));
        }
        if !self.can_confirm() {
            return Err(SendError::OutOfOrder);
        }
        self.work.reviewed_fee = self.work.fee;
        self.work.reviewed_deploy_fee = self.work.deploy_fee;
        self.work.attempts = 0;
        Ok(
            self.rpc(Expect::SubmitChain, RpcPurpose::ChainId, false, |_, id| {
                crate::rpc::chain_id_request(id)
            }),
        )
    }

    /// A failure after an accepted automatic deployment names the deployment
    /// ("Account deployment … completed, but the following transaction could not be confirmed").
    fn after_deployment(&mut self, code: Code, hash: Option<Felt>) -> SendStep {
        if self.work.deployment_accepted {
            let deploy = self.work.deploy_hash;
            return self.fail(Code::DeploymentThenInvokeUnconfirmed, deploy);
        }
        self.fail(code, hash)
    }

    fn read_nonce(&mut self, expect: Expect) -> SendStep {
        self.rpc(expect, RpcPurpose::Nonce, false, |s, id| {
            rpc_bodies::nonce(id, &s.account.address)
        })
    }

    pub(crate) fn on_submit_reply(
        &mut self,
        host: &dyn PlanHost,
        expect: Expect,
        answer: Option<Answer>,
    ) -> SendStep {
        match expect {
            Expect::SubmitChain => match judge::chain(answer, self.account.network) {
                ChainAnswer::Match => self.probe(Probe::Submit),
                ChainAnswer::Mismatch => self.fail(Code::WrongChain, None),
                ChainAnswer::Unreadable => self.fail(Code::PreparationUnavailable, None),
            },
            Expect::SubmitNonce => match judge::nonce(answer) {
                Some(nonce) => {
                    self.work.nonce = nonce;
                    if self.is_transfer() {
                        let deployed = self.work.submit_deployed;
                        self.rpc(
                            Expect::SubmitEstimate,
                            RpcPurpose::EstimateFee,
                            false,
                            |s, id| s.invoke_estimate_body(id, deployed),
                        )
                    } else {
                        self.estimate_deployment(host, Expect::DeployEstimate)
                    }
                }
                None => self.fail(Code::PreparationUnavailable, None),
            },
            Expect::SubmitEstimate => self.on_invoke_estimate(answer),
            Expect::DeployNonce => match judge::nonce(answer) {
                Some(nonce) => {
                    self.work.nonce = nonce;
                    self.estimate_deployment(host, Expect::DeployEstimate)
                }
                None => self.fail(Code::DeploymentStatusUnknown, None),
            },
            Expect::DeployEstimate => self.on_deploy_estimate(host, answer),
            Expect::DeployWrite => self.on_deploy_write(answer),
            Expect::DeployStatus => self.on_deploy_status(host, answer),
            Expect::DeployReceipt => {
                if judge::receipt(answer) {
                    self.work.deployment_accepted = true;
                    self.work.attempts = 0;
                    self.read_nonce(Expect::InvokeNonce)
                } else {
                    self.retry_deploy_wait(AfterWait::DeployReceipt)
                }
            }
            Expect::InvokeNonce => match judge::nonce(answer) {
                Some(nonce) => self.sign_and_send_invoke(host, nonce),
                None => self.after_deployment(Code::PreparationUnavailable, None),
            },
            Expect::InvokeWrite => self.on_invoke_write(answer),
            Expect::ConfirmStatus | Expect::RefreshStatus => {
                self.on_confirm_status(host, expect, answer)
            }
            _ => self.fail(Code::NodeError, None),
        }
    }

    /// The confirmation's account-state read concluded (`submitInvokeTransaction` /
    /// `submitAccountDeployment`).
    pub(crate) fn submit_probed(&mut self, outcome: Outcome) -> SendStep {
        match outcome {
            Outcome::Deployed if !self.is_transfer() => self.fail(Code::AlreadyDeployed, None),
            // `refuseUndeployedBraavos`: a send never deploys a Braavos account.
            Outcome::Undeployed
                if self.is_transfer()
                    && self.account.is_braavos()
                    && self.account.deployment.is_some() =>
            {
                self.fail(Code::BraavosDeployFirst, None)
            }
            // `accountDeployment` refuses a class this wallet does not deploy before any estimate.
            Outcome::Undeployed if self.account.deployment.is_none() => {
                let code = self.not_deployable();
                self.fail(code, None)
            }
            Outcome::Deployed | Outcome::Undeployed => {
                self.work.submit_deployed = matches!(outcome, Outcome::Deployed);
                self.read_nonce(Expect::SubmitNonce)
            }
            Outcome::Refused(failure) => self.refuse(failure),
            Outcome::NodeError => self.fail(Code::NodeError, None),
            Outcome::Unreadable => self.fail(Code::PreparationUnavailable, None),
        }
    }

    /// `ensureAccountDeployed`'s account-state read concluded.
    pub(crate) fn deploy_probed(&mut self, outcome: Outcome) -> SendStep {
        match outcome {
            Outcome::Deployed => self.read_nonce(Expect::InvokeNonce),
            Outcome::Undeployed if self.work.reviewed_deploy_fee.is_none() => {
                self.fail(Code::ReviewedDeploymentFeeRequired, None)
            }
            Outcome::Undeployed if self.account.deployment.is_none() => {
                let code = self.not_deployable();
                self.fail(code, None)
            }
            Outcome::Undeployed => self.read_nonce(Expect::DeployNonce),
            Outcome::Refused(failure) => self.refuse(failure),
            Outcome::NodeError => self.fail(Code::NodeError, None),
            Outcome::Unreadable => self.fail(Code::PreparationUnavailable, None),
        }
    }

    fn on_invoke_estimate(&mut self, answer: Option<Answer>) -> SendStep {
        let deployed = self.work.submit_deployed;
        let Some(reviewed) = self.work.reviewed_fee else {
            return self.fail(Code::ReviewedFeeRequired, None);
        };
        let entries = match judge::estimate(answer) {
            EstimateAnswer::Entries(entries) => entries,
            EstimateAnswer::NodeError => return self.fail(Code::NodeError, None),
            EstimateAnswer::Unreadable => return self.fail(Code::PreparationUnavailable, None),
        };
        let entry = entries.get(usize::from(!deployed));
        match settle(
            self,
            entry,
            &reviewed,
            Code::ReviewedFeeExceeded,
            Code::NodeError,
        ) {
            Settled::Refused(code) => self.fail(code, None),
            Settled::Bounds(bounds) => {
                self.work.invoke_bounds = Some(bounds);
                if deployed {
                    self.read_nonce(Expect::InvokeNonce)
                } else {
                    self.probe(Probe::Deploy)
                }
            }
        }
    }

    fn sign_and_send_invoke(&mut self, host: &dyn PlanHost, nonce: Felt) -> SendStep {
        let (Some(bounds), Kind::Transfer(transfer)) = (self.work.invoke_bounds, &self.kind) else {
            return self.fail(Code::ReviewedFeeRequired, None);
        };
        let hash = match sign::invoke_hash(
            self.account.network,
            &self.account.address,
            &transfer.calldata,
            &nonce,
            &bounds,
        ) {
            Ok(hash) => hash,
            Err(refusal) => return self.after_deployment(refusal_code(refusal), None),
        };
        let signature = match self.sign(host, &hash) {
            Ok(signature) => signature,
            Err(code) => return self.after_deployment(code, None),
        };
        self.work.invoke_hash = Some(hash);
        self.persist_pending(host, &hash, true);
        self.rpc(Expect::InvokeWrite, RpcPurpose::AddInvoke, true, |s, id| {
            let unsigned = s
                .invoke_tx(&nonce, &signature)
                .expect("transfer plan has an invoke");
            rpc_bodies::add_invoke(
                id,
                &InvokeTx {
                    bounds: Some(&bounds),
                    ..unsigned
                },
            )
        })
    }

    fn on_invoke_write(&mut self, answer: Option<Answer>) -> SendStep {
        let Some(hash) = self.work.invoke_hash else {
            return self.fail(Code::SubmissionIndeterminate, None);
        };
        // Once signed, the invoke may land whatever the node answers, even an error (the
        // extension, which records only after an accepted write, differs here on purpose): its
        // record stays and every failure carries its hash.
        let code = match judge::write(answer, &hash) {
            WriteAnswer::Accepted => return self.broadcast(hash),
            WriteAnswer::Mismatch => Code::HashMismatch,
            WriteAnswer::Indeterminate => Code::SubmissionIndeterminate,
            WriteAnswer::NodeError => Code::NodeError,
        };
        self.after_deployment(code, Some(hash))
    }

    fn on_confirm_status(
        &mut self,
        host: &dyn PlanHost,
        expect: Expect,
        answer: Option<Answer>,
    ) -> SendStep {
        let Some(hash) = self.target_hash() else {
            return self.fail(Code::SubmissionIndeterminate, None);
        };
        let observation = judge::status(answer);
        let recorded = host.observe(&hash, observation);
        let status = recorded.unwrap_or(match observation {
            Observation::Found { status, .. } => status,
            _ => TxStatus::Pending,
        });
        if expect == Expect::RefreshStatus || status != TxStatus::Pending {
            return self.finish(host, status, hash);
        }
        let next = self.work.checkpoint + 1;
        self.checkpoint(host, next)
    }

    /// Has the host sign under the plan's session and returns the account's signature array
    /// (`serializeAccountSignature`). The public key must be the account's.
    pub(super) fn sign(&mut self, host: &dyn PlanHost, hash: &Felt) -> Result<Vec<Felt>, Code> {
        self.sign_renewing(host, hash, true)
    }

    /// `sign`, renewing the idle window only when `renew` (a review's signature does not).
    pub(super) fn sign_renewing(
        &mut self,
        host: &dyn PlanHost,
        hash: &Felt,
        renew: bool,
    ) -> Result<Vec<Felt>, Code> {
        let StarkSignature { public_key, r, s } =
            host.sign(hash, renew).map_err(|refusal| match refusal {
                SignRefusal::Locked => Code::Locked,
                SignRefusal::Failed => Code::SigningFailed,
            })?;
        match (felt::parse(&public_key), felt::parse(&r), felt::parse(&s)) {
            (Some(public_key), Some(r), Some(s)) if public_key == self.account.public_key => {
                self.signatures += 1;
                Ok(self.account.signature(r, s))
            }
            _ => Err(Code::SigningFailed),
        }
    }

    /// Hands the host the pending record for a write about to leave the core.
    pub(super) fn persist_pending(&self, host: &dyn PlanHost, hash: &Felt, invoke: bool) {
        let (kind, summary) = match (&self.kind, invoke) {
            (Kind::Transfer(t), true) => (
                PendingKind::Transfer {
                    amount: t.amount_text.clone(),
                    recipient: t.recipient,
                    symbol: t.symbol.clone(),
                    token_address: t.token_address,
                },
                t.summary.clone(),
            ),
            _ => (PendingKind::Deployment, DEPLOY_SUMMARY.to_string()),
        };
        host.record_pending(PendingRecord {
            hash: *hash,
            kind,
            summary,
            submitted_at: self.work.now_ms,
        });
    }
}
