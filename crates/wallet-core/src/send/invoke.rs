//! A batch of calls from a single-signer account, the plan's `Kind::Invoke`: what the extension's
//! `submitInvokeTransaction` (`packages/platform/starknet/src/rpc/rpc-submit.ts`) takes, with the
//! checks its callers make before signing. The batch is reviewed as a multisig batch is
//! (`multisig_review::summarise_calls`): a payee that mimics an address the person trusts refuses
//! it (`refuseOnPoisonedRecipient`), and the review's summary is what the activity row records.
//! A call to the account itself is refused (`refuseOnAccountSelfCall`), and the batch must fit
//! the wallet's `Policy` and the call grammar of the extension's `readProviderInvokeCall`. The
//! plan signs the `__execute__` calldata derived here, from the calls the review read, and holds
//! the batch to `check` again before it reads anything.
//!
//! Krusty 3.1.0 has no selector (`starknet_keccak` of an entrypoint name), so, as for a multisig,
//! the caller lends one through `multisig_invoke::StarkPrimitives`.

use super::call_decode::Known;
use super::calls::{execute_calldata, Call};
use super::multisig_invoke::StarkPrimitives;
use super::multisig_review::{summarise_calls, CallReviewError};
use super::types::{Account, Policy};
use crate::history::activity::MAX_SUMMARY_LENGTH;
use crate::Felt;

/// `INVOKE_LIMITS.entrypointCharacters` (`wallet-protocol/src/transactions/invoke-limits.ts`): a
/// bound of the call grammar, like the multisig one, not a wallet's choice.
const MAX_ENTRYPOINT_CHARACTERS: usize = 64;

/// Why a batch was refused before any plan exists. `Display` is the extension's sentence where it
/// has one; none carries a call, an address or an amount.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvokeRefusal {
    /// `submitInvokeTransaction`'s first check.
    #[error("No calls to submit")]
    NoCalls,
    /// More calls than the wallet's `Policy::max_invoke_calls`.
    #[error("too many calls")]
    TooManyCalls,
    /// A call with more calldata than the wallet's `Policy::max_call_calldata_felts`.
    #[error("calldata too long")]
    CalldataTooLong,
    /// A zero contract, an entrypoint outside the grammar, or one the selector primitive could
    /// not name.
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
    pub(crate) sender: Felt,
    pub(crate) calls: Vec<Call>,
    pub(crate) calldata: Vec<Felt>,
    pub(crate) summary: String,
}

/// `readProviderInvokeCall`'s entrypoint grammar: `^[A-Za-z_][A-Za-z0-9_]*$`, at most 64.
fn is_entrypoint(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() <= MAX_ENTRYPOINT_CHARACTERS
        && bytes
            .first()
            .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_')
        && bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'_')
}

/// The rules that need neither the wallet's context nor a selector: at least one call and no more
/// than `policy` allows, each a non-zero contract with a grammatical entrypoint, and none to
/// `sender` itself. Felts compare by value, so no spelling of the account slips past.
pub(crate) fn check(sender: &Felt, calls: &[Call], policy: &Policy) -> Result<(), InvokeRefusal> {
    if calls.is_empty() {
        return Err(InvokeRefusal::NoCalls);
    }
    if calls.len() > policy.max_invoke_calls {
        return Err(InvokeRefusal::TooManyCalls);
    }
    for call in calls {
        if call.calldata.len() > policy.max_call_calldata_felts {
            return Err(InvokeRefusal::CalldataTooLong);
        }
        if call.contract_address == Felt::ZERO || !is_entrypoint(&call.entrypoint) {
            return Err(InvokeRefusal::InvalidCall);
        }
    }
    if calls.iter().any(|call| call.contract_address == *sender) {
        return Err(CallReviewError::AccountSelfCall.into());
    }
    Ok(())
}

impl Invoke {
    /// Checks and reviews `calls` from `account` and derives their calldata. `policy` bounds the
    /// batch (the plan's must be the same one, or the plan refuses it); `known` is the wallet's
    /// trusted tokens and watched collections on the account's network, for the summary;
    /// `trusted` every other address the person recognises there (as for `Transfer::new`), for
    /// the poisoned-payee refusal.
    pub fn new(
        account: &Account,
        calls: Vec<Call>,
        policy: &Policy,
        known: &Known,
        trusted: &[Felt],
        primitives: &dyn StarkPrimitives,
    ) -> Result<Self, InvokeRefusal> {
        check(&account.address, &calls, policy)?;
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
        Ok(Self {
            sender: account.address,
            calls,
            calldata,
            summary,
        })
    }

    /// The review's line: the first call's summary and a count of the rest.
    pub fn summary(&self) -> &str {
        &self.summary
    }
}

#[cfg(test)]
#[path = "invoke_tests.rs"]
mod tests;
