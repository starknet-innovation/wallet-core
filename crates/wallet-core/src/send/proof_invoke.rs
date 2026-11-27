//! A proof-bearing INVOKE (SNIP-36): a STRK20 `apply_actions` call submitted with its proof. The
//! extension's `submitPrepared` (`official-sdk-service.ts`) and the proving-block wait it runs
//! first (`official-sdk-maturity.ts` `provingBlock`), as plain rules: the shape a batch must have
//! before it may carry a proof (`check_shape`), the maturity rule (`proving_block`), and the wait
//! over them as a sans-IO piece (`ProvingWait`). A plan carries the proof through
//! `Invoke::with_proof`; the hash appends `poseidon(proof_facts)` (`sign::invoke_hash_with_proof`)
//! and the estimate and the write send `proof` and `proof_facts` (`rpc_bodies::InvokeTx`).
//!
//! The wallet keeps the maturity store (the extension's per account, network and pool scope) and
//! hands the plan the scope's prior write (`PriorWrite`) when it builds it; a scope whose last
//! write is still `submitting` (signed, its answer lost) is the wallet's to resolve or refuse
//! before any plan exists. The wallet runs one privacy write per scope at a time, as the
//! extension serialises them.

use serde_json::Value;

use super::call_decode::APPROVE_SET;
use super::calls::Call;
use super::judge::{self, Answer, Observation, TxStatus};
use super::rpc_bodies;
use crate::Felt;

/// The pool's proof-bearing entrypoint.
pub const APPLY_ACTIONS: &str = "apply_actions";

/// A proof as `STRK20_PROOF`: opaque data plus felt outputs and facts. Nothing here is secret:
/// `data` and `proof_facts` go on chain with the transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proof {
    pub data: String,
    pub output: Vec<Felt>,
    pub proof_facts: Vec<Felt>,
}

/// `submitPrepared`'s one refusal, before anything is read or signed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("strk20 private submission unavailable")]
pub struct SubmissionUnavailable;

/// `submitPrepared`'s shape rule: a proof with data, output and facts, and a batch ending in the
/// one `apply_actions` on `pool`, every earlier call an `approve`.
pub fn check_shape(
    pool: &Felt,
    calls: &[Call],
    proof: &Proof,
) -> Result<(), SubmissionUnavailable> {
    let Some((last, earlier)) = calls.split_last() else {
        return Err(SubmissionUnavailable);
    };
    let proved =
        !proof.data.is_empty() && !proof.output.is_empty() && !proof.proof_facts.is_empty();
    let applies = last.contract_address == *pool && last.entrypoint == APPLY_ACTIONS;
    if !proved || !applies || earlier.iter().any(|call| call.entrypoint != APPROVE_SET) {
        return Err(SubmissionUnavailable);
    }
    Ok(())
}

/// The scope's last privacy write, as the wallet's maturity store holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriorWrite {
    /// Accepted in this block (or released at it): the floor a proof must be past.
    Accepted(u64),
    /// Broadcast, its block not yet known: its status and receipt are read first.
    Submitted(Felt),
}

/// The wallet's numbers for the proving-block wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Maturity {
    /// How far below the tip a proof targets state.
    pub depth: u64,
    /// Pauses before the wait gives up: `max_polls + 1` block reads in all.
    pub max_polls: u32,
    pub poll_pause_ms: u32,
    /// Status reads of a prior write before the wait gives up, and the pause between them.
    pub status_reads: u32,
    pub status_pause_ms: u32,
}

/// The maturity rule (`matureCandidate`): `latest - depth`, clamped at 0, once it is strictly
/// past the prior write's block.
pub fn proving_block(latest: u64, prior: Option<u64>, depth: u64) -> Option<u64> {
    let candidate = latest.saturating_sub(depth);
    prior
        .is_none_or(|block| block < candidate)
        .then_some(candidate)
}

/// What a plan carries beside a proof-bearing batch: the proof, the scope's prior write, and the
/// wallet's numbers for the wait.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProofSubmission {
    pub proof: Proof,
    pub prior: Option<PriorWrite>,
    pub maturity: Maturity,
}

/// A read the wait asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitRead {
    Status(Felt),
    Receipt(Felt),
    BlockNumber,
}

impl WaitRead {
    /// Its JSON-RPC body under `id`.
    pub fn body(self, id: u64) -> String {
        match self {
            Self::Status(hash) => rpc_bodies::transaction_status(id, &hash),
            Self::Receipt(hash) => rpc_bodies::transaction_receipt(id, &hash),
            Self::BlockNumber => rpc_bodies::block_number(id),
        }
    }
}

/// The next thing the wait needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitStep {
    Read(WaitRead),
    Pause(u32),
    /// The proving block: the chain is past the prior write.
    Ready(u64),
    /// "strk20 services unavailable": the prior write failed or could not be read, a read was
    /// unusable, or the chain did not move far enough within the polls.
    Failed,
}

/// `provingBlock`: read the prior write's accepted block when the store lacks it
/// (`waitForTransaction`), then poll the tip until `proving_block` answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProvingWait {
    maturity: Maturity,
    prior: Option<PriorWrite>,
    reading: WaitRead,
    reads: u32,
}

impl ProvingWait {
    pub fn new(maturity: Maturity, prior: Option<PriorWrite>) -> Self {
        let reading = match prior {
            Some(PriorWrite::Submitted(hash)) => WaitRead::Status(hash),
            _ => WaitRead::BlockNumber,
        };
        Self {
            maturity,
            prior,
            reading,
            reads: 0,
        }
    }

    /// The first read.
    pub fn start(&mut self) -> WaitStep {
        self.read(self.reading)
    }

    /// The prior write's accepted block once known: what the wallet's store records for it.
    pub fn prior_accepted_block(&self) -> Option<u64> {
        match self.prior {
            Some(PriorWrite::Accepted(block)) => Some(block),
            _ => None,
        }
    }

    fn read(&mut self, read: WaitRead) -> WaitStep {
        if read != self.reading {
            self.reads = 0;
        }
        self.reading = read;
        self.reads += 1;
        WaitStep::Read(read)
    }

    /// The answer to the last read (`None`: no usable reply).
    pub fn on_reply(&mut self, answer: Option<Answer>) -> WaitStep {
        match self.reading {
            WaitRead::Status(hash) => match judge::status(answer) {
                Observation::Found {
                    status: TxStatus::Accepted,
                    accepted_finality: true,
                    ..
                } => self.read(WaitRead::Receipt(hash)),
                Observation::Found {
                    status: TxStatus::Rejected | TxStatus::Reverted,
                    ..
                } => WaitStep::Failed,
                _ if self.reads >= self.maturity.status_reads => WaitStep::Failed,
                _ => WaitStep::Pause(self.maturity.status_pause_ms),
            },
            WaitRead::Receipt(_) => match accepted_block(answer) {
                Some(block) => {
                    self.prior = Some(PriorWrite::Accepted(block));
                    self.read(WaitRead::BlockNumber)
                }
                None => WaitStep::Failed,
            },
            WaitRead::BlockNumber => {
                let Some(latest) = block_number(answer) else {
                    return WaitStep::Failed;
                };
                let prior = self.prior_accepted_block();
                match proving_block(latest, prior, self.maturity.depth) {
                    Some(block) => WaitStep::Ready(block),
                    None if self.reads > self.maturity.max_polls => WaitStep::Failed,
                    None => WaitStep::Pause(self.maturity.poll_pause_ms),
                }
            }
        }
    }

    /// The pause asked for has elapsed: read again.
    pub fn after_pause(&mut self) -> WaitStep {
        self.read(self.reading)
    }
}

/// `starknet_blockNumber`'s answer: a JSON integer.
fn block_number(answer: Option<Answer>) -> Option<u64> {
    match answer? {
        Answer::Result(value) => value.as_u64(),
        Answer::Error { .. } => None,
    }
}

/// A succeeded receipt's block (`waitForTransaction`, then `isSuccess` and `block_number`).
fn accepted_block(answer: Option<Answer>) -> Option<u64> {
    let Some(Answer::Result(receipt)) = answer else {
        return None;
    };
    let succeeded = receipt.get("execution_status").and_then(Value::as_str) == Some("SUCCEEDED");
    succeeded
        .then(|| receipt.get("block_number").and_then(Value::as_u64))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(result: Value) -> Option<Answer> {
        Some(Answer::Result(result))
    }

    const MATURITY: Maturity = Maturity {
        depth: 10,
        max_polls: 2,
        poll_pause_ms: 1_000,
        status_reads: 2,
        status_pause_ms: 250,
    };

    #[test]
    fn the_candidate_must_be_strictly_past_the_prior_write() {
        assert_eq!(proving_block(5, None, 10), Some(0));
        assert_eq!(proving_block(1_000_013, Some(1_000_003), 10), None);
        assert_eq!(
            proving_block(1_000_014, Some(1_000_003), 10),
            Some(1_000_004)
        );
        // A prior write at block 0 still holds a young chain back.
        assert_eq!(proving_block(5, Some(0), 10), None);
    }

    #[test]
    fn the_wait_gives_up_after_its_polls() {
        let mut wait = ProvingWait::new(MATURITY, Some(PriorWrite::Accepted(100)));
        assert_eq!(wait.start(), WaitStep::Read(WaitRead::BlockNumber));
        for _ in 0..MATURITY.max_polls {
            assert_eq!(wait.on_reply(ok(Value::from(100))), WaitStep::Pause(1_000));
            assert_eq!(wait.after_pause(), WaitStep::Read(WaitRead::BlockNumber));
        }
        assert_eq!(wait.on_reply(ok(Value::from(100))), WaitStep::Failed);
    }

    #[test]
    fn a_prior_write_that_did_not_succeed_fails_the_wait() {
        let hash = Felt::from(7u8);
        for (status, receipt) in [
            (
                r#"{"finality_status":"ACCEPTED_ON_L2","execution_status":"REVERTED"}"#,
                None,
            ),
            (r#"{"finality_status":"REJECTED"}"#, None),
            (
                r#"{"finality_status":"ACCEPTED_ON_L2","execution_status":"SUCCEEDED"}"#,
                Some(r#"{"execution_status":"REVERTED","block_number":5}"#),
            ),
            (
                r#"{"finality_status":"ACCEPTED_ON_L2","execution_status":"SUCCEEDED"}"#,
                Some(r#"{"execution_status":"SUCCEEDED","block_number":"0x5"}"#),
            ),
        ] {
            let mut wait = ProvingWait::new(MATURITY, Some(PriorWrite::Submitted(hash)));
            assert_eq!(wait.start(), WaitStep::Read(WaitRead::Status(hash)));
            let step = wait.on_reply(ok(serde_json::from_str(status).unwrap()));
            let Some(receipt) = receipt else {
                assert_eq!(step, WaitStep::Failed, "{status}");
                continue;
            };
            assert_eq!(step, WaitStep::Read(WaitRead::Receipt(hash)));
            let step = wait.on_reply(ok(serde_json::from_str(receipt).unwrap()));
            assert_eq!(step, WaitStep::Failed, "{receipt}");
            assert_eq!(wait.prior_accepted_block(), None);
        }
    }

    #[test]
    fn a_pending_prior_write_is_read_again_within_its_budget() {
        let hash = Felt::from(7u8);
        let mut wait = ProvingWait::new(MATURITY, Some(PriorWrite::Submitted(hash)));
        wait.start();
        let pending = || ok(serde_json::json!({"finality_status":"RECEIVED"}));
        assert_eq!(wait.on_reply(pending()), WaitStep::Pause(250));
        assert_eq!(wait.after_pause(), WaitStep::Read(WaitRead::Status(hash)));
        assert_eq!(wait.on_reply(pending()), WaitStep::Failed);
    }

    #[test]
    fn an_unreadable_block_number_fails() {
        for answer in [None, ok(Value::from("0x10")), ok(Value::from(-1))] {
            let mut wait = ProvingWait::new(MATURITY, None);
            wait.start();
            assert_eq!(wait.on_reply(answer), WaitStep::Failed);
        }
    }

    #[test]
    fn the_shape_rule_holds_the_last_call_to_the_pool() {
        let pool = Felt::from(0x9001u32);
        let call = |contract: u32, entrypoint: &str| Call {
            contract_address: Felt::from(contract),
            entrypoint: entrypoint.into(),
            calldata: Vec::new(),
        };
        let proof = Proof {
            data: "AA==".into(),
            output: vec![Felt::ONE],
            proof_facts: vec![Felt::TWO],
        };
        let apply = call(0x9001, APPLY_ACTIONS);
        assert!(check_shape(&pool, &[call(5, "approve"), apply.clone()], &proof).is_ok());
        // Only `approve` itself, not the allowance increase `decodeCall` also reads as one.
        assert!(check_shape(
            &pool,
            &[call(5, "increaseAllowance"), apply.clone()],
            &proof
        )
        .is_err());
        assert!(check_shape(&pool, &[call(0x9002, APPLY_ACTIONS)], &proof).is_err());
        assert_eq!(
            SubmissionUnavailable.to_string(),
            "strk20 private submission unavailable"
        );
    }
}
