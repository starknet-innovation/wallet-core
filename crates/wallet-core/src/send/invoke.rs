//! A batch of calls from a single-signer account, the plan's `Kind::Invoke`: what the extension's
//! `submitInvokeTransaction` (`packages/platform/starknet/src/rpc/rpc-submit.ts`) takes, with the
//! checks its callers make before signing. The batch is reviewed as a multisig batch is
//! (`multisig_review::summarise_calls`): a payee that mimics an address the person trusts refuses
//! it (`refuseOnPoisonedRecipient`), and the review's summary is what the activity row records.
//! The plan signs the `__execute__` calldata derived here, from the calls the review read.
//!
//! Krusty 3.1.0 has no selector (`starknet_keccak` of an entrypoint name), so, as for a multisig,
//! the caller lends one through `multisig_invoke::StarkPrimitives`.

use super::call_decode::Known;
use super::calls::{execute_calldata, Call};
use super::multisig_invoke::StarkPrimitives;
use super::multisig_review::{summarise_calls, CallReviewError};
use super::types::Account;
use crate::history::activity::MAX_SUMMARY_LENGTH;
use crate::Felt;

/// Why a batch was refused before any plan exists. `Display` is the extension's sentence where it
/// has one; none carries a call, an address or an amount.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvokeRefusal {
    /// `submitInvokeTransaction`'s first check.
    #[error("No calls to submit")]
    NoCalls,
    /// An entrypoint the selector primitive could not name.
    #[error("invalid call")]
    InvalidCall,
    /// The call review refused the batch.
    #[error("{0}")]
    Review(#[from] CallReviewError),
}

/// One reviewed batch. Built only by `Invoke::new`, so the calldata the plan signs is always the
/// calls the review described.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invoke {
    pub(crate) calldata: Vec<Felt>,
    pub(crate) summary: String,
}

impl Invoke {
    /// Reviews `calls` from `account` and derives their calldata. `known` is the wallet's trusted
    /// tokens and watched collections on the account's network, for the summary; `trusted` every
    /// other address the person recognises there (as for `Transfer::new`), for the poisoned-payee
    /// refusal.
    pub fn new(
        account: &Account,
        calls: Vec<Call>,
        known: &Known,
        trusted: &[Felt],
        primitives: &dyn StarkPrimitives,
    ) -> Result<Self, InvokeRefusal> {
        if calls.is_empty() {
            return Err(InvokeRefusal::NoCalls);
        }
        let summary = summarise_calls(
            &account.address,
            &calls,
            known,
            trusted,
            MAX_SUMMARY_LENGTH,
            "",
        )?;
        let calldata = execute_calldata(&calls, |entrypoint| primitives.selector(entrypoint))
            .ok_or(InvokeRefusal::InvalidCall)?;
        Ok(Self { calldata, summary })
    }

    /// The review's line: the first call's summary and a count of the rest.
    pub fn summary(&self) -> &str {
        &self.summary
    }
}

#[cfg(test)]
#[path = "invoke_tests.rs"]
mod tests;
