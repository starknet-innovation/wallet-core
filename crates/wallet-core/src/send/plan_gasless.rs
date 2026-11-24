//! The reviewed transfer a gasless fee may pay for (`handleReviewTransfer`'s offers and
//! `submitChosenTransfer`'s gasless branch): what the offer round and the submission need from a
//! plan, and the one hand-over from its STRK path to the gasless one. Only a transfer the review
//! found short of STRK (`Insufficient`, an FRI fee) qualifies, as the extension offers gasless
//! only then; its recipient passed the review's poisoning check, as a Confirm would require.

use super::calls::{execute_calldata, Call, ReviewFlag};
use super::fees::{self, FeeUnit};
use super::plan::{Kind, Plan, Stage};
use super::types::FeeAffordability;
use super::u256::U256;
use crate::Felt;

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
    pub fn take_for_gasless(&mut self) -> Option<GaslessBasis> {
        let basis = self.gasless_basis()?;
        self.close();
        Some(basis)
    }
}
