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
//! extension's does not, since no hash came back.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use wallet_core::activity::detail::gasless_fee_summary;
use wallet_core::gasless::{
    fee_reference, GaslessAccount, GaslessError, GaslessFeeOffer, GaslessHeldToken, GaslessPolicy,
    GaslessSession, GaslessSubmission, OfferRound, RoundStep, SubmitRound, SubmitStep,
};
use wallet_core::send::calls::format_token_amount;
use wallet_core::send::plan::{PendingKind, PendingRecord};
use wallet_core::send::plan_gasless::GaslessBasis;
use wallet_core::send::types::Account;
use wallet_core::send::u256::U256;
use wallet_core::Felt;

use super::plan::TransferPlan;
use super::wallet_ext::Use;
use crate::felt;
use crate::gasless::{gasless_v2_classes, GASLESS_POLICY};
use crate::portfolio::prices::PriceView;
use crate::portfolio::registry::known_tokens;
use crate::portfolio::wire::{HttpOutcome, HttpRequest};
use crate::wallet::WalletCore;
use crate::wallet_accounts::signing_record;

static POLICY: GaslessPolicy = GASLESS_POLICY;

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
    /// The relay accepted it: its hash, and its page on the network's explorer.
    Submitted {
        transaction_hash: String,
        explorer_url: Option<String>,
    },
}

enum Stage {
    Offering(Box<OfferRound<'static>>),
    Offered(Vec<GaslessFeeOffer>),
    Submitting(Box<SubmitRound<'static>>, GaslessFeeOffer),
    Spent,
}

/// The gasless choice for one reviewed transfer.
#[derive(uniffi::Object)]
pub struct GaslessFee {
    plan: Arc<TransferPlan>,
    account: Account,
    session: GaslessSession<'static>,
    basis: GaslessBasis,
    stage: Mutex<Stage>,
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

fn offer_step(step: RoundStep) -> GaslessOfferStep {
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
fn strk_address(network: crate::NetworkId) -> Option<Felt> {
    crate::networks::info(network)
        .tokens
        .iter()
        .find(|token| token.symbol == "STRK")
        .and_then(|token| felt::parse(&token.address))
}

/// `strk20FeeReference` for `token` on the plan's network: the STRK fee at the wallet's prices.
fn reference(
    plan: &TransferPlan,
    basis: &GaslessBasis,
    prices: &PriceView,
    token: Felt,
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

    /// Files the submitted transfer's row with the fee it signed.
    fn record(
        &self,
        core: &WalletCore,
        offer: &GaslessFeeOffer,
        submission: &GaslessSubmission,
        now_ms: u64,
    ) {
        let basis = &self.basis;
        let amount = U256::from_decimal(&submission.user_fee.amount).unwrap_or(U256::ZERO);
        let record = PendingRecord {
            hash: submission.transaction_hash,
            kind: PendingKind::Transfer {
                amount: basis.amount_text.clone(),
                recipient: basis.recipient,
                symbol: basis.symbol.clone(),
                token_address: basis.token_address,
            },
            summary: gasless_fee_summary(
                &basis.summary,
                &amount,
                offer.decimals,
                &submission.user_fee.token,
            ),
            submitted_at: now_ms,
        };
        self.plan
            .record_row(core, record, Some(&submission.user_fee));
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
        if self.plan.take_for_gasless().is_none() {
            return Err(GaslessFeeError::NotFound);
        }
        *stage = Stage::Spent;
        let token = felt::parse(&chosen.token_address).ok_or(GaslessFeeError::NotFound)?;
        let reference =
            prices.and_then(|prices| reference(&self.plan, &self.basis, &prices, token));
        let (round, step) =
            SubmitRound::start(self.session.clone(), &chosen, reference, now_ms / 1000)?;
        *stage = Stage::Submitting(Box::new(round), chosen);
        drop(stage);
        self.submit_step(step, now_ms)
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
                    match next {
                        Ok(next) => next,
                        Err(error) => break Err(error),
                    }
                }
                SubmitStep::Submitted(submission) => {
                    // Filed even if the wallet locked meanwhile: the relay has it either way.
                    if let Some(core) = self.plan.core.upgrade() {
                        self.record(&core, chosen, &submission, now_ms);
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

#[uniffi::export]
impl TransferPlan {
    /// The gasless choice for this reviewed transfer (`reviewGaslessOffers`): `None` unless the
    /// review found its STRK short of an FRI fee, the network's route is on, and the account's
    /// class signs SNIP-9 v2 and is deployed. `prices` is the wallet's own fresh USD answer for
    /// STRK and its curated tokens (`PriceBook::plan`); `balances` the host's last read of those
    /// tokens (base units; a token missing or `None` is unread). The offers are curated tokens
    /// other than STRK, with Bramble's labels and decimals, never the paymaster's. `now_ms` is
    /// the host's wall clock.
    pub fn gasless_offers(
        self: Arc<Self>,
        prices: Option<PriceView>,
        balances: Vec<GaslessBalance>,
        now_ms: u64,
    ) -> Option<GaslessOffers> {
        let core = self.core.upgrade()?;
        let basis = self.gasless_basis()?;
        let acting = core.plan_account_for(&self.account_id, Use::Sign).ok()?;
        let (family, class_hash) = core
            .with_session(false, |session| {
                let record = signing_record(&session.payload, &self.account_id)?;
                Ok((record.family(), record.class_felt()))
            })
            .ok()?;
        let account = GaslessAccount {
            network: self.network,
            address: acting.account.address,
            family,
            class_hash,
            deployed: basis.deployed,
            deployment: None,
        };
        let calls = [basis.call.clone()];
        let session =
            GaslessSession::open(&POLICY, &account, &calls, &gasless_v2_classes()).ok()?;
        let strk = strk_address(self.network)?;
        let held = known_tokens(self.network)
            .into_iter()
            .filter_map(|known| {
                let address = felt::parse(&known.address).filter(|address| *address != strk)?;
                let balance = balances
                    .iter()
                    .find(|b| felt::parse(&b.token_address) == Some(address))
                    .and_then(|b| b.balance.as_deref().and_then(U256::from_decimal));
                let reference = prices
                    .as_ref()
                    .and_then(|p| reference(&self, &basis, p, address));
                let token = GaslessHeldToken {
                    address,
                    balance,
                    decimals: known.decimals,
                    label: known.symbol,
                };
                Some((token, reference))
            })
            .collect();
        let (round, step) = OfferRound::start(session.clone(), held, strk, now_ms / 1000);
        let stage = match &step {
            RoundStep::Done(offers) => Stage::Offered(offers.clone()),
            RoundStep::Post { .. } => Stage::Offering(Box::new(round)),
        };
        let fee = Arc::new(GaslessFee {
            plan: self.clone(),
            account: acting.account,
            session,
            basis,
            stage: Mutex::new(stage),
        });
        Some(GaslessOffers {
            fee,
            step: offer_step(step),
        })
    }
}

/// The host's last read of one curated token's balance (`getTokenBalances`).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct GaslessBalance {
    pub token_address: String,
    /// Base units as decimal digits; `None` when the read failed.
    pub balance: Option<String>,
}
