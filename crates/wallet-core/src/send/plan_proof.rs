//! A proof-bearing batch's proving-block wait inside a plan (`submitPrepared`'s
//! `maturity.provingBlock`), between `Confirm` and the submit's chain read. The wait itself is
//! `proof_invoke::ProvingWait`; the plan only relays its reads and pauses.

use super::judge::Answer;
use super::plan::{AfterWait, Expect, Kind, Plan};
use super::proof_invoke::{Proof, ProvingWait, WaitRead, WaitStep};
use super::types::{RpcPurpose, SendFailureCode as Code, SendStep};

impl Plan {
    /// The proof the plan's batch carries, if it is proof-bearing.
    pub(crate) fn invoke_proof(&self) -> Option<&Proof> {
        match &self.kind {
            Kind::Invoke(invoke) => invoke.proof().map(|submission| &submission.proof),
            _ => None,
        }
    }

    /// The prior write's accepted block once the wait read it: what the wallet's maturity store
    /// records for that write. `None` before, or when the store already had it.
    pub fn prior_accepted_block(&self) -> Option<u64> {
        self.work.proving?.prior_accepted_block()
    }

    /// Starts the wait for a proof-bearing batch; `None` for any other plan.
    pub(super) fn start_proving(&mut self) -> Option<SendStep> {
        let Kind::Invoke(invoke) = &self.kind else {
            return None;
        };
        let submission = invoke.proof()?;
        let mut wait = ProvingWait::new(submission.maturity, submission.prior);
        let step = wait.start();
        Some(self.proving(wait, step))
    }

    pub(super) fn on_proving(&mut self, answer: Option<Answer>) -> SendStep {
        let Some(mut wait) = self.work.proving else {
            return self.fail(Code::PreparationUnavailable, None);
        };
        let step = wait.on_reply(answer);
        self.proving(wait, step)
    }

    pub(super) fn after_proving_pause(&mut self) -> SendStep {
        let Some(mut wait) = self.work.proving else {
            return self.fail(Code::PreparationUnavailable, None);
        };
        let step = wait.after_pause();
        self.proving(wait, step)
    }

    fn proving(&mut self, wait: ProvingWait, step: WaitStep) -> SendStep {
        self.work.proving = Some(wait);
        match step {
            WaitStep::Read(read) => {
                let purpose = match read {
                    WaitRead::Status(_) => RpcPurpose::TransactionStatus,
                    WaitRead::Receipt(_) => RpcPurpose::TransactionReceipt,
                    WaitRead::BlockNumber => RpcPurpose::AcceptedBlock,
                };
                self.rpc(Expect::Proving, purpose, false, |_, id| read.body(id))
            }
            WaitStep::Pause(delay_ms) => self.wait(delay_ms, AfterWait::Proving),
            WaitStep::Ready(_) => self.prove_chain(),
            // "strk20 services unavailable", before anything was signed.
            WaitStep::Failed => self.fail(Code::PreparationUnavailable, None),
        }
    }
}
