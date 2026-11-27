//! The automatic (and standalone) account deployment inside a plan, as the extension's
//! `ensureAccountDeployed` / `submitAccountDeployment`: re-estimate against the separately
//! reviewed deployment fee, sign `compute_deploy_account_v3_hash` over the account's own
//! deployment (OpenZeppelin, Ready, or Braavos with its extended authorization, through the host
//! or on the account's device), persist the pending record, broadcast once, then wait for
//! ACCEPTED_ON_L2/L1 and the receipt like starknet.js `waitForTransaction` (bounded at 200
//! attempts).

use crate::Felt;

use super::fees::Bounds;
use super::judge::TxStatus;
use super::judge::{self, Answer, EstimateAnswer, Observation, WriteAnswer};
use super::plan::{
    AfterWait, Authorization, Expect, Plan, PlanHost, DEPLOY_POLL_ATTEMPTS, DEPLOY_POLL_INTERVAL_MS,
};
use super::plan_device::{DeployWrite, SignFor};
use super::plan_submit::{refusal_code, settle, Settled};
use super::rpc_bodies::{self, DeployTx};
use super::sign;
use super::types::{RpcPurpose, SendFailureCode as Code, SendStep};

/// What a deployment estimate is for: a standalone deployment, or the one before an invoke
/// signed at these bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DeploymentFor {
    Standalone,
    Invoke(Bounds),
}

/// A broadcast deployment being waited for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DeploymentPoll {
    pub hash: Felt,
    pub bounds: Bounds,
    pub attempts: u32,
}

impl Plan {
    /// The parameters Braavos's extended deploy authorization carries for this account.
    fn braavos_params(&self) -> Result<Vec<Felt>, Code> {
        let Authorization::Braavos { implementation, .. } = self.account.authorization else {
            return Err(Code::BraavosNotDeployable);
        };
        Ok(sign::braavos_deploy_params(
            self.account.network,
            implementation,
        ))
    }

    /// Braavos's extended deploy authorization (`braavosDeploySignature`): `[r, s, ...params,
    /// aux_r, aux_s]`, where `(aux_r, aux_s)` signs `poseidon(params)` with the same key. The
    /// parameters are signed first, as the extension signs them, so `hash` is the last signed.
    pub(super) fn braavos_deploy_signature(
        &mut self,
        host: &dyn PlanHost,
        hash: &Felt,
        renew: bool,
    ) -> Result<Vec<Felt>, Code> {
        let params = self.braavos_params()?;
        let aux = self.sign_renewing(host, &sign::braavos_params_hash(&params), renew)?;
        let mut signature = self.sign_renewing(host, hash, renew)?;
        signature.extend(params);
        signature.extend(aux);
        Ok(signature)
    }

    /// `braavos_deploy_signature` on the account's device: the parameters' hash, then `hash`,
    /// each its own device signature (`SignFor::BraavosParams`), as the extension's
    /// `braavosDeploySignature` asks its `accountHashSigner` twice.
    fn braavos_on_device(&mut self, host: &dyn PlanHost, hash: Felt, then: SignFor) -> SendStep {
        let params = match self.braavos_params() {
            Ok(params) => params,
            Err(code) => return self.fail(code, None),
        };
        let params_hash = sign::braavos_params_hash(&params);
        let then = SignFor::BraavosParams {
            hash,
            params,
            then: Box::new(then),
        };
        self.sign_for(host, params_hash, then)
    }

    /// Emits the deployment estimate at the read nonce. Ready and a multisig validate their query
    /// signature and OpenZeppelin skips validation; Braavos validates a real signature over the
    /// query-version hash, since its constructor checks the deploy authorization even in an
    /// estimate. A Braavos account on a device signs it there first (`braavos_on_device`).
    pub(crate) fn estimate_deployment(&mut self, host: &dyn PlanHost, expect: Expect) -> SendStep {
        let nonce = self.work.nonce;
        let (signature, skip) = match self.account.deployment.as_ref() {
            Some(deployment) if self.account.is_braavos() => {
                let hash = sign::deploy_query_hash(
                    self.account.network,
                    &self.account.address,
                    deployment,
                    &nonce,
                );
                if self.device.is_some() {
                    return self.braavos_on_device(host, hash, SignFor::DeployEstimate(expect));
                }
                // The review's estimate is passive; the confirmed re-estimate renews.
                let renew = expect != Expect::ReviewDeployEstimate;
                match self.braavos_deploy_signature(host, &hash, renew) {
                    Ok(signature) => (signature, false),
                    Err(code) => return self.fail(code, None),
                }
            }
            _ => (
                self.account.query_signature(),
                !self.account.is_argent() && !self.account.is_multisig(),
            ),
        };
        self.send_deploy_estimate(expect, signature, skip)
    }

    /// Sends the deployment estimate at the read nonce, carrying `signature`.
    pub(super) fn send_deploy_estimate(
        &mut self,
        expect: Expect,
        signature: Vec<Felt>,
        skip: bool,
    ) -> SendStep {
        let nonce = self.work.nonce;
        self.rpc(
            expect,
            RpcPurpose::EstimateDeploymentFee,
            false,
            move |s, id| {
                rpc_bodies::estimate(id, None, s.deploy_tx(&nonce, &signature).as_ref(), skip)
            },
        )
    }

    pub(super) fn on_deploy_estimate(
        &mut self,
        host: &dyn PlanHost,
        purpose: DeploymentFor,
        answer: Option<Answer>,
    ) -> SendStep {
        // Inside `ensureAccountDeployed` a transport failure is "deployment status unknown" and
        // any other error "automatic deployment failed"; the standalone flow reports them as
        // any other write preparation does.
        let transfer = matches!(purpose, DeploymentFor::Invoke(_));
        let (transport, node) = if transfer {
            (Code::DeploymentStatusUnknown, Code::AutoDeploymentFailed)
        } else {
            (Code::PreparationUnavailable, Code::NodeError)
        };
        let Some(reviewed) = self.work.reviewed_deploy_fee else {
            return self.fail(Code::ReviewedDeploymentFeeRequired, None);
        };
        let entries = match judge::estimate(answer) {
            EstimateAnswer::Entries(entries) => entries,
            EstimateAnswer::NodeError => return self.fail(node, None),
            EstimateAnswer::Unreadable => return self.fail(transport, None),
        };
        let bounds = match settle(
            self,
            entries.first(),
            &reviewed,
            Code::ReviewedDeploymentFeeExceeded,
            node,
        ) {
            Settled::Bounds(bounds) => bounds,
            Settled::Refused(Code::PreparationUnavailable) => return self.fail(transport, None),
            Settled::Refused(code) => return self.fail(code, None),
        };
        let account = &self.account;
        let Some(deployment) = account.deployment.clone() else {
            return self.fail(Code::AccountNotDeployable, None);
        };
        let hash = match sign::deploy_hash(account.network, &account.address, &deployment, &bounds)
        {
            Ok(hash) => hash,
            Err(refusal) => return self.fail(refusal_code(refusal), None),
        };
        let write = DeployWrite {
            bounds,
            purpose,
            deployment,
        };
        if !self.account.is_braavos() {
            return self.sign_for(host, hash, SignFor::Deploy(write));
        }
        if self.device.is_some() {
            return self.braavos_on_device(host, hash, SignFor::Deploy(write));
        }
        // Braavos signs twice through the host.
        match self.braavos_deploy_signature(host, &hash, true) {
            Ok(signature) => self.send_deploy(host, hash, write, signature),
            Err(code) => self.fail(code, None),
        }
    }

    /// Sends the deployment `hash` names, once signed: the only way a deployment's write leaves.
    pub(super) fn send_deploy(
        &mut self,
        host: &dyn PlanHost,
        hash: Felt,
        write: DeployWrite,
        signature: Vec<Felt>,
    ) -> SendStep {
        let DeployWrite {
            bounds,
            purpose,
            deployment,
        } = write;
        self.work.deploy_hash = Some(hash);
        self.persist_pending(host, &hash, false);
        self.rpc(
            Expect::DeployWrite { hash, purpose },
            RpcPurpose::AddDeployAccount,
            true,
            |_, id| {
                rpc_bodies::add_deploy(
                    id,
                    &DeployTx {
                        class_hash: &deployment.class_hash,
                        salt: &deployment.salt,
                        constructor_calldata: &deployment.constructor_calldata,
                        nonce: &Felt::ZERO,
                        bounds: Some(&bounds),
                        signature: &signature,
                    },
                )
            },
        )
    }

    pub(super) fn on_deploy_write(
        &mut self,
        hash: Felt,
        purpose: DeploymentFor,
        answer: Option<Answer>,
    ) -> SendStep {
        // The pending record and signed hash survive any write outcome.
        let transfer = matches!(purpose, DeploymentFor::Invoke(_));
        let code = match judge::write(answer, &hash) {
            WriteAnswer::Accepted => {
                return match purpose {
                    DeploymentFor::Standalone => self.broadcast(hash),
                    DeploymentFor::Invoke(bounds) => {
                        let poll = DeploymentPoll {
                            hash,
                            bounds,
                            attempts: 0,
                        };
                        self.wait(DEPLOY_POLL_INTERVAL_MS, AfterWait::DeployStatus(poll))
                    }
                }
            }
            WriteAnswer::Mismatch | WriteAnswer::Indeterminate if transfer => {
                Code::DeploymentStatusUnknown
            }
            WriteAnswer::NodeError if transfer => Code::AutoDeploymentFailed,
            WriteAnswer::Mismatch => Code::HashMismatch,
            WriteAnswer::Indeterminate => Code::SubmissionIndeterminate,
            WriteAnswer::NodeError => Code::NodeError,
        };
        self.fail(code, Some(hash))
    }

    pub(crate) fn poll_deployment(&mut self, poll: DeploymentPoll) -> SendStep {
        self.rpc(
            Expect::DeployStatus(poll),
            RpcPurpose::TransactionStatus,
            false,
            |_, id| rpc_bodies::transaction_status(id, &poll.hash),
        )
    }

    pub(crate) fn read_deploy_receipt(&mut self, poll: DeploymentPoll) -> SendStep {
        self.rpc(
            Expect::DeployReceipt(poll),
            RpcPurpose::TransactionReceipt,
            false,
            |_, id| rpc_bodies::transaction_receipt(id, &poll.hash),
        )
    }

    /// Status and receipt retries share one deployment budget.
    fn retry_deploy_wait(
        &mut self,
        mut poll: DeploymentPoll,
        next: fn(DeploymentPoll) -> AfterWait,
    ) -> SendStep {
        poll.attempts += 1;
        if poll.attempts >= DEPLOY_POLL_ATTEMPTS {
            return self.fail(Code::DeploymentUnconfirmed, Some(poll.hash));
        }
        self.wait(DEPLOY_POLL_INTERVAL_MS, next(poll))
    }

    pub(super) fn on_deploy_status(
        &mut self,
        host: &dyn PlanHost,
        poll: DeploymentPoll,
        answer: Option<Answer>,
    ) -> SendStep {
        let observation = judge::status(answer);
        host.observe(&poll.hash, observation);
        match observation {
            Observation::Found {
                accepted_finality: true,
                ..
            } => self.read_deploy_receipt(poll),
            Observation::Found {
                status: TxStatus::Rejected,
                ..
            } => self.fail(Code::DeploymentUnconfirmed, Some(poll.hash)),
            _ => self.retry_deploy_wait(poll, AfterWait::DeployStatus),
        }
    }

    pub(super) fn on_deploy_receipt(
        &mut self,
        host: &dyn PlanHost,
        poll: DeploymentPoll,
        answer: Option<Answer>,
    ) -> SendStep {
        // The deployment's receipt is read here anyway, so its record takes its fee.
        if let Some(fee) = judge::receipt_fee(answer.as_ref()) {
            if host.wants_fee(&poll.hash) {
                host.record_fee(&poll.hash, &fee);
            }
        }
        if judge::receipt(answer) {
            self.read_nonce(Expect::InvokeNonce {
                bounds: poll.bounds,
                deployment: Some(poll.hash),
            })
        } else {
            self.retry_deploy_wait(poll, AfterWait::DeployReceipt)
        }
    }
}
