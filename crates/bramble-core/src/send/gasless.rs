//! A transfer's network fee paid in another token (P14: `handleReviewTransfer`'s gasless offers
//! and `submitChosenTransfer`'s gasless branch, over `wallet_core::gasless`). The host never hands
//! over a call: the gasless path starts from a `TransferPlan` the person reviewed, and only when
//! that review found its STRK short of the fee (`Plan::gasless_basis`). The core describes every
//! paymaster request, the host posts it as described (after its delay) and hands back what came
//! back, and the core judges every reply: the offers it may show, then, for the one chosen, the
//! gateway's outside execution checked against that transfer and the policy's ceiling before
//! the core signs its hash, and the execute.
//!
//! Single use, as the extension's held review is: `choose` takes the transfer from its plan
//! (closing it, so its STRK path can never also be confirmed) and the offers with it; a second
//! choice, a choice the round did not offer, or a step out of turn is `NotFound` and changes
//! nothing. A lock since `prepare_transfer` voids it. The submitted transfer's row carries the
//! signed fee (`userFee`) and names it in its summary; a lost execute files no row, as the
//! extension's does not, since no hash came back. The round starts in `gasless_offers.rs`.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use wallet_core::activity::detail::gasless_fee_summary;
use wallet_core::gasless::{
    fee_reference, GaslessError, GaslessFeeOffer, GaslessPolicy, GaslessSession, GaslessSubmission,
    OfferRound, RoundStep, SubmitRound, SubmitStep,
};
use wallet_core::send::calls::format_token_amount;
use wallet_core::send::plan::{PendingKind, PendingRecord};
use wallet_core::send::plan_gasless::GaslessBasis;
use wallet_core::send::types::Account;
use wallet_core::send::u256::U256;
use wallet_core::Felt;

use super::plan::TransferPlan;
use crate::activity::with_log;
use crate::felt;
use crate::gasless::GASLESS_POLICY;
use crate::portfolio::prices::PriceView;
use crate::portfolio::registry::known_tokens;
use crate::portfolio::wire::{HttpOutcome, HttpRequest};
use crate::wallet::WalletCore;
use wallet_core::gasless::CheckedQuote;
use wallet_core::history::activity::RelayExecution;

pub(super) static POLICY: GaslessPolicy = GASLESS_POLICY;

/// `INVOKE_NOT_JOURNALED`: the execution could not be filed before posting, so it was not posted.
const INVOKE_NOT_JOURNALED: &str =
    "The transaction could not be recorded before sending. Nothing was submitted — try again.";

/// Why a gasless step was refused. Each is a fixed sentence (the extension's); none carries an
/// address, an amount or reply text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum GaslessFeeError {
    #[error("wallet locked")]
    Locked,
    /// Not the step outstanding, an offer this round did not make, or a choice already made.
    #[error("the fee changed since this request was reviewed")]
    NotFound,
    /// The route, the account, the paymaster or the check refused; nothing was submitted.
    /// `retryable` only for screening that stayed unavailable through every repeat.
    #[error("{reason}")]
    Refused { reason: String, retryable: bool },
    /// The execute's answer could not be read: the relay may hold the signed execution.
    #[error("{reason}")]
    Lost { reason: String },
}

impl From<GaslessError> for GaslessFeeError {
    fn from(error: GaslessError) -> Self {
        let reason = error.to_string();
        match error {
            GaslessError::Lost => Self::Lost { reason },
            GaslessError::Refused { retryable, .. } => Self::Refused { reason, retryable },
            GaslessError::ReviewedFeeExceeded => Self::Refused {
                reason,
                retryable: false,
            },
        }
    }
}

/// A fee token the person may choose, as the review shows it: the most it costs (`amount`, base
/// units; `display`, with its symbol) and the token.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct GaslessOffer {
    pub amount: String,
    pub decimals: u8,
    pub label: String,
    pub token_address: String,
    pub display: String,
}

/// One paymaster request: post `request` after `delay_ms`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct GaslessPost {
    pub request: HttpRequest,
    pub delay_ms: u64,
}

/// The offer round's next step.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum GaslessOfferStep {
    Post {
        post: GaslessPost,
    },
    /// The offers the review may show beside STRK; empty hides the choice.
    Done {
        offers: Vec<GaslessOffer>,
    },
}

/// The submission's next step.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum GaslessSubmitStep {
    Post {
        post: GaslessPost,
    },
    /// Screening was unavailable for the execute: wait `delay_ms`, then call `resume`, which
    /// posts again only if the wallet is still unlocked (`postGasless` checks after its pause).
    Wait {
        delay_ms: u64,
    },
    /// The relay accepted it: its hash, and its page on the network's explorer.
    Submitted {
        transaction_hash: String,
        explorer_url: Option<String>,
    },
}

pub(super) enum Stage {
    Offering(Box<OfferRound<'static>>),
    Offered(Vec<GaslessFeeOffer>),
    Submitting(Box<SubmitRound<'static>>, GaslessFeeOffer),
    /// An execute repeat held until its pause is over.
    Paused(Box<SubmitRound<'static>>, GaslessFeeOffer, HttpRequest),
    Spent,
}

/// The gasless choice for one reviewed transfer.
#[derive(uniffi::Object)]
pub struct GaslessFee {
    pub(super) plan: Arc<TransferPlan>,
    pub(super) account: Account,
    pub(super) session: GaslessSession<'static>,
    pub(super) basis: GaslessBasis,
    pub(super) stage: Mutex<Stage>,
    /// The signed message whose journal row the relay's answer settles.
    pub(super) journaled: Mutex<Option<Felt>>,
}

/// A started offer round: the object, and the first request to post.
#[derive(uniffi::Record)]
pub struct GaslessOffers {
    pub fee: Arc<GaslessFee>,
    pub step: GaslessOfferStep,
}

fn offer_view(offer: &GaslessFeeOffer) -> GaslessOffer {
    let shown = U256::from_decimal(&offer.amount).map_or_else(
        || offer.amount.clone(),
        |raw| format_token_amount(&raw, offer.decimals),
    );
    GaslessOffer {
        amount: offer.amount.clone(),
        decimals: offer.decimals,
        label: offer.label.clone(),
        token_address: offer.token_address.clone(),
        display: format!("{shown} {}", offer.label),
    }
}

pub(super) fn offer_step(step: RoundStep) -> GaslessOfferStep {
    match step {
        RoundStep::Post { request, delay_ms } => GaslessOfferStep::Post {
            post: GaslessPost { request, delay_ms },
        },
        RoundStep::Done(offers) => GaslessOfferStep::Done {
            offers: offers.iter().map(offer_view).collect(),
        },
    }
}

/// `STRK_TOKEN_ADDRESS`: STRK in Bramble's bundled catalog for `network`.
pub(super) fn strk_address(network: crate::NetworkId) -> Option<Felt> {
    crate::networks::info(network)
        .tokens
        .iter()
        .find(|token| token.symbol == "STRK")
        .and_then(|token| felt::parse(&token.address))
}

/// `strk20FeeReference` for `token` on the plan's network: the STRK fee at the wallet's prices.
pub(super) fn reference(
    plan: &TransferPlan,
    basis: &GaslessBasis,
    prices: &PriceView,
    token: Felt,
    now_ms: u64,
) -> Option<U256> {
    let decimals = |address: &Felt| {
        known_tokens(plan.network)
            .into_iter()
            .find(|known| felt::parse(&known.address).as_ref() == Some(address))
            .map(|known| known.decimals)
    };
    let strk = strk_address(plan.network)?;
    let strk = (strk, decimals(&strk)?);
    fee_reference(
        &POLICY,
        prices,
        strk,
        (token, decimals(&token)?),
        basis.strk_fee,
        now_ms,
    )
}

impl GaslessFee {
    fn stage(&self) -> MutexGuard<'_, Stage> {
        self.stage.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn core(&self) -> Result<Arc<WalletCore>, GaslessFeeError> {
        let core = self.plan.core.upgrade().ok_or(GaslessFeeError::Locked)?;
        match core.session_generation() {
            Some(generation) if generation == self.plan.generation => Ok(core),
            _ => Err(GaslessFeeError::Locked),
        }
    }

    /// Signs the checked hash for the plan's record, still in its session, and serialises the
    /// account's signature (`serializeAccountSignature`).
    fn sign(&self, core: &WalletCore, hash: &Felt) -> Result<Vec<Felt>, GaslessFeeError> {
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

    /// The relay accepted: the journal row takes the transaction hash, one row, never two.
    fn settle(&self, core: &WalletCore, submission: &GaslessSubmission) {
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
}

#[uniffi::export]
impl GaslessFee {
    /// Judges the outcome of the offer round's last `Post`.
    pub fn accept_offer_reply(
        &self,
        outcome: HttpOutcome,
    ) -> Result<GaslessOfferStep, GaslessFeeError> {
        let mut stage = self.stage();
        let Stage::Offering(round) = &mut *stage else {
            return Err(GaslessFeeError::NotFound);
        };
        let step = round.accept(&outcome);
        if let RoundStep::Done(offers) = &step {
            *stage = Stage::Offered(offers.clone());
        }
        Ok(offer_step(step))
    }

    /// The person's choice, at `now_ms` (the host's wall clock): one of the offers the round
    /// made, exactly, priced again by `prices` (the wallet's own fresh USD answer for STRK and the
    /// token). Takes the transfer from its plan, which closes, and returns the build to post.
    /// `NotFound` and nothing changed for an offer not made or a second choice; a refusal (no
    /// reference: "This token's network fee could not be checked.") ends the choice.
    pub fn choose(
        &self,
        offer: GaslessOffer,
        prices: Option<PriceView>,
        now_ms: u64,
    ) -> Result<GaslessSubmitStep, GaslessFeeError> {
        self.core()?;
        let mut stage = self.stage();
        let Stage::Offered(offers) = &*stage else {
            return Err(GaslessFeeError::NotFound);
        };
        let chosen = offers
            .iter()
            .find(|made| offer_view(made) == offer)
            .cloned()
            .ok_or(GaslessFeeError::NotFound)?;
        // The offers stand only for the review they were made for: a re-review (a new estimate)
        // replaces it, as the extension replaces its held review.
        if self.plan.gasless_basis().as_ref() != Some(&self.basis) {
            return Err(GaslessFeeError::NotFound);
        }
        let taken = self.plan.take_for_gasless();
        *stage = Stage::Spent;
        let basis = taken
            .filter(|taken| *taken == self.basis)
            .ok_or(GaslessFeeError::NotFound)?;
        let token = felt::parse(&chosen.token_address).ok_or(GaslessFeeError::NotFound)?;
        let reference =
            prices.and_then(|prices| reference(&self.plan, &basis, &prices, token, now_ms));
        let (round, step) =
            SubmitRound::start(self.session.clone(), &chosen, reference, now_ms / 1000)?;
        *stage = Stage::Submitting(Box::new(round), chosen);
        drop(stage);
        self.submit_step(step, now_ms)
    }

    /// After a `Wait`: the execute posted again, or `Locked` (and the submission over) when the
    /// wallet locked during the pause.
    pub fn resume(&self) -> Result<GaslessSubmitStep, GaslessFeeError> {
        let mut stage = self.stage();
        if !matches!(*stage, Stage::Paused(..)) {
            return Err(GaslessFeeError::NotFound);
        }
        let Stage::Paused(round, chosen, request) = std::mem::replace(&mut *stage, Stage::Spent)
        else {
            return Err(GaslessFeeError::NotFound);
        };
        self.core()?;
        *stage = Stage::Submitting(round, chosen);
        Ok(GaslessSubmitStep::Post {
            post: GaslessPost {
                request,
                delay_ms: 0,
            },
        })
    }

    /// Judges the outcome of the submission's last `Post`, at `now_ms`. A checked build is
    /// signed here, in the plan's session, and its execute returned; a submitted one files its
    /// row. Every refusal ends the submission.
    pub fn accept_submit_reply(
        &self,
        outcome: HttpOutcome,
        now_ms: u64,
    ) -> Result<GaslessSubmitStep, GaslessFeeError> {
        let step = {
            let mut stage = self.stage();
            let Stage::Submitting(round, _) = &mut *stage else {
                return Err(GaslessFeeError::NotFound);
            };
            match round.accept(&outcome) {
                Ok(step) => step,
                Err(error) => {
                    *stage = Stage::Spent;
                    return Err(error.into());
                }
            }
        };
        self.submit_step(step, now_ms)
    }
}

impl GaslessFee {
    // Carries a submission step through the host's half the core owns: the session check before
    // signing and before every execute, the signature itself, and the row.
    fn submit_step(
        &self,
        mut step: SubmitStep,
        now_ms: u64,
    ) -> Result<GaslessSubmitStep, GaslessFeeError> {
        let mut stage = self.stage();
        let Stage::Submitting(round, chosen) = &mut *stage else {
            return Err(GaslessFeeError::NotFound);
        };
        let outcome = loop {
            step = match step {
                SubmitStep::Post {
                    request,
                    delay_ms,
                    executes,
                } => {
                    if executes && self.core().is_err() {
                        break Err(GaslessFeeError::Locked);
                    }
                    if executes && delay_ms > 0 {
                        if let Stage::Submitting(round, chosen) =
                            std::mem::replace(&mut *stage, Stage::Spent)
                        {
                            *stage = Stage::Paused(round, chosen, request);
                        }
                        return Ok(GaslessSubmitStep::Wait { delay_ms });
                    }
                    return Ok(GaslessSubmitStep::Post {
                        post: GaslessPost { request, delay_ms },
                    });
                }
                SubmitStep::Sign { hash } => {
                    let signed = self.core().and_then(|core| self.sign(&core, &hash));
                    let next = signed.and_then(|signature| {
                        round
                            .signed(&signature, now_ms / 1000)
                            .map_err(GaslessFeeError::from)
                    });
                    let journaled = next.and_then(|next| {
                        let core = self.core()?;
                        let quote = round.checked().ok_or(GaslessFeeError::NotFound)?;
                        if !self.journal(&core, quote, chosen, now_ms) {
                            return Err(GaslessFeeError::Refused {
                                reason: INVOKE_NOT_JOURNALED.into(),
                                retryable: false,
                            });
                        }
                        Ok(next)
                    });
                    match journaled {
                        Ok(next) => next,
                        Err(error) => break Err(error),
                    }
                }
                SubmitStep::Submitted(submission) => {
                    // Settled even if the wallet locked meanwhile: the relay has it either way.
                    if let Some(core) = self.plan.core.upgrade() {
                        self.settle(&core, &submission);
                    }
                    let hash = felt::canonical(&submission.transaction_hash);
                    break Ok(GaslessSubmitStep::Submitted {
                        explorer_url: crate::history::explorer::explorer_transaction_url(
                            self.plan.network,
                            hash.clone(),
                        ),
                        transaction_hash: hash,
                    });
                }
            };
        };
        *stage = Stage::Spent;
        outcome
    }
}
