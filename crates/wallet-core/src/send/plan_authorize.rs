//! `requireLiveAuthorizableClass` as a plan that never signs (`Kind::Authorize`): before the
//! wallet signs something an account contract will verify (a dapp's SNIP-12 typed data), the
//! account's live state must accept the signature this wallet spells. The reads and verdicts are
//! the send review's own (`plan_probe`): the endpoint's chain, then for Ready the accepted block,
//! the class at it (Argent 0.4's signer array), the owner, its type and no guardian; for Braavos
//! the block, a signing implementation and this wallet's key as first Stark signer with no 2FA
//! signer still active. An undeployed account authorizes only through the class its deployment
//! would install (a Ready account this wallet deploys); an undeployed Braavos never does.
//!
//! OpenZeppelin needs none of it: its signature does not depend on the class.

use super::judge::{Observation, TxStatus};
use super::plan::{PendingRecord, Plan, PlanHost};
use super::plan_probe::{authorizable, Outcome};
use super::types::{
    Authorization, Kind, SendFailure, SendFailureCode as Code, SendStep, SignRefusal, StepReply,
};
use crate::{Felt, StarkSignature};

/// One step of an authorization read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorizeStep {
    /// POST this step's body to an endpoint for the account's network and reply to it with
    /// [`Plan::authorize`] (`StepReply::Rpc`, or `TransportFailure`).
    Read(SendStep),
    /// The live account accepts this wallet's signature.
    Authorized,
    /// It does not, or it could not be read: never sign.
    Refused(SendFailure),
}

/// A host for a plan that cannot sign or record anything.
struct NoHost;

impl PlanHost for NoHost {
    fn session_current(&self) -> bool {
        false
    }
    fn sign(&self, _hash: &Felt, _renew: bool) -> Result<StarkSignature, SignRefusal> {
        Err(SignRefusal::Failed)
    }
    fn record_pending(&self, _record: PendingRecord) {}
    fn observe(&self, _hash: &Felt, _observation: Observation) -> Option<TxStatus> {
        None
    }
    fn explorer_url(&self, _hash: &str) -> Option<String> {
        None
    }
}

fn refusal(code: Code) -> AuthorizeStep {
    AuthorizeStep::Refused(SendFailure {
        code,
        hash: None,
        detail: None,
    })
}

impl Plan {
    /// Runs a `Kind::Authorize` plan: `StepReply::Start`, then each read's reply. Any other kind
    /// is refused, and nothing here signs.
    pub fn authorize(&mut self, reply: StepReply) -> AuthorizeStep {
        if !matches!(self.kind, Kind::Authorize) {
            self.close();
            return refusal(Code::PreparationUnavailable);
        }
        match self.advance(&NoHost, reply) {
            Ok(step @ SendStep::Rpc { .. }) => AuthorizeStep::Read(step),
            Ok(_) if self.work.authorized => AuthorizeStep::Authorized,
            Ok(SendStep::Failed { failure }) => AuthorizeStep::Refused(failure),
            _ => {
                self.close();
                refusal(Code::NodeError)
            }
        }
    }

    /// The account checks concluded. On success the plan closes with `work.authorized` set; the
    /// step it returns is never handed out (`authorize` reports `Authorized`).
    pub(crate) fn authorize_probed(&mut self, outcome: Outcome) -> SendStep {
        let undeployed = || match (&self.account.authorization, &self.account.deployment) {
            (Authorization::ArgentSignerArray { classes }, Some(deployment)) => {
                authorizable(classes, &deployment.class_hash)
                    .map_err(|_| Code::AccountClassUnsupported)
            }
            (Authorization::Braavos { .. }, _) => Err(Code::BraavosDeployFirst),
            _ => Err(Code::AccountClassUnsupported),
        };
        let verdict = match outcome {
            Outcome::Deployed => Ok(()),
            Outcome::Undeployed => undeployed(),
            Outcome::Refused(failure) => return self.refuse(failure),
            Outcome::NodeError | Outcome::Unreadable => Err(Code::NodeError),
        };
        match verdict {
            Ok(()) => {
                self.work.authorized = true;
                self.close();
                SendStep::Review
            }
            Err(code) => self.fail(code, None),
        }
    }
}

#[cfg(test)]
#[path = "plan_authorize_tests.rs"]
mod tests;
