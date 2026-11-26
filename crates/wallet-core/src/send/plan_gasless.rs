//! The reviewed transfer a gasless fee may pay for (`handleReviewTransfer`'s offers and
//! `submitChosenTransfer`'s gasless branch): what the offer round and the submission need from a
//! plan, and the one hand-over from its STRK path to the gasless one. Only a transfer the review
//! found short of STRK (`Insufficient`, an FRI fee) qualifies, as the extension offers gasless
//! only then; its recipient passed the review's poisoning check, as a Confirm would require.
//!
//! A Ledger account (`Plan::with_device`) signs the outside execution on its device, as the
//! extension's `accountHashSigner` routes it: the plan taken for gasless stays open only to sign
//! the hash of a `CheckedQuote` (the gateway's build, checked against the reviewed call), through
//! the same device exchange its invoke would use (`plan_device`), and hands the account's
//! signature back (`sign_relay`, `advance_relay`). It never signs a hash the host names.

use super::calls::{execute_calldata, Call, ReviewFlag};
use super::fees::{self, FeeUnit};
use super::plan::{Kind, Plan, PlanHost, Stage};
use super::plan_device::SignFor;
use super::types::{FeeAffordability, SendError, SendStep, StepReply};
use super::u256::U256;
use crate::gasless::CheckedQuote;
use crate::Felt;

/// Where a device plan's relay signature stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayStep {
    /// `SendStep::Device`, `SendStep::DeviceRetry` or `SendStep::Failed`, answered as a plan's.
    Step(SendStep),
    /// The account's signature over the checked outside execution, verified on its key.
    Signed(Vec<Felt>),
}

/// One reviewed transfer, as the gasless path takes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GaslessBasis {
    /// `buildErc20TransferCall`: the one call the outside execution must carry.
    pub call: Call,
    /// The review's STRK fee (`fees.totalFee`), which the token reference converts.
    pub strk_fee: U256,
    /// Whether the review found the account deployed.
    pub deployed: bool,
    /// The transfer's own row: its summary, amount as typed, recipient, token and symbol.
    pub summary: String,
    pub amount_text: String,
    pub recipient: Felt,
    pub token_address: Felt,
    pub symbol: String,
}

impl Plan {
    /// The gasless basis while this plan is a reviewed transfer, not confirmed, whose recipient
    /// is not poisoned and whose STRK falls short of its FRI fee; `None` otherwise.
    pub fn gasless_basis(&self) -> Option<GaslessBasis> {
        let Kind::Transfer(transfer) = &self.kind else {
            return None;
        };
        let review = self.current_review()?;
        let work = &self.work;
        let fee = match (work.requires_deployment, &work.fee, &work.deploy_fee) {
            (false, Some(fee), _) => *fee,
            (true, Some(fee), Some(deploy)) => fees::sum(fee, deploy)?,
            _ => return None,
        };
        let call = Call {
            contract_address: transfer.token_address,
            entrypoint: "transfer".into(),
            calldata: vec![
                transfer.recipient,
                Felt::from(transfer.amount.low),
                Felt::from(transfer.amount.high),
            ],
        };
        let qualifies = matches!(self.stage, Stage::Reviewed)
            // `execute_calldata` names the entrypoint (always by name); the outside execution takes
            // a felt entrypoint as its own selector (`selector_of`). Equal for this one "transfer";
            // a batch with felt entrypoints would need both to agree before it is widened to.
            && execute_calldata(std::slice::from_ref(&call)) == transfer.calldata
            && !transfer.flags.contains(&ReviewFlag::AddressPoisoning)
            && review.affordability == FeeAffordability::Insufficient
            && fee.unit == FeeUnit::Fri
            && fee.spend != U256::ZERO;
        qualifies.then(|| GaslessBasis {
            call,
            strk_fee: fee.spend,
            deployed: work.deployed == Some(true) && !work.requires_deployment,
            summary: transfer.summary.clone(),
            amount_text: transfer.amount_text.clone(),
            recipient: transfer.recipient,
            token_address: transfer.token_address,
            symbol: transfer.symbol.clone(),
        })
    }

    /// Hands the reviewed transfer over to the gasless path, once: its basis, with the plan
    /// closed so its STRK path can never also be confirmed (the extension's held review is
    /// consumed the same way). `None`, and the plan untouched, when it does not qualify.
    /// A device plan stays open for its relay signature alone (`sign_relay`).
    pub fn take_for_gasless(&mut self) -> Option<GaslessBasis> {
        let basis = self.gasless_basis()?;
        self.stage = match self.device {
            Some(_) => Stage::Relay,
            None => Stage::Closed,
        };
        Some(basis)
    }

    /// Whether this plan's account signs on a device: its relay only through `sign_relay`.
    pub fn signs_on_device(&self) -> bool {
        self.device.is_some()
    }

    /// Starts signing `quote`'s outside execution on the device, once, for a device plan taken
    /// for gasless. Anything else is `OutOfOrder` and closes the plan.
    pub fn sign_relay(
        &mut self,
        host: &dyn PlanHost,
        quote: &CheckedQuote<'_>,
    ) -> Result<RelayStep, SendError> {
        if !matches!(self.stage, Stage::Relay) {
            self.close();
            return Err(SendError::OutOfOrder);
        }
        let step = self.sign_for(host, quote.message_hash(), SignFor::Relay);
        self.relay_step(step)
    }

    /// Feeds the device's answer (`Device`, `DeviceFailure`) or `RetryDevice` to the relay
    /// signature in progress.
    pub fn advance_relay(
        &mut self,
        host: &dyn PlanHost,
        reply: StepReply,
    ) -> Result<RelayStep, SendError> {
        if !self.relaying() {
            self.close();
            return Err(SendError::OutOfOrder);
        }
        let step = self.run(host, reply)?;
        self.relay_step(step)
    }

    /// Whether the plan was taken for gasless on a device and has not yet handed back (or
    /// failed) its relay signature: `sign_relay` or `advance_relay` is what it takes next.
    pub fn relaying(&self) -> bool {
        match &self.stage {
            Stage::Relay | Stage::Relayed(_) => true,
            Stage::Device(wait) | Stage::DeviceRefused(wait) => wait.relays(),
            _ => false,
        }
    }

    /// `plan_device`'s end of a relay signature: parked for `relay_step` to hand back.
    pub(super) fn relayed(&mut self, signature: Vec<Felt>) -> SendStep {
        self.stage = Stage::Relayed(signature);
        // Never seen by a host: `relay_step` reads the stage instead.
        SendStep::Review
    }

    fn relay_step(&mut self, step: SendStep) -> Result<RelayStep, SendError> {
        match (std::mem::replace(&mut self.stage, Stage::Closed), step) {
            (Stage::Relayed(signature), _) => Ok(RelayStep::Signed(signature)),
            (stage, step @ (SendStep::Device { .. } | SendStep::DeviceRetry { .. })) => {
                self.stage = stage;
                Ok(RelayStep::Step(step))
            }
            (_, step @ SendStep::Failed { .. }) => Ok(RelayStep::Step(step)),
            _ => Err(SendError::OutOfOrder),
        }
    }
}
