//! The extension's `prepareArgentMultisigInvokeV3` and `submitPreparedArgentMultisigInvokeV3` as
//! two sans-IO state machines. The core builds every body (`rpc_bodies`) and judges every reply;
//! the host moves bytes to one endpoint. Neither plan signs or holds a key: a proposal freezes
//! the transaction every co-signer then signs over its hash, and a submission broadcasts the
//! threshold signature the co-signers collected (`multisig_intent`).
//!
//! The wire order is the extension's (`extension-multisig-transactions.json`): prepare reads
//! `starknet_chainId`, `starknet_getNonce` and one validated `starknet_estimateFee` carrying the
//! Argent query signature; submit proves the chain, then sends `starknet_addInvokeTransaction`
//! exactly once. The extension fails a read over to its next endpoint; here a transport failure
//! ends the plan and the host may start another against a different endpoint.

use starknet_types_core::felt::Felt;

use super::fees::{quote_with, FeeRefusal, Overhead};
use super::judge::{self, Answer, ChainAnswer, EstimateAnswer, WriteAnswer};
use super::multisig_intent::{IntentState, SigningIntent};
use super::multisig_invoke::{
    execute_calldata, input_felt, normalize_calls, query_signature, signature_felts, MultisigCall,
    MultisigInvokeError, PreparedInvoke, StarkPrimitives, MAX_EXECUTE_CALLDATA_FELTS,
};
use super::multisig_review::require_safe_calls;
use super::rpc_bodies::{self, InvokeTx};
use super::types::{RpcPurpose, SendError, StepReply};
use crate::accounts::multisig::{ArgentMultisigClass, ArgentMultisigConfig};
use crate::accounts::multisig_records::Record;
use crate::kms::NetworkId;
use crate::{felt, rpc};

/// What the host must do next for a multisig plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MultisigStep {
    /// POST `body` and reply `StepReply::Rpc` (same `id`) or `StepReply::TransportFailure`. A
    /// write is sent exactly once and never redirected.
    Rpc {
        id: u64,
        body: String,
        purpose: RpcPurpose,
        is_write: bool,
    },
    /// Terminal: the frozen transaction a proposal asks the co-signers to sign.
    Prepared(Box<PreparedInvoke>),
    /// Terminal: the node accepted exactly the frozen hash.
    Submitted { hash: String },
    /// Terminal. The plan is closed.
    Failed(MultisigFailure),
}

/// Why a multisig plan ended. Fixed variants: no node text reaches them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultisigFailure {
    /// The endpoint is on another chain than the account.
    WrongChain,
    /// A read failed in transport or answered with something unreadable. Nothing was sent.
    PreparationUnavailable,
    /// A determinate JSON-RPC error. Nothing was accepted.
    NodeError,
    /// The estimate with its overhead does not fit the bounds a v3 transaction can sign.
    BoundsOutOfRange,
    /// The session ended before the write (the extension's `isAuthorized`).
    Locked,
    /// The node refused the frozen fee ceiling (`ArgentMultisigFeeExpiredError`): not submitted.
    FeeExpired,
    /// The node answered with a different hash: it may have accepted something; check the chain.
    HashMismatch,
    /// The write's outcome is unknown: check the chain before retrying.
    SubmissionIndeterminate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    Chain,
    Nonce,
    Estimate,
    Write,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Fresh,
    Awaiting { id: u64, expect: Expect },
    Closed,
}

/// Matches the reply against the outstanding request; the judged answer (`None` for a
/// transport failure or an unparseable body) and what it was for.
fn take_reply(
    stage: &mut Stage,
    reply: StepReply,
) -> Result<Option<(Expect, Option<Answer>)>, SendError> {
    let current = std::mem::replace(stage, Stage::Closed);
    match (current, reply) {
        (Stage::Closed, _) => Err(SendError::PlanClosed),
        (Stage::Fresh, StepReply::Start) => Ok(None),
        (Stage::Awaiting { id, expect }, StepReply::Rpc { id: got, body, .. }) if id == got => {
            Ok(Some((expect, judge::answer(&body, id))))
        }
        (Stage::Awaiting { id, expect }, StepReply::TransportFailure { id: got, .. })
            if id == got =>
        {
            Ok(Some((expect, None)))
        }
        _ => Err(SendError::OutOfOrder),
    }
}

fn request(
    stage: &mut Stage,
    id: u64,
    expect: Expect,
    body: String,
    purpose: RpcPurpose,
) -> MultisigStep {
    *stage = Stage::Awaiting { id, expect };
    MultisigStep::Rpc {
        id,
        body,
        purpose,
        is_write: expect == Expect::Write,
    }
}

/// A felt result in any spelling `canonicalArgentMultisigFelt` takes.
fn felt_result(answer: Option<Answer>) -> Result<Felt, MultisigFailure> {
    match answer {
        Some(Answer::Result(serde_json::Value::String(text))) => {
            input_felt(&text).ok_or(MultisigFailure::PreparationUnavailable)
        }
        Some(Answer::Error { .. }) => Err(MultisigFailure::NodeError),
        _ => Err(MultisigFailure::PreparationUnavailable),
    }
}

/// `judge::chain` in the multisig grammar; a determinate JSON-RPC error is `NodeError`, as for
/// the nonce read.
fn chain_matches(answer: Option<Answer>, network: NetworkId) -> Result<(), MultisigFailure> {
    if matches!(answer, Some(Answer::Error { .. })) {
        return Err(MultisigFailure::NodeError);
    }
    match judge::chain_spelled(answer, network, input_felt) {
        ChainAnswer::Match => Ok(()),
        ChainAnswer::Mismatch => Err(MultisigFailure::WrongChain),
        ChainAnswer::Unreadable => Err(MultisigFailure::PreparationUnavailable),
    }
}

/// `isArgentMultisigFeeExpiredMessage`, over the judged error text (already lower-cased).
fn fee_expired(text: &str) -> bool {
    let normalized: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    normalized.contains("lowerthantheactualgasprice")
        && ["maxl1gasprice", "maxl1datagasprice", "maxl2gasprice"]
            .iter()
            .any(|label| normalized.contains(label))
}

/// `prepareArgentMultisigInvokeV3`: proposes `calls` from the multisig, freezing the nonce and
/// the bounds an estimate validated with the Argent query signature plus `overhead`.
pub struct ProposePlan {
    network: NetworkId,
    sender: Felt,
    calls: Vec<MultisigCall>,
    calldata: Vec<Felt>,
    query_signature: Vec<Felt>,
    overhead: Overhead,
    nonce: Felt,
    next_id: u64,
    stage: Stage,
}

impl ProposePlan {
    /// Checks the account is `class` on a known network, normalises `config` and `calls`, holds
    /// the calls to `multisig_review::require_safe_calls` (no self-call in a batch, no upgrade
    /// callback, a self-call only as a valid config change), and compiles the execute calldata
    /// before anything is read. The wallet's own `review_calls` says why; this only refuses, so a
    /// host that skips the review cannot freeze an unsafe batch. `overhead` is the wallet's fee
    /// margin for a transaction that may wait days for co-signers (Bramble's is
    /// `MULTISIG_FEE_OVERHEAD`).
    pub fn new(
        record: &Record,
        calls: &[MultisigCall],
        config: &ArgentMultisigConfig,
        class: &ArgentMultisigClass,
        overhead: Overhead,
        primitives: &dyn StarkPrimitives,
    ) -> Result<Self, MultisigInvokeError> {
        let network = NetworkId::from_catalog_id(&record.network_id)
            .filter(|_| record.account_contract_type == "argent-multisig")
            .filter(|_| input_felt(&record.class_hash) == Some(class.hash()))
            .ok_or(MultisigInvokeError::NotPinnedMultisig)?;
        let sender = input_felt(&record.address)
            .filter(|address| *address != Felt::ZERO)
            .ok_or(MultisigInvokeError::InvalidAddress)?;
        let query_signature = query_signature(config, class)?;
        let calls = normalize_calls(calls)?;
        require_safe_calls(&record.address, &calls, class)
            .map_err(|_| MultisigInvokeError::UnsafeCalls)?;
        let calldata =
            execute_calldata(&calls, primitives).ok_or(MultisigInvokeError::InvalidCalls)?;
        if calldata.len() > MAX_EXECUTE_CALLDATA_FELTS {
            return Err(MultisigInvokeError::CalldataLength);
        }
        Ok(Self {
            network,
            sender,
            calls,
            calldata,
            query_signature,
            overhead,
            nonce: Felt::ZERO,
            next_id: 0,
            stage: Stage::Fresh,
        })
    }

    /// Feeds the last reply (`Start` first) and returns the next step. A reply that does not
    /// answer the outstanding request is `OutOfOrder`; it and every terminal step close the plan.
    pub fn advance(&mut self, reply: StepReply) -> Result<MultisigStep, SendError> {
        let step = match take_reply(&mut self.stage, reply)? {
            None => Ok(self.ask(Expect::Chain)),
            Some((expect, answer)) => self.on_reply(expect, answer),
        };
        Ok(step.unwrap_or_else(MultisigStep::Failed))
    }

    fn ask(&mut self, expect: Expect) -> MultisigStep {
        self.next_id += 1;
        let id = self.next_id;
        let (body, purpose) = match expect {
            Expect::Chain => (rpc::chain_id_request(id), RpcPurpose::ChainId),
            Expect::Nonce => (rpc_bodies::nonce(id, &self.sender), RpcPurpose::Nonce),
            _ => {
                let query = InvokeTx {
                    sender: &self.sender,
                    calldata: &self.calldata,
                    nonce: &self.nonce,
                    bounds: None,
                    signature: &self.query_signature,
                };
                (
                    rpc_bodies::estimate(id, Some(&query), None, false),
                    RpcPurpose::EstimateFee,
                )
            }
        };
        request(&mut self.stage, id, expect, body, purpose)
    }

    fn on_reply(
        &mut self,
        expect: Expect,
        answer: Option<Answer>,
    ) -> Result<MultisigStep, MultisigFailure> {
        match expect {
            Expect::Chain => {
                chain_matches(answer, self.network)?;
                Ok(self.ask(Expect::Nonce))
            }
            Expect::Nonce => {
                self.nonce = felt_result(answer)?;
                Ok(self.ask(Expect::Estimate))
            }
            _ => {
                let entry = match judge::estimate(answer) {
                    EstimateAnswer::Entries(entries) if entries.len() == 1 => entries,
                    EstimateAnswer::NodeError => return Err(MultisigFailure::NodeError),
                    _ => return Err(MultisigFailure::PreparationUnavailable),
                };
                let (bounds, _) =
                    quote_with(&entry[0], &self.overhead).map_err(|refusal| match refusal {
                        FeeRefusal::BoundsOutOfRange => MultisigFailure::BoundsOutOfRange,
                        _ => MultisigFailure::PreparationUnavailable,
                    })?;
                let prepared = PreparedInvoke::freeze(
                    self.network,
                    &self.sender,
                    std::mem::take(&mut self.calls),
                    &self.calldata,
                    &self.nonce,
                    &bounds,
                )
                .ok_or(MultisigFailure::BoundsOutOfRange)?;
                Ok(MultisigStep::Prepared(Box::new(prepared)))
            }
        }
    }
}

/// `submitPreparedArgentMultisigInvokeV3`: broadcasts the exact frozen fields with a complete
/// threshold signature. No nonce read, estimate, rebuild or signing.
pub struct SubmitPlan {
    network: NetworkId,
    sender: Felt,
    calldata: Vec<Felt>,
    nonce: Felt,
    bounds: super::fees::Bounds,
    hash: Felt,
    signature: Vec<Felt>,
    next_id: u64,
    stage: Stage,
}

impl SubmitPlan {
    /// Re-validates the intent's prepared transaction (its calldata and hash recomputed),
    /// serialises exactly `config.threshold` member signatures in GUID order, and then requires
    /// what the extension's `resolveSubmission` reads from its store: a ready, stale or submitted
    /// intent that [`SigningIntent::is_valid`] accepts, so every signature is verified over the
    /// hash and the count is the snapshot's threshold.
    pub fn new(
        intent: &SigningIntent,
        config: &ArgentMultisigConfig,
        class: &ArgentMultisigClass,
        max_summary_length: usize,
        primitives: &dyn StarkPrimitives,
    ) -> Result<Self, MultisigInvokeError> {
        let prepared = &intent.prepared;
        let (network, sender, calldata, nonce, bounds) = prepared
            .checked(primitives)
            .ok_or(MultisigInvokeError::InvalidPrepared)?;
        let signature = signature_felts(&intent.signatures, config, class)?;
        let submittable = matches!(
            intent.state,
            IntentState::Ready | IntentState::Stale | IntentState::Submitted
        );
        if !submittable || !intent.is_valid(class, max_summary_length, primitives) {
            return Err(MultisigInvokeError::InvalidIntent);
        }
        let hash =
            felt::parse(&prepared.transaction_hash).ok_or(MultisigInvokeError::InvalidPrepared)?;
        Ok(Self {
            network,
            sender,
            calldata,
            nonce,
            bounds,
            hash,
            signature,
            next_id: 0,
            stage: Stage::Fresh,
        })
    }

    /// Feeds the last reply (`Start` first). `authorized` is asked once, after the chain is
    /// proven and before the write leaves; false ends the plan with `Locked`.
    pub fn advance(
        &mut self,
        reply: StepReply,
        authorized: &dyn Fn() -> bool,
    ) -> Result<MultisigStep, SendError> {
        let step = match take_reply(&mut self.stage, reply)? {
            None => {
                self.next_id += 1;
                let body = rpc::chain_id_request(self.next_id);
                request(
                    &mut self.stage,
                    self.next_id,
                    Expect::Chain,
                    body,
                    RpcPurpose::ChainId,
                )
            }
            Some((Expect::Chain, answer)) => match chain_matches(answer, self.network) {
                Err(failure) => MultisigStep::Failed(failure),
                Ok(()) if !authorized() => MultisigStep::Failed(MultisigFailure::Locked),
                Ok(()) => self.write(),
            },
            Some((_, answer)) => self.judge_write(answer),
        };
        Ok(step)
    }

    fn write(&mut self) -> MultisigStep {
        self.next_id += 1;
        let tx = InvokeTx {
            sender: &self.sender,
            calldata: &self.calldata,
            nonce: &self.nonce,
            bounds: Some(&self.bounds),
            signature: &self.signature,
        };
        let body = rpc_bodies::add_invoke(self.next_id, &tx);
        request(
            &mut self.stage,
            self.next_id,
            Expect::Write,
            body,
            RpcPurpose::AddInvoke,
        )
    }

    fn judge_write(&self, answer: Option<Answer>) -> MultisigStep {
        if let Some(Answer::Error { text, .. }) = &answer {
            if fee_expired(text) {
                return MultisigStep::Failed(MultisigFailure::FeeExpired);
            }
        }
        MultisigStep::Failed(match judge::write(answer, &self.hash) {
            WriteAnswer::Accepted => {
                return MultisigStep::Submitted {
                    hash: felt::canonical(&self.hash),
                }
            }
            WriteAnswer::Mismatch => MultisigFailure::HashMismatch,
            WriteAnswer::Indeterminate => MultisigFailure::SubmissionIndeterminate,
            WriteAnswer::NodeError => MultisigFailure::NodeError,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::fee_expired;

    #[test]
    fn fee_expiry_is_the_price_floor_refusal_only() {
        assert!(fee_expired(
            "validation failure max l1 data gas price (1) is lower than the actual gas price: 2"
        ));
        assert!(fee_expired(
            "max l2 gas price lower than the actual gas price"
        ));
        assert!(!fee_expired("max fee is lower than the actual gas price"));
        assert!(!fee_expired("invalid transaction nonce"));
    }
}
