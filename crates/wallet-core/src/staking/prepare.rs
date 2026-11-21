//! A staking action, prepared then confirmed (`staking-action.ts`, `staking-inputs.ts`,
//! `staking-preparations.ts`).
//!
//! Prepare: the staker is the person's pick (from the directory or pasted) and only a candidate;
//! the token is a tracked token the staking contract accepts; the pool is verified on chain at
//! one pinned block (`read`). The calls are built from those facts alone, reviewed as every
//! batch is (`send::invoke::Invoke::new`, which refuses a poisoned payee), and frozen with the
//! sentence the person confirms. Confirm: the preparation is claimed once and must still be for
//! the active account and network; the host then hands its `invoke` to `send::plan::Plan`, which
//! estimates, signs and submits. Nothing here signs.

use std::collections::VecDeque;

use super::calls::{activity_summary, build_staking_calls, BuildInput, StakingAction};
use super::read::{PoolOutcome, StakingPair, StakingRead, StakingReading};
use super::review::review_sentence;
use super::{staker_address, StakingNetwork, StakingPolicy, StakingRefusal};
use crate::catalog::TokenInfo;
use crate::js::felt::canonical_felt;
use crate::js::num::short_label;
use crate::portfolio::model::TokenConfig;
use crate::send::call_decode::Known;
use crate::send::calls::parse_token_amount;
use crate::send::invoke::Invoke;
use crate::send::multisig_invoke::StarkPrimitives;
use crate::send::types::{Account, Policy};
use crate::{felt, Felt, NetworkId};

/// `MAX_AMOUNT_LENGTH`: the longest amount text a request may carry.
const MAX_AMOUNT_LENGTH: usize = 80;

/// `AMOUNT_PATTERN` (`^\d+(?:\.\d+)?$`) within the length bound.
fn is_amount(value: &str) -> bool {
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    value.len() <= MAX_AMOUNT_LENGTH
        && match value.split_once('.') {
            Some((whole, fraction)) => digits(whole) && digits(fraction),
            None => digits(value),
        }
}

/// `StakingPrepareInput`: what the person asked for. Never a pool or calldata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrepareInput {
    pub action: StakingAction,
    /// The decimal amount entered, for stake and unstake only.
    pub amount: Option<String>,
    /// The canonical staker address.
    pub staker: Felt,
}

impl PrepareInput {
    /// `readStakingPrepareInput`'s rules: the staker is a non-zero felt, and an amount is
    /// required exactly for the actions that move one and refused for the others.
    pub fn new(
        action: StakingAction,
        amount: Option<&str>,
        staker: &str,
    ) -> Result<Self, StakingRefusal> {
        let staker = staker_address(staker).ok_or(StakingRefusal::InvalidRequest)?;
        let amount = match (action.takes_amount(), amount) {
            (true, Some(amount)) if is_amount(amount) => Some(amount.to_string()),
            (false, None) => None,
            _ => return Err(StakingRefusal::InvalidRequest),
        };
        Ok(Self {
            action,
            amount,
            staker,
        })
    }

    /// The read this action needs: its staker and `token` for `account`, against `staking`'s
    /// contract. `token` must be one the account tracks on its own network.
    pub fn read(
        &self,
        staking: &StakingNetwork,
        rewards: &TokenInfo,
        account: &Account,
        token: &TokenConfig,
    ) -> Result<StakingRead, StakingRefusal> {
        if staking.network != account.network || !staking.enabled {
            return Err(StakingRefusal::Unavailable);
        }
        let pair = action_pair(self, account, token)?;
        StakingRead::new(staking, rewards, account.address, vec![pair])
            .ok_or(StakingRefusal::Unavailable)
    }
}

fn action_pair(
    input: &PrepareInput,
    account: &Account,
    token: &TokenConfig,
) -> Result<StakingPair, StakingRefusal> {
    let address = canonical_felt(&token.address).and_then(|a| felt::parse_non_zero(&a));
    match address {
        Some(token_address) if token.network == account.network => Ok(StakingPair {
            staker: input.staker,
            token: token_address,
        }),
        _ => Err(StakingRefusal::OptionUnavailable),
    }
}

/// What `prepare` needs besides the input and the reading: the acting account and tracked
/// token, the clock (Unix seconds) and `Invoke::new`'s arguments.
pub struct PrepareContext<'a> {
    pub account: &'a Account,
    pub token: &'a TokenConfig,
    pub now: u64,
    pub policy: &'a Policy,
    pub known: &'a Known<'a>,
    pub trusted: &'a [Felt],
    pub primitives: &'a dyn StarkPrimitives,
}

/// `StakingPreparation`: a reviewed action, frozen until one confirm. `invoke` is bound to the
/// account and network it was reviewed for, and is what the plan signs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StakingPreparation {
    pub action: StakingAction,
    /// The amount the person entered, for the activity row.
    pub amount: Option<String>,
    /// The verified pool.
    pub pool: Felt,
    /// What the calls do, in the signing wording.
    pub review: String,
    /// The activity summary (`stakingActivitySummary`).
    pub summary: String,
    /// The activity row's token: the rewards token for a claim, the staked one otherwise.
    pub symbol: String,
    pub token_address: Felt,
    pub invoke: Invoke,
}

/// `prepareStakingResponse` past its reads: builds and reviews one action from a finished
/// `PrepareInput::read`. The review names the full staker address and the activity its short
/// form, never a directory name.
pub fn prepare(
    input: &PrepareInput,
    reading: Option<&StakingReading>,
    context: &PrepareContext,
) -> Result<StakingPreparation, StakingRefusal> {
    let account = context.account;
    let pair = action_pair(input, account, context.token)?;
    let reading = reading
        .filter(|r| r.is_for(&account.address, account.network))
        .ok_or(StakingRefusal::Unreadable)?;
    if reading.facts.is_none() {
        return Err(StakingRefusal::TokenNotStakeable);
    }
    let pool = reading
        .pools
        .iter()
        .find(|pool| pool.pair == pair)
        .ok_or(StakingRefusal::Unreadable)?;
    let (verified, member) = match &pool.outcome {
        PoolOutcome::Verified { pool, member } => (pool, member.as_ref()),
        PoolOutcome::Refused(refusal) => return Err(*refusal),
        PoolOutcome::Inactive => return Err(StakingRefusal::TokenNotStakeable),
    };
    let token = context.token;
    let calls = build_staking_calls(&BuildInput {
        account: account.address,
        action: input.action,
        amount: input
            .amount
            .as_deref()
            .and_then(|amount| parse_token_amount(amount, token.decimals).ok()),
        member,
        now: context.now,
        pool: verified,
        token: pair.token,
    })?;
    let invoke = Invoke::new(
        account,
        calls,
        context.policy,
        context.known,
        context.trusted,
        context.primitives,
    )?;
    let staker = felt::canonical(&input.staker);
    let amount = input.amount.as_deref();
    let claim = input.action == StakingAction::Claim;
    let rewards = &reading.rewards;
    Ok(StakingPreparation {
        action: input.action,
        amount: input.amount.clone(),
        pool: verified.pool,
        review: review_sentence(
            &account.address,
            input.action,
            amount,
            member,
            &token.symbol,
            &format!("validator {staker}"),
        ),
        summary: activity_summary(
            input.action,
            amount,
            &token.symbol,
            &format!("validator {}", short_label(&staker)),
        ),
        // A claim pays the rewards token whatever the staked one.
        symbol: if claim {
            rewards.symbol.clone()
        } else {
            token.symbol.clone()
        },
        token_address: if claim {
            canonical_felt(&rewards.address)
                .and_then(|a| felt::parse(&a))
                .unwrap_or(pair.token)
        } else {
            pair.token
        },
        invoke,
    })
}

/// `createStakingPreparationStore`: the one-time preparations, at most the policy's
/// `max_preparations` (the oldest is dropped past it; a dropped review is prepared again). A
/// wallet that shares one across threads wraps it in its own lock.
#[derive(Debug)]
pub struct StakingPreparations {
    pending: VecDeque<(u64, StakingPreparation)>,
    last_id: u64,
}

impl StakingPreparations {
    /// An empty store whose ids start at a random point, so an id held from another store (an
    /// earlier process, a store rebuilt on unlock) never names one of this store's preparations.
    pub fn new() -> Result<Self, crate::Error> {
        Ok(Self {
            pending: VecDeque::new(),
            last_id: u64::from_le_bytes(crate::envelope::random()?),
        })
    }

    /// Holds `preparation` for one confirm and returns its id.
    pub fn put(&mut self, preparation: StakingPreparation, policy: &StakingPolicy) -> u64 {
        self.last_id = self.last_id.wrapping_add(1);
        self.pending.push_back((self.last_id, preparation));
        while self.pending.len() > policy.max_preparations {
            self.pending.pop_front();
        }
        self.last_id
    }

    /// `executeStakingResponse`'s claim: removes the preparation first, so a double tap cannot
    /// submit it twice, then requires staking still on and the preparation reviewed for this
    /// `account` on this `network`. Any miss is "Review this staking action again."
    pub fn claim(
        &mut self,
        id: u64,
        account: &Felt,
        network: NetworkId,
        policy: &StakingPolicy,
    ) -> Result<StakingPreparation, StakingRefusal> {
        let index = self
            .pending
            .iter()
            .position(|(held, _)| *held == id)
            .ok_or(StakingRefusal::ReviewAgain)?;
        let (_, preparation) = self
            .pending
            .remove(index)
            .ok_or(StakingRefusal::ReviewAgain)?;
        policy.network(network).ok_or(StakingRefusal::Unavailable)?;
        if preparation.invoke.sender != *account || preparation.invoke.network != network {
            return Err(StakingRefusal::ReviewAgain);
        }
        Ok(preparation)
    }
}
