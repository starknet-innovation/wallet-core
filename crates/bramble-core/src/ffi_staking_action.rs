//! A staking action on mobile (`wallet.stakingPrepare` and `wallet.stakingExecute`,
//! `background/staking/staking-action.ts`): the host names an action, a validator, a tracked
//! token and an amount, never a call. The core reads the pool at one pinned block, builds the
//! calls from that verified reading alone (`wallet_core::staking::prepare`, reviewed by
//! `Invoke::reviewed` with the pool the one pinned contract), and runs them through the plan
//! like the revoke (`send::revoke`): the review and fee affordability, one confirm, and a stake's
//! exact-amount STRK approval filed in the allowance log before its activity row
//! (`TransferPlan::record_pending`).
//!
//! The preparation is single use, as the extension's `stakingPreparations.claim` is: the first
//! `plan` that succeeds spends it, so a double tap or a host retry cannot get a second plan to
//! sign, and every later call is "Review this staking action again." A refused `plan` (an
//! unfinished or failed read, an unverified pool, an action the position does not allow) spends
//! nothing, as the extension's refusals come before it holds a preparation. The plan is bound to
//! the account, network and session it was prepared in, so a lock voids it. Mainnet only.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use wallet_core::history::events::ScanStep;
use wallet_core::send::call_decode::Known;
use wallet_core::send::plan::Kind;
use wallet_core::send::review_flags::ReviewContext;
use wallet_core::send::u256::U256;
use wallet_core::staking::calls::StakingAction;
use wallet_core::staking::prepare::{prepare, PrepareContext, PrepareInput};
use wallet_core::staking::read::StakingRead;
use wallet_core::staking::StakingRefusal;

use crate::allowances::{ranking, token_infos, trusted_tokens, with_allowance_log};
use crate::felt;
use crate::ffi_staking::{rewards_token, staking_network, tracked_tokens, StakingError};
use crate::portfolio::model::TokenConfig;
use crate::portfolio::tokens::TokenPreferences;
use crate::portfolio::wire::{RpcReply, RpcRequest};
use crate::send::plan::{TransferPlan, SEND_POLICY};
use crate::send::wallet_ext::{known_token_addresses, PlanAccount, Use};
use crate::staking::STAKING_POLICY;
use crate::wallet::WalletCore;

/// A planned staking action: the plan to run, and the sentence the person confirms
/// (`stakingReviewSentence`, naming the full validator address).
#[derive(uniffi::Record)]
pub struct StakingPlan {
    pub plan: Arc<TransferPlan>,
    pub review: String,
}

/// One staking action, bound to the account, network and session it was prepared in. Ask
/// `next_calls` until it is empty (the pinned pool read), then `plan`.
#[derive(uniffi::Object)]
pub struct StakingPreparation {
    core: Weak<WalletCore>,
    dir: PathBuf,
    acting: PlanAccount,
    input: PrepareInput,
    token: TokenConfig,
    /// The trusted tokens the review reads and a write evicts by.
    trusted: Vec<TokenConfig>,
    read: Mutex<StakingRead>,
    /// Set by the first `plan` that succeeds; held across `plan` so two calls cannot both pass.
    spent: Mutex<bool>,
}

impl StakingPreparation {
    fn read(&self) -> MutexGuard<'_, StakingRead> {
        self.read.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[uniffi::export]
impl StakingPreparation {
    /// The pool read to send now to an endpoint of the account's network; empty while waiting
    /// for replies or when done.
    pub fn next_calls(&self) -> Vec<RpcRequest> {
        self.read().next_calls()
    }

    /// Judges one reply. `Transport`: `restart` on the next endpoint.
    pub fn accept(&self, reply: RpcReply) -> ScanStep {
        self.read().accept(reply)
    }

    /// Discards the read so it replays on another endpoint.
    pub fn restart(&self) {
        self.read().restart();
    }

    /// The action's plan at Unix second `now_seconds`: its calls built from the finished read
    /// and reviewed as the extension's `verifyCalls` reviews them, with fee affordability over
    /// `fee_balance` (STRK base units the host last read; `None` is `Unknown`). Refused with the
    /// extension's sentence when the read is unfinished or failed, the pool unverified, or the
    /// action not allowed in the account's position; such a refusal spends nothing. Once a plan
    /// is returned, every later call is refused with "Review this staking action again." A lock
    /// since `prepare_staking` voids it.
    pub fn plan(
        &self,
        now_seconds: u64,
        fee_balance: Option<String>,
    ) -> Result<StakingPlan, StakingError> {
        let mut spent = self.spent.lock().unwrap_or_else(PoisonError::into_inner);
        if *spent {
            return Err(StakingRefusal::ReviewAgain.into());
        }
        let core = self.core.upgrade().ok_or(StakingError::Locked)?;
        let network = self.acting.network;
        let reading = self.read().finish();
        let infos = token_infos(&self.trusted);
        let trusted = core.trusted_recipients(network)?;
        let token_contracts: Vec<_> = known_token_addresses(network)
            .into_iter()
            .chain(self.trusted.iter().filter_map(|t| felt::parse(&t.address)))
            .collect();
        let standing = with_allowance_log(&self.dir, |log| log.list())?;
        let review = ReviewContext {
            known: Known {
                tokens: &infos,
                nft_contracts: &[],
            },
            trusted: &trusted,
            known_contracts: &[],
            token_contracts: &token_contracts,
            standing: &standing,
        };
        let prepared = prepare(
            &self.input,
            reading.as_ref(),
            &PrepareContext {
                account: &self.acting.account,
                staking: &STAKING_POLICY,
                token: &self.token,
                now: now_seconds,
                policy: &SEND_POLICY,
                review: &review,
            },
        )?;
        let mut invoke = prepared.invoke;
        if let Some(balance) = fee_balance.as_deref().and_then(U256::from_decimal) {
            invoke = invoke.with_fee_balance(balance);
        }
        *spent = true;
        Ok(StakingPlan {
            plan: TransferPlan::new(
                &core,
                self.acting.clone(),
                Kind::Invoke(Box::new(invoke)),
                ranking(&self.trusted),
            ),
            review: prepared.review,
        })
    }
}

#[uniffi::export]
impl WalletCore {
    /// Starts `action` for `account_id` (a vault record id that passes the signing gate) with
    /// validator `staker` (from the directory or pasted) in the tracked token `token_id` (a
    /// `TokenConfig::id` on the account's network). `amount` is the decimal amount entered, for
    /// stake and unstake only. Refuses another network, an untracked token and malformed input
    /// before any read. Requires an unlocked wallet; the session is recorded, so a lock voids the
    /// preparation and its plan.
    pub fn prepare_staking(
        self: Arc<Self>,
        account_id: String,
        action: StakingAction,
        staker: String,
        token_id: String,
        amount: Option<String>,
        tokens: Arc<TokenPreferences>,
    ) -> Result<Arc<StakingPreparation>, StakingError> {
        let input = PrepareInput::new(action, amount.as_deref(), &staker)?;
        let acting = self.plan_account_for(&account_id, Use::Sign)?;
        let network = acting.network;
        let staking = staking_network(network)?;
        let token = tracked_tokens(&tokens, network)
            .into_iter()
            .find(|token| token.id == token_id)
            .ok_or(StakingRefusal::OptionUnavailable)?;
        let read = input.read(staking, rewards_token(network)?, &acting.account, &token)?;
        Ok(Arc::new(StakingPreparation {
            core: Arc::downgrade(&self),
            dir: self.data_dir.clone(),
            read: Mutex::new(read),
            spent: Mutex::new(false),
            trusted: trusted_tokens(&tokens, network),
            acting,
            input,
            token,
        }))
    }
}
