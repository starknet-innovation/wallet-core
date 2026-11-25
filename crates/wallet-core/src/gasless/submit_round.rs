//! The reviewed submission (`submitReviewedGaslessInvoke`), sans-IO: the build for the token the
//! person chose, its check, the one signature over the checked hash, and the execute. The host
//! posts each [`SubmitStep::Post`] after its delay, signs at [`SubmitStep::Sign`], and checks
//! the session is still unlocked before signing, before the execute and before an execute's
//! repeat (a lock refuses, "wallet locked", and nothing more is posted). Every step is taken once
//! and in order; a step out of turn is refused and ends the submission.

use super::rpc::{Exchange, Exchanged};
use super::submit::{CheckedQuote, PendingSubmit};
use super::{read_transaction_hash, GaslessCode, GaslessError, GaslessFeeOffer, GaslessSubmission};
use crate::portfolio::wire::{HttpOutcome, HttpRequest};
use crate::send::u256::U256;
use crate::Felt;

use super::request::GaslessSession;

/// What the host does next.
#[derive(Debug, Clone, PartialEq)]
pub enum SubmitStep {
    /// Wait `delay_ms`, then post `request` and pass its outcome to [`SubmitRound::accept`].
    /// `executes`: this is `paymaster_executeTransaction`, so a repeat (`delay_ms > 0`) is posted
    /// only while the session is still unlocked, and a lost answer may have left the signed
    /// execution with the relay.
    Post {
        request: HttpRequest,
        delay_ms: u64,
        executes: bool,
    },
    /// Sign `hash` (the checked outside execution's SNIP-12 hash) with the account's key and pass
    /// the account's signature to [`SubmitRound::signed`].
    Sign { hash: Felt },
    /// The relay accepted the execution.
    Submitted(GaslessSubmission),
}

enum Stage<'p> {
    Building(Box<PendingSubmit>, Exchange),
    Signing(Box<CheckedQuote<'p>>),
    Executing(Box<CheckedQuote<'p>>, Exchange),
    Over,
}

/// One submission of a reviewed gasless fee.
pub struct SubmitRound<'p> {
    session: GaslessSession<'p>,
    stage: Stage<'p>,
}

impl<'p> SubmitRound<'p> {
    /// `prepareSubmission` up to its build, at `now`: `reviewed` is the offer the person chose
    /// and `reference` the fee token's independent reference (`None` refuses as `Unverifiable`
    /// before anything is posted).
    pub fn start(
        session: GaslessSession<'p>,
        reviewed: &GaslessFeeOffer,
        reference: Option<U256>,
        now: u64,
    ) -> Result<(Self, SubmitStep), GaslessError> {
        let (pending, request) = session.submit_build(reviewed, reference, now)?;
        let exchange = Exchange::new(request, false);
        let step = post(&exchange, 0);
        let stage = Stage::Building(Box::new(pending), exchange);
        Ok((Self { session, stage }, step))
    }

    /// The checked outside execution, once the build passed: what the host may persist (its
    /// nonce) before the execute.
    pub fn checked(&self) -> Option<&CheckedQuote<'p>> {
        match &self.stage {
            Stage::Signing(quote) | Stage::Executing(quote, _) => Some(quote),
            _ => None,
        }
    }

    /// Judges the outcome of the last `Post`: the build's (checked against the reviewed calls,
    /// the ceiling and the reviewed amount, then `Sign`) or the execute's (`Submitted`; a reply
    /// without a usable hash is `Lost`). A refusal ends the submission.
    pub fn accept(&mut self, outcome: &HttpOutcome) -> Result<SubmitStep, GaslessError> {
        let policy = self.session.policy;
        match std::mem::replace(&mut self.stage, Stage::Over) {
            Stage::Building(pending, mut exchange) => match exchange.judge(outcome, policy) {
                Exchanged::Repeat { delay_ms } => {
                    let step = post(&exchange, delay_ms);
                    self.stage = Stage::Building(pending, exchange);
                    Ok(step)
                }
                Exchanged::Settled(result) => {
                    let quote = pending.judge(&self.session, &result?)?;
                    let hash = quote.message_hash();
                    self.stage = Stage::Signing(Box::new(quote));
                    Ok(SubmitStep::Sign { hash })
                }
            },
            Stage::Executing(quote, mut exchange) => match exchange.judge(outcome, policy) {
                Exchanged::Repeat { delay_ms } => {
                    let step = post(&exchange, delay_ms);
                    self.stage = Stage::Executing(quote, exchange);
                    Ok(step)
                }
                Exchanged::Settled(result) => {
                    let transaction_hash =
                        read_transaction_hash(&result?).ok_or(GaslessError::Lost)?;
                    Ok(SubmitStep::Submitted(GaslessSubmission {
                        transaction_hash,
                        user_fee: quote.user_fee().clone(),
                    }))
                }
            },
            Stage::Signing(_) | Stage::Over => Err(GaslessCode::Unavailable.into()),
        }
    }

    /// The account's `signature` over the hash `Sign` named, with the clock read after signing
    /// (`readNowSeconds`): the execute, once the window still has the policy's floor left.
    pub fn signed(&mut self, signature: &[Felt], now: u64) -> Result<SubmitStep, GaslessError> {
        let Stage::Signing(quote) = std::mem::replace(&mut self.stage, Stage::Over) else {
            return Err(GaslessCode::Unavailable.into());
        };
        let exchange = Exchange::new(quote.execute_request(signature, now)?, true);
        let step = post(&exchange, 0);
        self.stage = Stage::Executing(quote, exchange);
        Ok(step)
    }
}

fn post(exchange: &Exchange, delay_ms: u64) -> SubmitStep {
    SubmitStep::Post {
        request: exchange.request().clone(),
        delay_ms,
        executes: exchange.executes(),
    }
}

#[cfg(test)]
#[path = "submit_round_tests.rs"]
mod tests;
