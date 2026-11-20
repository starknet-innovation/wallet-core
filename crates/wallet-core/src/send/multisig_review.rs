//! The review of a call batch for an Argent multisig, as the extension's
//! `requireSafeArgentMultisigCalls` (`packages/features/multisig/src/chain.ts`) and
//! `reviewArgentMultisigCalls` (`apps/extension/src/background/multisig/multisig-call-review.ts`):
//! a signer-management self-call becomes a [`ConfigReview`] against the current configuration;
//! ordinary calls become the summary of the first (`call_decode`) and a count of the rest, and a
//! payee that mimics an address the person trusts is refused (`refuseOnPoisonedRecipient`).
//!
//! What the wallet knows arrives as arguments: its trusted tokens and watched collections
//! ([`Known`]), the addresses the person recognises, and its summary length. Refusals are typed;
//! each one's `Display` is the extension's own sentence.

use std::fmt;

use serde::Serialize;
use starknet_types_core::felt::Felt;

use super::call_decode::{call_summary, recipient_for_flags, Known};
use super::calls::is_lookalike_address;
use crate::accounts::multisig::ArgentMultisigClass;
use crate::accounts::multisig_call::{is_argent_multisig_calls, ArgentMultisigCall};
use crate::accounts::multisig_change::{
    parse_config_change, review_config_change, ChangeError, ConfigReview,
};
use crate::accounts::multisig_records::PublicConfig;
use crate::felt;
use crate::js::felt::same_felt;
use crate::js::num::{is_js_whitespace, utf16_slice};

/// The account's two internal upgrade callbacks, never called directly. The extension compares
/// `getSelectorFromName` hashes, which for these names is the same as comparing the names.
const FORBIDDEN_SELF_CALLS: [&str; 2] = ["execute_after_upgrade", "perform_upgrade"];

/// `ArgentMultisigCallReview`: a trusted description of one validated batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CallReview {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub config: Option<ConfigReview>,
    pub summary: String,
}

/// Why a batch was refused. `Display` is the extension's sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallReviewError {
    InvalidCalls,
    BatchSelfCall,
    UpgradeCallback,
    /// The self-call is not a valid change of the current configuration.
    Change(ChangeError),
    /// A self-call was reviewed without the account's current configuration.
    MissingCurrentConfig,
    /// A payee closely mimics an address the person trusts.
    PoisonedRecipient,
}

impl CallReviewError {
    pub fn message(self) -> &'static str {
        match self {
            Self::InvalidCalls => "Invalid Argent multisig calls",
            Self::BatchSelfCall => "Argent multisig batches cannot contain account self-calls",
            Self::UpgradeCallback => {
                "Argent multisig internal upgrade callbacks cannot be called directly"
            }
            Self::Change(error) => error.message(),
            Self::MissingCurrentConfig => {
                "Signer-management intents require the complete current public signer configuration"
            }
            Self::PoisonedRecipient => {
                "The recipient closely mimics an address you saved or paid but is a different account. Re-check every character before sending."
            }
        }
    }
}

impl fmt::Display for CallReviewError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for CallReviewError {}

impl From<ChangeError> for CallReviewError {
    fn from(error: ChangeError) -> Self {
        Self::Change(error)
    }
}

/// `requireSafeArgentMultisigCalls`: Argent Multisig 0.2's call-set restriction. A batch may not
/// hold a self-call, the upgrade callbacks are never called, and a lone self-call must parse as a
/// signer change.
pub fn require_safe_calls(
    account_address: &str,
    calls: &[ArgentMultisigCall],
    class: &ArgentMultisigClass,
) -> Result<(), CallReviewError> {
    if !is_argent_multisig_calls(calls) {
        return Err(CallReviewError::InvalidCalls);
    }
    let mut self_calls = calls
        .iter()
        .filter(|call| same_felt(&call.contract_address, account_address));
    let Some(call) = self_calls.next() else {
        return Ok(());
    };
    if calls.len() > 1 {
        return Err(CallReviewError::BatchSelfCall);
    }
    if FORBIDDEN_SELF_CALLS.contains(&call.entrypoint.as_str()) {
        return Err(CallReviewError::UpgradeCallback);
    }
    parse_config_change(account_address, call, class)?;
    Ok(())
}

/// The felts of a call `is_argent_multisig_calls` accepted.
fn felts(values: &[String]) -> Vec<Felt> {
    values
        .iter()
        .filter_map(|value| felt::parse(value))
        .collect()
}

/// `reviewArgentMultisigCalls` for the multisig at `account_address`. `current` is its complete
/// current configuration, required only for a self-call. `trusted` is every other address the
/// person recognises on this network (their own accounts, saved contacts and recipients this
/// wallet paid); the multisig itself is always trusted. `summary_length` is the wallet's cap on
/// the stored summary, in UTF-16 units.
pub fn review_calls(
    account_address: &str,
    calls: &[ArgentMultisigCall],
    current: Option<&PublicConfig>,
    class: &ArgentMultisigClass,
    known: &Known,
    trusted: &[Felt],
    summary_length: usize,
) -> Result<CallReview, CallReviewError> {
    require_safe_calls(account_address, calls, class)?;
    if let Some(call) = calls
        .iter()
        .find(|call| same_felt(&call.contract_address, account_address))
    {
        let current = current.ok_or(CallReviewError::MissingCurrentConfig)?;
        let review = review_config_change(account_address, current, call, class)?;
        return Ok(CallReview {
            summary: review.summary.clone(),
            config: Some(review),
        });
    }
    let account = felt::parse(account_address);
    let mut summaries = Vec::with_capacity(calls.len());
    for call in calls {
        let contract = felt::parse(&call.contract_address).ok_or(CallReviewError::InvalidCalls)?;
        let calldata = felts(&call.calldata);
        if let Some(recipient) = recipient_for_flags(&call.entrypoint, &calldata) {
            if account
                .iter()
                .chain(trusted)
                .any(|address| is_lookalike_address(&recipient, address))
            {
                return Err(CallReviewError::PoisonedRecipient);
            }
        }
        summaries.push(call_summary(&contract, &call.entrypoint, &calldata, known));
    }
    let suffix = match summaries.len() {
        0 | 1 => String::new(),
        count => format!(" (+{} more)", count - 1),
    };
    let first = summaries
        .first()
        .map_or("Review Argent multisig call", String::as_str);
    // Trimmed after the cut, as the extension does: a cut can end on a space.
    let summary = utf16_slice(&format!("{first}{suffix}"), 0, summary_length);
    Ok(CallReview {
        config: None,
        summary: summary.trim_end_matches(is_js_whitespace).to_string(),
    })
}

#[cfg(test)]
#[path = "multisig_review_tests.rs"]
mod tests;
