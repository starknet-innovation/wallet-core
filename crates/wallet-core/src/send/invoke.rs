//! A batch of calls from a single-signer account, the plan's `Kind::Invoke`: what the extension's
//! `submitInvokeTransaction` (`packages/platform/starknet/src/rpc/rpc-submit.ts`) takes, with the
//! checks its callers make before signing. The batch is reviewed as a multisig batch is
//! (`multisig_review`'s refusal and line): a payee that mimics an address the person trusts refuses
//! it (`refuseOnPoisonedRecipient`), and the review's summary is what the activity row records
//! unless the builder names another.
//! A call to the account itself is refused (`refuseOnAccountSelfCall`), and the batch must fit
//! the wallet's `Policy` and the call grammar of the extension's `readProviderInvokeCall`. The
//! plan signs the `__execute__` calldata derived here, from the calls the review read, and holds
//! the batch to `check` again before it reads anything.
//!
//! Besides the line, the review keeps every call's `decodeCall` kind and summary and the annotation
//! flags `verifyCalls` raises on it (`review_flags`), and the builder may name the activity row's
//! text as the extension's callers do (a swap's or a stake's line, a dapp's "<origin> submitted N
//! calls"), cut as the activity log cuts it.
//!
//! Krusty 3.1.0 has no selector (`starknet_keccak` of an entrypoint name), so, as for a multisig,
//! the caller lends one through `multisig_invoke::StarkPrimitives`.

use super::call_decode::{decode_call, CallKind};
use super::calls::{execute_calldata, Call};
use super::multisig_invoke::StarkPrimitives;
use super::multisig_review::{batch_line, refuse_poisoned, CallReviewError};
use super::plan::PendingKind;
use super::review_flags::{call_flags, dedupe, ReviewContext, RiskFlag};
use super::types::{Account, Policy};
use super::u256::U256;
use crate::allowances::calls::{allowance_changes, AllowanceChange, AllowanceOwner};
use crate::history::activity::MAX_SUMMARY_LENGTH;
use crate::js::num::utf16_slice;
use crate::{Felt, NetworkId};

/// `activityOrigin`'s bound: a longer origin keeps its end, where the registrable domain is.
const MAX_ACTIVITY_ORIGIN_LENGTH: usize = 80;

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

/// One reviewed batch. Built only through `Invoke::reviewed`, so the calldata the plan signs is
/// always the calls the review described.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invoke {
    pub(crate) network: NetworkId,
    pub(crate) sender: Felt,
    pub(crate) calls: Vec<Call>,
    pub(crate) calldata: Vec<Felt>,
    pub(crate) summary: String,
    pub(crate) reviewed: Vec<ReviewedCall>,
    pub(crate) activity_summary: Option<String>,
    pub(crate) activity_asset: Option<ActivityAsset>,
    pub(crate) allowances: Vec<AllowanceChange>,
    pub(crate) fee_balance: Option<U256>,
}

/// The amount and token an activity row names beside its text (`recordSubmittedActivity`'s
/// `amount` and `activityToken`): what a swap sells, what a stake moves. The token is named by
/// address as well as symbol, so a row never rests on a symbol another contract may share.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivityAsset {
    /// The amount as the person entered it; `None` for an action that names none (a claim).
    pub amount: Option<String>,
    pub symbol: String,
    pub token_address: Felt,
}

/// One call as the review shows it: `decodeCall`'s kind and summary, and the flags `verifyCalls`
/// raises on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewedCall {
    pub kind: CallKind,
    pub summary: String,
    pub flags: Vec<RiskFlag>,
}

/// `clampActivitySummary`: at most `MAX_SUMMARY_LENGTH` UTF-16 units, a longer one cut to one
/// fewer and ended with an ellipsis.
fn clamp_activity_summary(summary: &str) -> String {
    if summary.encode_utf16().nth(MAX_SUMMARY_LENGTH).is_none() {
        return summary.to_string();
    }
    format!("{}…", utf16_slice(summary, 0, MAX_SUMMARY_LENGTH - 1))
}

/// `activityOrigin`: an origin of at most 80 UTF-16 units as it is, a longer one as an ellipsis
/// and its last 79.
fn activity_origin(origin: &str) -> String {
    let length = origin.encode_utf16().count();
    if length <= MAX_ACTIVITY_ORIGIN_LENGTH {
        return origin.to_string();
    }
    let start = length - (MAX_ACTIVITY_ORIGIN_LENGTH - 1);
    format!("…{}", utf16_slice(origin, start, length))
}

/// `readProviderInvokeCall`'s entrypoint grammar: `^[A-Za-z_][A-Za-z0-9_]*$`, at most 64.
pub(crate) fn is_entrypoint(name: &str) -> bool {
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
    /// batch (the plan's must be the same one, or the plan refuses it); `context` is everything
    /// the wallet knows on the account's network (`verifyCalls` with its `knownContracts` and
    /// `standing`): `known` for the summaries, `trusted` for the poisoned-payee refusal, and all
    /// of it for the flags. The only public constructor, so no host reviews a batch with less: a
    /// partial context under-warns (a router reads as unknown, STRK as no token, no allowance is
    /// seeded).
    pub fn reviewed(
        account: &Account,
        calls: Vec<Call>,
        policy: &Policy,
        context: &ReviewContext,
        primitives: &dyn StarkPrimitives,
    ) -> Result<Self, InvokeRefusal> {
        check(&account.address, &calls, policy)?;
        refuse_poisoned(&account.address, &calls, context.trusted)?;
        let decoded: Vec<_> = calls
            .iter()
            .map(|call| {
                decode_call(
                    &call.contract_address,
                    &call.entrypoint,
                    &call.calldata,
                    &context.known,
                )
            })
            .collect();
        let flags = call_flags(&calls, &decoded, account, context);
        let summaries: Vec<&str> = decoded.iter().map(|call| call.summary.as_str()).collect();
        let summary = batch_line(&summaries, MAX_SUMMARY_LENGTH, "");
        let calldata = execute_calldata(&calls, |entrypoint| primitives.selector(entrypoint))
            .ok_or(InvokeRefusal::InvalidCall)?;
        let reviewed = decoded
            .into_iter()
            .zip(flags)
            .map(|(call, flags)| ReviewedCall {
                kind: call.kind,
                summary: call.summary,
                flags,
            })
            .collect();
        // What the batch grants, read with the lists the review read (`allowanceChangesFromCalls`
        // under `allowanceOwner(account)`), so the plan's pending record carries it to the log.
        let allowances = AllowanceOwner::of(account.network, &account.address)
            .map(|owner| allowance_changes(&calls, &owner, &context.known))
            .unwrap_or_default();
        Ok(Self {
            network: account.network,
            sender: account.address,
            calls,
            calldata,
            summary,
            reviewed,
            activity_summary: None,
            activity_asset: None,
            allowances,
            fee_balance: None,
        })
    }

    /// The review's line: the first call's summary and a count of the rest.
    pub fn summary(&self) -> &str {
        &self.summary
    }

    /// Every call as the review shows it, in batch order.
    pub fn calls(&self) -> &[ReviewedCall] {
        &self.reviewed
    }

    /// The batch's flags as the extension's card lists them: every call's, in order, each line
    /// once (`dedupeFlags`).
    pub fn flags(&self) -> Vec<&RiskFlag> {
        dedupe(self.reviewed.iter().flat_map(|call| call.flags.iter()))
    }

    /// Names the activity row's text, as a swap or a stake does; cut as the activity log cuts it.
    /// An empty text leaves the review's line. The text is the wallet's own, never one a dapp's
    /// message carried (`activitySummary` is background-authored only in the extension): the row
    /// is what the person later reads as what they signed, and nothing checks it against the calls.
    #[must_use]
    pub fn with_activity_summary(mut self, summary: &str) -> Self {
        self.activity_summary = (!summary.is_empty()).then(|| clamp_activity_summary(summary));
        self
    }

    /// Names the amount and token of the activity row, for the crate's own builders (a swap, a
    /// stake): a dapp's batch names neither.
    #[must_use]
    pub(crate) fn with_activity_asset(mut self, asset: ActivityAsset) -> Self {
        self.activity_asset = Some(asset);
        self
    }

    /// The STRK the host last read for the sending account, for the review's shortfall warning
    /// only (`resolveFeeAffordability`, fee-only for a batch). Without it the review says
    /// `Unknown`, never blocking. It rides on the batch the plan binds to its account and network.
    #[must_use]
    pub fn with_fee_balance(mut self, balance: U256) -> Self {
        self.fee_balance = Some(balance);
        self
    }

    /// The activity text of a dapp's batch: "<origin> submitted N call(s)", N this batch's own
    /// count and the origin as `activityOrigin` cuts it. `origin` is the one the wallet saw the
    /// request come from, never a name the dapp gave itself.
    #[must_use]
    pub fn with_dapp_origin(self, origin: &str) -> Self {
        let count = self.calls.len();
        let plural = if count == 1 { "" } else { "s" };
        let text = format!("{} submitted {count} call{plural}", activity_origin(origin));
        self.with_activity_summary(&text)
    }

    /// What the activity row records: the builder's text, or the review's line.
    pub fn activity_summary(&self) -> &str {
        self.activity_summary.as_deref().unwrap_or(&self.summary)
    }

    /// What the plan's pending record says of this batch: the row's amount and token, when its
    /// builder named them, and the allowance changes to file under the signed hash.
    pub(crate) fn pending_kind(&self) -> PendingKind {
        PendingKind::Invoke {
            asset: self.activity_asset.clone(),
            allowances: self.allowances.clone(),
        }
    }

    /// Every ERC-20 allowance change the batch carries (`allowanceChangesFromCalls`), owned by
    /// its sender: what the plan's pending record hands the host to file.
    pub fn allowance_changes(&self) -> &[AllowanceChange] {
        &self.allowances
    }
}

#[cfg(test)]
#[path = "invoke_tests.rs"]
mod tests;
