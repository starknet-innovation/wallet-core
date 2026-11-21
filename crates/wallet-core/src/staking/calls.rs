//! The staking actions, their calls and the rules for when each is allowed (`staking-calls.ts`).
//! Every builder takes a pool `read` already verified and the account's membership read at the
//! same block, and decides from those alone. The host never supplies calldata; the calls are
//! rebuilt on every request. Amounts are exact: a stake approves the pool for exactly what it
//! stakes, in the same batch, never an open-ended allowance.

use super::decode::{PoolMember, VerifiedPool};
use super::StakingRefusal;
use crate::js::num::js_trim;
use crate::send::calls::Call;
use crate::send::u256::U256;
use crate::Felt;

/// `StakingAction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StakingAction {
    /// First stake or add stake (`approve` + enter or add).
    Stake,
    /// Claim the pool's rewards to the member's reward address.
    Claim,
    /// Start the exit window for an amount.
    Unstake,
    /// Cancel a pending exit (`exit_delegation_pool_intent(0)`).
    CancelUnstake,
    /// Withdraw a pending exit once its window has passed.
    Withdraw,
}

impl StakingAction {
    /// Whether the action moves an amount the person enters (stake, unstake).
    pub fn takes_amount(self) -> bool {
        matches!(self, Self::Stake | Self::Unstake)
    }
}

/// `StakingPositionState`: where the account stands in one pool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionState {
    /// `none`: not a member, or a member with nothing staked and no exit pending.
    NotStaked,
    Staked,
    ExitPending,
    Withdrawable,
}

/// `stakingPositionState` at Unix second `now`.
pub fn position_state(member: Option<&PoolMember>, now: u64) -> PositionState {
    let Some(member) = member else {
        return PositionState::NotStaked;
    };
    match member.unpool_time {
        None if member.amount > 0 => PositionState::Staked,
        None => PositionState::NotStaked,
        Some(time) if now >= time => PositionState::Withdrawable,
        Some(_) => PositionState::ExitPending,
    }
}

/// `StakingBuildInput`: everything one action is built from, all wallet-side.
#[derive(Debug, Clone, Copy)]
pub struct BuildInput<'a> {
    pub account: Felt,
    pub action: StakingAction,
    /// Base units; read for stake and unstake only.
    pub amount: Option<U256>,
    pub member: Option<&'a PoolMember>,
    pub now: u64,
    pub pool: &'a VerifiedPool,
    pub token: Felt,
}

// `validAmount`: above zero and below 2^128.
fn valid_amount(amount: Option<U256>) -> Option<u128> {
    amount.filter(|a| a.high == 0 && a.low > 0).map(|a| a.low)
}

fn pool_call(pool: &Felt, entrypoint: &str, calldata: Vec<Felt>) -> Call {
    Call {
        contract_address: *pool,
        entrypoint: entrypoint.into(),
        calldata,
    }
}

fn build_stake(input: &BuildInput) -> Result<Vec<Call>, StakingRefusal> {
    if !input.pool.accepting {
        return Err(StakingRefusal::Leaving);
    }
    let amount = valid_amount(input.amount).ok_or(StakingRefusal::StakeAmount)?;
    // Adding while an exit is pending is allowed by the pool (the extension checked a Mainnet
    // simulation); the pending exit is unchanged.
    let entrypoint = if input.member.is_some() {
        "add_to_delegation_pool"
    } else {
        "enter_delegation_pool"
    };
    Ok(vec![
        // `buildErc20ApproveCall(token, pool, amount)`: spender, then the u256 limbs.
        Call {
            contract_address: input.token,
            entrypoint: "approve".into(),
            calldata: vec![input.pool.pool, Felt::from(amount), Felt::ZERO],
        },
        pool_call(
            &input.pool.pool,
            entrypoint,
            vec![input.account, Felt::from(amount)],
        ),
    ])
}

fn build_unstake(input: &BuildInput, state: PositionState) -> Result<Vec<Call>, StakingRefusal> {
    if matches!(
        state,
        PositionState::ExitPending | PositionState::Withdrawable
    ) {
        return Err(StakingRefusal::PendingExit);
    }
    let member = input
        .member
        .filter(|_| state == PositionState::Staked)
        .ok_or(StakingRefusal::NothingStaked)?;
    let amount = valid_amount(input.amount).ok_or(StakingRefusal::UnstakeAmount)?;
    if amount > member.amount {
        return Err(StakingRefusal::MoreThanStaked);
    }
    Ok(vec![pool_call(
        &input.pool.pool,
        "exit_delegation_pool_intent",
        vec![Felt::from(amount)],
    )])
}

/// `buildStakingCalls`: the calls for one action, or the reason it is not allowed in the
/// account's current state.
pub fn build_staking_calls(input: &BuildInput) -> Result<Vec<Call>, StakingRefusal> {
    let state = position_state(input.member, input.now);
    let pool = &input.pool.pool;
    match input.action {
        StakingAction::Stake => build_stake(input),
        StakingAction::Claim => match input.member {
            Some(member) if member.unclaimed_rewards > 0 => {
                Ok(vec![pool_call(pool, "claim_rewards", vec![input.account])])
            }
            _ => Err(StakingRefusal::NoRewards),
        },
        StakingAction::Unstake => build_unstake(input, state),
        StakingAction::CancelUnstake => match state {
            PositionState::ExitPending | PositionState::Withdrawable => Ok(vec![pool_call(
                pool,
                "exit_delegation_pool_intent",
                vec![Felt::ZERO],
            )]),
            _ => Err(StakingRefusal::NoUnstake),
        },
        StakingAction::Withdraw => match state {
            PositionState::Withdrawable => Ok(vec![pool_call(
                pool,
                "exit_delegation_pool_action",
                vec![input.account],
            )]),
            PositionState::ExitPending => Err(StakingRefusal::NotWithdrawableYet),
            _ => Err(StakingRefusal::NothingToWithdraw),
        },
    }
}

/// `stakingActivitySummary`: the activity line for an action. Activity dedup and the row
/// classifier match on these verbs (stake, unstake, claim rewards), so they must not drift.
pub fn activity_summary(
    action: StakingAction,
    amount: Option<&str>,
    symbol: &str,
    validator: &str,
) -> String {
    let figure = format!("{} {symbol}", amount.unwrap_or_default());
    let figure = js_trim(&figure);
    match action {
        StakingAction::Stake => format!("Stake {figure} with {validator}"),
        StakingAction::Claim => format!("Claim staking rewards from {validator}"),
        StakingAction::Unstake => format!("Unstake {figure} from {validator}"),
        StakingAction::CancelUnstake => format!("Keep {symbol} staked with {validator}"),
        StakingAction::Withdraw => format!("Withdraw unstaked {figure} from {validator}"),
    }
}
