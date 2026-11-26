//! Signing a gasless transfer's checked outside execution (`gasless.rs`): with the account's key
//! (`WalletCore::plan_sign`), or, for a Ledger account, on its device inside its plan
//! (`Plan::sign_relay`, over `plan_device`'s exchange: app version, the stored key, the
//! signature verified on it, a lock stopping the APDUs). Either way the signature is over the hash
//! of the build the core checked against the reviewed call, and the transfer's row is journaled
//! before its execute is handed out (`recordRequiredRelaySignature`), so a lost answer cannot pay
//! twice; the relay's answer settles it.

use std::sync::PoisonError;

use wallet_core::activity::detail::gasless_fee_summary;
use wallet_core::gasless::{
    CheckedQuote, GaslessCode, GaslessError, GaslessFeeOffer, GaslessSubmission,
};
use wallet_core::gasless::{SubmitRound, SubmitStep};
use wallet_core::history::activity::RelayExecution;
use wallet_core::send::plan::{PendingKind, PendingRecord};
use wallet_core::send::plan_gasless::RelayStep;
use wallet_core::send::types::{SendError as CoreSendError, SendFailureCode};
use wallet_core::send::u256::U256;
use wallet_core::Felt;

use super::gasless::{GaslessFee, GaslessFeeError, GaslessSubmitStep, Stage};
use super::types::{SendStep, StepReply};
use crate::activity::with_log;
use crate::felt;
use crate::wallet::WalletCore;

/// `INVOKE_NOT_JOURNALED`: the execution could not be filed before posting, so it was not posted.
const INVOKE_NOT_JOURNALED: &str =
    "The transaction could not be recorded before sending. Nothing was submitted — try again.";

/// The extension's `ledgerStarkHashSigner` refusal for a device signature that did not verify.
const LEDGER_VERIFY_FAILED: &str = "Ledger signature verification failed";

impl GaslessFee {
    /// Signs the checked hash for the plan's record, still in its session, and serialises the
    /// account's signature (`serializeAccountSignature`).
    pub(super) fn sign(
        &self,
        core: &WalletCore,
        hash: &Felt,
    ) -> Result<Vec<Felt>, GaslessFeeError> {
        let signature = core
            .plan_sign(self.plan.generation, &self.plan.account_id, hash, true)
            .map_err(|_| GaslessFeeError::Locked)?;
        let (r, s) = (felt::parse(&signature.r), felt::parse(&signature.s));
        let (Some(r), Some(s)) = (r, s) else {
            return Err(GaslessFeeError::Locked);
        };
        Ok(self.account.signature(r, s))
    }

    /// `recordRequiredRelaySignature`: files the transfer's row before its execute is posted,
    /// under the signed message's hash with the execution's nonce and deadline, so a lost answer
    /// still leaves a pending row that guards a second signature (`ActivityLog::relay_pending`).
    /// Whether it is stored; when not, nothing may be posted.
    fn journal(
        &self,
        core: &WalletCore,
        quote: &CheckedQuote<'_>,
        offer: &GaslessFeeOffer,
        now_ms: u64,
    ) -> bool {
        let basis = &self.basis;
        let Some(nonce) = quote.nonce() else {
            return false;
        };
        let relay = RelayExecution {
            nonce: felt::canonical(&nonce),
            execute_before: quote.execute_before(),
        };
        let user_fee = quote.user_fee();
        let amount = U256::from_decimal(&user_fee.amount).unwrap_or(U256::ZERO);
        let record = PendingRecord {
            hash: quote.message_hash(),
            kind: PendingKind::Transfer {
                amount: basis.amount_text.clone(),
                recipient: basis.recipient,
                symbol: basis.symbol.clone(),
                token_address: basis.token_address,
            },
            summary: gasless_fee_summary(&basis.summary, &amount, offer.decimals, &user_fee.token),
            submitted_at: now_ms,
        };
        let stored = self
            .plan
            .record_row(core, record, Some(user_fee), Some(&relay));
        if stored {
            *self
                .journaled
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(quote.message_hash());
        }
        stored
    }

    /// The account's `signature` over the checked execution: the execute, once its row is
    /// journaled (`recordRequiredRelaySignature`); nothing is posted without it.
    pub(super) fn after_signed(
        &self,
        round: &mut SubmitRound<'static>,
        chosen: &GaslessFeeOffer,
        signature: &[Felt],
        now_ms: u64,
    ) -> Result<SubmitStep, GaslessFeeError> {
        let next = round.signed(signature, now_ms / 1000)?;
        let core = self.core()?;
        let quote = round.checked().ok_or(GaslessFeeError::NotFound)?;
        if !self.journal(&core, quote, chosen, now_ms) {
            return Err(GaslessFeeError::Refused {
                reason: INVOKE_NOT_JOURNALED.into(),
                retryable: false,
            });
        }
        Ok(next)
    }

    /// The relay accepted: the journal row takes the transaction hash, one row, never two.
    pub(super) fn settle(&self, core: &WalletCore, submission: &GaslessSubmission) {
        let journaled = self
            .journaled
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let Some(message_hash) = journaled else {
            return;
        };
        let account_id = &self.plan.account_id;
        with_log(&core.data_dir, |log| {
            log.settle_relay(account_id, &message_hash, &submission.transaction_hash)
        });
    }

    /// The device's part of a relay signature: the account's signature, or the step to hand the
    /// host (`Device`, `DeviceRetry`). A plan's failure ends the submission.
    pub(super) fn relay(
        &self,
        step: Result<RelayStep, CoreSendError>,
    ) -> Result<Result<Vec<Felt>, GaslessSubmitStep>, GaslessFeeError> {
        match step.map_err(|_| GaslessFeeError::NotFound)? {
            RelayStep::Signed(signature) => Ok(Ok(signature)),
            RelayStep::Step(SendStep::Device { apdu }) => {
                Ok(Err(GaslessSubmitStep::Device { apdu }))
            }
            RelayStep::Step(SendStep::DeviceRetry { reason }) => {
                Ok(Err(GaslessSubmitStep::DeviceRetry { reason }))
            }
            RelayStep::Step(SendStep::Failed { failure }) => Err(match failure.code {
                SendFailureCode::Locked => GaslessFeeError::Locked,
                SendFailureCode::DeviceRefused => GaslessFeeError::Refused {
                    reason: LEDGER_VERIFY_FAILED.into(),
                    retryable: false,
                },
                _ => GaslessError::from(GaslessCode::Unavailable).into(),
            }),
            RelayStep::Step(_) => Err(GaslessFeeError::NotFound),
        }
    }
}

#[uniffi::export]
impl GaslessFee {
    /// A Ledger account's device answer to the last `Device` step (`StepReply::Device` or
    /// `DeviceFailure`), or `RetryDevice` after `DeviceRetry`, at `now_ms` (the host's wall
    /// clock). Once the device's signature verified on the account's key, the execute to post.
    /// Any other reply, a lock, or a signature that does not verify ends the submission.
    pub fn advance_device(
        &self,
        reply: StepReply,
        now_ms: u64,
    ) -> Result<GaslessSubmitStep, GaslessFeeError> {
        // Only while the device's signature is outstanding: a stray reply leaves the rest be.
        if !self.plan.relaying() {
            return Err(GaslessFeeError::NotFound);
        }
        let step = {
            let mut stage = self.stage();
            let Stage::Submitting(round, chosen) = &mut *stage else {
                return Err(GaslessFeeError::NotFound);
            };
            let next =
                self.relay(self.plan.advance_relay(reply))
                    .and_then(|relayed| match relayed {
                        Ok(signature) => {
                            self.after_signed(round, chosen, &signature, now_ms).map(Ok)
                        }
                        Err(device) => Ok(Err(device)),
                    });
            match next {
                Ok(Ok(next)) => next,
                Ok(Err(device)) => return Ok(device),
                Err(error) => {
                    *stage = Stage::Spent;
                    return Err(error);
                }
            }
        };
        self.submit_step(step, now_ms)
    }
}
