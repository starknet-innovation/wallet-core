//! The AVNU token swap (`handleSwapOps` in `background/transactions/swap-ops.ts`), the second
//! export that builds a call batch. The host names an account, two tracked token ids and an
//! amount; the core describes the quote and build requests, the host performs them as described
//! (`portfolio::wire`) and hands back what came back, and the core judges every reply
//! (`wallet_core::swap`): the quote pinned to the pair and amount, the build to one exact approval
//! of the sell token to the router `SWAP_POLICY` pins for the account's network plus one
//! `multi_route_swap` on it that pays this account at least the reviewed floor. The judged batch
//! is reviewed (`Invoke::reviewed`) and runs through the plan like a transfer; its approval is
//! filed in the allowance log before its row (`TransferPlan::record_pending`). The host never
//! hands over a call.
//!
//! One `TokenSwap` is one quote, one build and one plan, in that order, each step once: a step out
//! of turn, or a second plan of the same preparation, is `NotFound` (the extension's one-time
//! `claim`) and changes nothing; a refused reply ends the swap. A new price is a new `TokenSwap`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use wallet_core::send::call_decode::Known;
use wallet_core::send::plan::Kind;
use wallet_core::send::review_flags::ReviewContext;
use wallet_core::send::u256::U256;
use wallet_core::swap::{PendingBuild, PendingQuote, SwapPreparation, SwapQuote, SwapRefusal};

use super::plan::{TransferPlan, SEND_POLICY};
use super::types::{RiskFlag, SendError};
use super::wallet_ext::{known_token_addresses, PlanAccount, Use};
use crate::allowances::{ranking, token_infos, trusted_tokens, with_allowance_log};
use crate::felt;
use crate::portfolio::model::TokenConfig;
use crate::portfolio::prices::PriceView;
use crate::portfolio::tokens::TokenPreferences;
use crate::portfolio::wire::{HttpOutcome, HttpRequest};
use crate::swap::SWAP_POLICY;
use crate::wallet::WalletCore;

/// Why a swap step was refused. Each is a fixed sentence (the extension's); none carries an
/// address, an amount or reply text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum SwapError {
    #[error("wallet is locked")]
    Locked,
    #[error("unknown account")]
    UnknownAccount,
    /// The allowance log, contacts or multisig file could not be read.
    #[error("a wallet file could not be read")]
    Storage,
    /// Not the step outstanding, or the preparation was already planned or refused.
    #[error("swap preparation not found")]
    NotFound,
    /// The quote, the build or the batch review refused; `reason` is its fixed sentence.
    #[error("{reason}")]
    Refused { reason: String },
}

impl From<SwapRefusal> for SwapError {
    fn from(refusal: SwapRefusal) -> Self {
        Self::Refused {
            reason: refusal.to_string(),
        }
    }
}

impl From<SendError> for SwapError {
    fn from(error: SendError) -> Self {
        match error {
            SendError::Locked => Self::Locked,
            SendError::UnknownAccount => Self::UnknownAccount,
            // `trusted_recipients` fails closed on an unreadable contacts or multisig file.
            _ => Self::Storage,
        }
    }
}

impl From<wallet_core::Error> for SwapError {
    fn from(_: wallet_core::Error) -> Self {
        Self::Storage
    }
}

/// A judged quote, as the swap screen shows it (`swapQuoteResponse`'s `swapQuote`).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SwapQuoteView {
    /// The amount sold, as typed (trimmed).
    pub sell_amount: String,
    /// The quoted amount bought, in the buy token's decimals.
    pub buy_amount: String,
    /// AVNU's own figures, for display only.
    pub sell_amount_in_usd: f64,
    pub buy_amount_in_usd: f64,
    pub gas_fees_in_usd: f64,
    /// The percent the buy side is worth more (+) or less (-) than the sell side, from the
    /// wallet's own price feed. `None` means the price is AVNU's word alone: show it as
    /// unverified, never as fair.
    pub value_delta_percent: Option<f64>,
}

/// A judged build, frozen for review (`SwapPreparationReview`); the fee is the plan's review.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SwapReview {
    /// One line per call, in call order; the route's states its floor and its payee.
    pub summaries: Vec<String>,
    /// The minimum the signed route enforces, in the buy token's decimals.
    pub min_buy_amount: String,
    pub flags: Vec<RiskFlag>,
}

enum Stage {
    Quoting(Box<PendingQuote>),
    Quoted(Box<SwapQuote>),
    Building(Box<PendingBuild>),
    Prepared(Box<SwapPreparation>),
    Spent,
}

/// One swap, bound to the account, network and session it was prepared in: `quote_request`,
/// `accept_quote`, `build_request`, `accept_build`, then `plan`.
#[derive(uniffi::Object)]
pub struct TokenSwap {
    core: Weak<WalletCore>,
    dir: PathBuf,
    acting: PlanAccount,
    /// The trusted tokens on the account's network: the review's known tokens and the ranking
    /// the approval is filed by.
    tokens: Vec<TokenConfig>,
    quote_request: HttpRequest,
    stage: Mutex<Stage>,
}

impl TokenSwap {
    fn stage(&self) -> MutexGuard<'_, Stage> {
        self.stage.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Takes the stage `pick` accepts, leaving `Spent`; any other stage is left as it was.
    fn take<T>(&self, pick: impl FnOnce(Stage) -> Result<T, Stage>) -> Result<T, SwapError> {
        let mut stage = self.stage();
        pick(std::mem::replace(&mut *stage, Stage::Spent)).map_err(|other| {
            *stage = other;
            SwapError::NotFound
        })
    }
}

#[uniffi::export]
impl TokenSwap {
    /// The quote request (`getQuote`'s GET) to perform as described.
    pub fn quote_request(&self) -> HttpRequest {
        self.quote_request.clone()
    }

    /// Judges the quote reply: the best quote, for exactly the pair and amount asked. `prices`
    /// is the wallet's own feed for the two tokens (`PriceBook::plan`), for the value check. A
    /// refusal ends the swap.
    pub fn accept_quote(
        &self,
        outcome: HttpOutcome,
        prices: Option<PriceView>,
    ) -> Result<SwapQuoteView, SwapError> {
        let pending = self.take(|stage| match stage {
            Stage::Quoting(pending) => Ok(*pending),
            other => Err(other),
        })?;
        let quote = pending.judge(&outcome)?;
        let view = SwapQuoteView {
            sell_amount: quote.sell_amount_text().to_string(),
            buy_amount: quote.buy_amount_text(),
            sell_amount_in_usd: quote.sell_amount_in_usd,
            buy_amount_in_usd: quote.buy_amount_in_usd,
            gas_fees_in_usd: quote.gas_fees_in_usd,
            value_delta_percent: quote.value_delta_percent(prices.as_ref()),
        };
        *self.stage() = Stage::Quoted(Box::new(quote));
        Ok(view)
    }

    /// Confirms the quote: the build request (`buildCalls`' POST) at `slippage` (0.5% when
    /// `None`, at most 5%), against the floor the quote implies. A refused slippage keeps the
    /// quote.
    pub fn build_request(&self, slippage: Option<f64>) -> Result<HttpRequest, SwapError> {
        let mut stage = self.stage();
        let Stage::Quoted(quote) = &*stage else {
            return Err(SwapError::NotFound);
        };
        let (pending, request) = quote.prepare(&self.acting.account, slippage)?;
        *stage = Stage::Building(Box::new(pending));
        Ok(request)
    }

    /// Judges the build reply (the build policy, then the batch review with everything the
    /// wallet knows on the network) and freezes it for review. A refusal ends the swap.
    pub fn accept_build(&self, outcome: HttpOutcome) -> Result<SwapReview, SwapError> {
        let core = self.core.upgrade().ok_or(SwapError::Locked)?;
        let network = self.acting.network;
        let infos = token_infos(&self.tokens);
        let trusted = core.trusted_recipients(network)?;
        let token_contracts: Vec<_> = known_token_addresses(network)
            .into_iter()
            .chain(self.tokens.iter().filter_map(|t| felt::parse(&t.address)))
            .collect();
        let standing = with_allowance_log(&self.dir, |log| log.list())?;
        let context = ReviewContext {
            known: Known {
                tokens: &infos,
                nft_contracts: &[],
            },
            trusted: &trusted,
            // `PendingBuild::judge` pins the router alone.
            known_contracts: &[],
            token_contracts: &token_contracts,
            standing: &standing,
        };
        let pending = self.take(|stage| match stage {
            Stage::Building(pending) => Ok(*pending),
            other => Err(other),
        })?;
        let prepared = pending.judge(&outcome, &context, &SEND_POLICY)?;
        let review = SwapReview {
            summaries: prepared.summaries().to_vec(),
            min_buy_amount: prepared.min_buy_amount_text().to_string(),
            flags: prepared.flags().into_iter().map(RiskFlag::from).collect(),
        };
        *self.stage() = Stage::Prepared(Box::new(prepared));
        Ok(review)
    }

    /// The swap's plan, once: the reviewed batch with fee affordability over `fee_balance` (STRK
    /// base units the host last read for the account; `None` is `Unknown`). Its card is
    /// `TransferPlan::current_review`, `reviewed_calls` and `risk_flags`. A lock since
    /// `prepare_swap` voids the plan.
    pub fn plan(&self, fee_balance: Option<String>) -> Result<Arc<TransferPlan>, SwapError> {
        let core = self.core.upgrade().ok_or(SwapError::Locked)?;
        let prepared = self.take(|stage| match stage {
            Stage::Prepared(prepared) => Ok(*prepared),
            other => Err(other),
        })?;
        let mut invoke = prepared.into_invoke(&self.acting.account)?;
        if let Some(balance) = fee_balance.as_deref().and_then(U256::from_decimal) {
            invoke = invoke.with_fee_balance(balance);
        }
        Ok(TransferPlan::new(
            &core,
            self.acting.clone(),
            Kind::Invoke(Box::new(invoke)),
            ranking(&self.tokens),
        ))
    }
}

#[uniffi::export]
impl WalletCore {
    /// Starts a swap of `amount` (decimal text as typed) of the tracked token `sell_token_id` for
    /// `buy_token_id` (`TokenConfig::id`s on the account's network, resolved here, never taken as
    /// addresses) from `account_id` (a vault record id that passes the signing gate). Refuses a
    /// network AVNU is not pinned on, an id not tracked there, the same token twice and an amount
    /// the token cannot carry, before any request. Requires an unlocked wallet; the session is
    /// recorded, so a lock voids the swap's plan.
    pub fn prepare_swap(
        self: Arc<Self>,
        account_id: String,
        sell_token_id: String,
        buy_token_id: String,
        amount: String,
        tokens: Arc<TokenPreferences>,
    ) -> Result<Arc<TokenSwap>, SwapError> {
        let acting = self.plan_account_for(&account_id, Use::Sign)?;
        let tracked = tokens.tracked(acting.network);
        let by_id = |id: &str| {
            tracked
                .iter()
                .find(|tracked| tracked.token.id == id)
                .and_then(|tracked| token_infos(std::slice::from_ref(&tracked.token)).pop())
                .ok_or(SwapRefusal::TokenUnavailable)
        };
        let (sell, buy) = (by_id(&sell_token_id)?, by_id(&buy_token_id)?);
        let (pending, quote_request) =
            PendingQuote::new(&acting.account, &sell, &buy, &amount, &SWAP_POLICY)?;
        Ok(Arc::new(TokenSwap {
            core: Arc::downgrade(&self),
            dir: self.data_dir.clone(),
            tokens: trusted_tokens(&tokens, acting.network),
            acting,
            quote_request,
            stage: Mutex::new(Stage::Quoting(Box::new(pending))),
        }))
    }
}
