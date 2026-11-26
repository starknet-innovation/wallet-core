//! "Update network fee" on a stale intent (`openUpdatedIntent` in `multisig-intent-ui.ts`): open
//! the fee-updated version already stored (`feeReplacementIntentFor`), else propose the stale
//! intent's own stored calls again (`MultisigProposalRequest::Repropose`). The host names the
//! intent; the calls never cross the FFI, and a proposal reviews them afresh.

use wallet_core::accounts::multisig_records::Record;
use wallet_core::js::felt::same_felt;
use wallet_core::send::multisig_intent::{IntentState, SigningIntent};
use wallet_core::send::multisig_invoke::MultisigCall;
use wallet_core::send::multisig_stale::same_calls;

use crate::multisig_tx::{intent_view, MultisigIntent};
use crate::multisig_tx_ops::refused;
use crate::wallet::WalletCore;
use crate::{multisig_intents, CoreError};

/// The extension's text when the intent is gone or no longer stale.
const UNAVAILABLE: &str = "That transaction is unavailable";

/// `sameAction`: the same multisig, snapshot (class, threshold, GUIDs in order), nonce and calls.
fn same_action(left: &SigningIntent, right: &SigningIntent) -> bool {
    let (a, b) = (&left.config_snapshot, &right.config_snapshot);
    left.account_id == right.account_id
        && same_felt(&a.class_hash, &b.class_hash)
        && a.threshold == b.threshold
        && a.signer_guids.len() == b.signer_guids.len()
        && a.signer_guids
            .iter()
            .zip(&b.signer_guids)
            .all(|(x, y)| same_felt(x, y))
        && same_felt(&left.prepared.nonce, &right.prepared.nonce)
        && same_calls(&left.prepared.calls, &right.prepared.calls)
}

/// `feeReplacementIntentFor`: the open version of `stale`'s action with the most signatures (the
/// last of equals, as the extension's reduce keeps it), if `stale` is stale.
fn fee_replacement<'a>(
    stale: &SigningIntent,
    intents: &'a [SigningIntent],
) -> Option<&'a SigningIntent> {
    if stale.state != IntentState::Stale {
        return None;
    }
    intents
        .iter()
        .filter(|candidate| {
            candidate.id != stale.id
                && matches!(
                    candidate.state,
                    IntentState::Collecting | IntentState::Ready
                )
                && same_action(stale, candidate)
        })
        .fold(None, |best: Option<&SigningIntent>, candidate| match best {
            Some(best) if candidate.signatures.len() < best.signatures.len() => Some(best),
            _ => Some(candidate),
        })
}

impl WalletCore {
    /// The calls `Repropose` proposes again: `intent_id`'s, while it is a stale intent of
    /// `record`.
    pub(crate) fn stale_calls(
        &self,
        intent_id: &str,
        record: &Record,
    ) -> Result<Vec<MultisigCall>, CoreError> {
        multisig_intents::get(self.data_dir(), intent_id)?
            .filter(|intent| intent.account_id == record.id && intent.state == IntentState::Stale)
            .map(|intent| intent.prepared.calls)
            .ok_or_else(|| refused(UNAVAILABLE))
    }
}

#[uniffi::export]
impl WalletCore {
    /// `feeReplacementIntentFor`: the fee-updated version of the stale intent `intent_id` already
    /// stored (collecting or ready, same snapshot, nonce and calls), with the most signatures.
    /// `None` when there is none or the intent is not stale. The stale card reads it for its
    /// "Continue" label and opens it instead of proposing again.
    pub fn multisig_fee_replacement(
        &self,
        intent_id: String,
    ) -> Result<Option<MultisigIntent>, CoreError> {
        self.require_unlocked()?;
        let intents = multisig_intents::load(self.data_dir())?;
        let Some(stale) = intents.iter().find(|intent| intent.id == intent_id) else {
            return Err(refused(UNAVAILABLE));
        };
        let Some(replacement) = fee_replacement(stale, &intents) else {
            return Ok(None);
        };
        let record = self.multisig_record(&replacement.account_id).ok();
        Ok(Some(intent_view(replacement, record.as_ref())))
    }
}
