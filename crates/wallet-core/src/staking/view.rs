//! What a host shows (`staking-row-view.ts`, `staking-view.ts`, and the row rules of
//! `staking-ops.ts` and `staking-positions-ops.ts`): one validator's pools for the account (the
//! overview) and the account's positions across validators ("Your stakes"). Every figure comes
//! from a verified on-chain reading, formatted in the token's own decimals (rewards in the
//! rewards token's); nothing is read from Starkscan, and nothing here builds a call.

use super::calls::{position_state, PositionState};
use super::directory::hundredths_percent;
use super::positions::PositionsPage;
use super::read::{PoolOutcome, PoolReading, StakingPair, StakingReading};
use super::{StakingPolicy, StakingRefusal};
use crate::js::felt::{canonical_felt, same_felt};
use crate::portfolio::model::TokenConfig;
use crate::send::calls::format_token_amount;
use crate::send::u256::U256;
use crate::{felt, Felt, NetworkId};

/// A verified row's figures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowFigures {
    /// False while the validator is leaving: no new stake, exits still work.
    pub accepting: bool,
    /// Commission, e.g. `"10%"`.
    pub commission: String,
    /// Total delegated to the pool.
    pub delegated: String,
    /// The membership's reward address, when it is not this account.
    pub rewards_to: Option<String>,
    /// The account's stake (`"0"` when not a member).
    pub staked: String,
    pub state: PositionState,
    /// Unclaimed rewards, in the rewards token (STRK, whatever the staked token).
    pub unclaimed: String,
    /// The amount waiting out the exit window.
    pub unpool_amount: String,
    /// Unix seconds the pending exit becomes withdrawable.
    pub unpool_time: Option<u64>,
}

/// `StakingRowView`: one staker and tracked token. A refusal replaces the figures when the pool
/// could not be verified, and the row offers no action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StakingRow {
    /// The canonical staker address the row was read and verified for.
    pub staker: String,
    pub symbol: String,
    pub token_id: String,
    pub figures: Result<RowFigures, StakingRefusal>,
}

/// The first tracked token on `network` at `address`.
fn tracked<'a>(
    tokens: &'a [TokenConfig],
    network: NetworkId,
    address: &Felt,
) -> Option<&'a TokenConfig> {
    let address = felt::canonical(address);
    tokens
        .iter()
        .find(|token| token.network == network && same_felt(&token.address, &address))
}

/// `rowView` for one pair, or `None` when its token is not staked (`Inactive`).
fn row_view(
    reading: &PoolReading,
    token: &TokenConfig,
    account: &Felt,
    rewards_decimals: u8,
    now: u64,
) -> Option<StakingRow> {
    let figures = match &reading.outcome {
        PoolOutcome::Inactive => return None,
        PoolOutcome::Refused(refusal) => Err(*refusal),
        PoolOutcome::Verified { pool, member } => {
            let format = |value: u128| format_token_amount(&U256::from_u128(value), token.decimals);
            Ok(RowFigures {
                accepting: pool.accepting,
                commission: hundredths_percent(u64::from(pool.commission)),
                delegated: format(pool.delegated),
                rewards_to: member
                    .filter(|member| member.reward_address != *account)
                    .map(|member| felt::canonical(&member.reward_address)),
                staked: format(member.map_or(0, |m| m.amount)),
                state: position_state(member.as_ref(), now),
                unclaimed: format_token_amount(
                    &U256::from_u128(member.map_or(0, |m| m.unclaimed_rewards)),
                    rewards_decimals,
                ),
                unpool_amount: format(member.map_or(0, |m| m.unpool_amount)),
                unpool_time: member.and_then(|m| m.unpool_time),
            })
        }
    };
    Some(StakingRow {
        staker: felt::canonical(&reading.pair.staker),
        symbol: token.symbol.clone(),
        token_id: token.id.clone(),
        figures,
    })
}

/// The reading behind a view: finished, made for `account` on `network`, with the staking
/// contract's facts.
fn usable<'a>(
    reading: Option<&'a StakingReading>,
    account: &Felt,
    network: NetworkId,
) -> Option<&'a StakingReading> {
    reading.filter(|r| r.is_for(account, network) && r.facts.is_some())
}

/// The pairs for one validator's overview: `staker` with each token the account tracks on
/// `network` (the read leaves out those the staking contract does not take).
pub fn overview_pairs(
    staker: &Felt,
    tokens: &[TokenConfig],
    network: NetworkId,
) -> Vec<StakingPair> {
    tokens
        .iter()
        .filter(|token| token.network == network)
        .filter_map(|token| canonical_felt(&token.address).and_then(|a| felt::parse(&a)))
        .map(|token| StakingPair {
            staker: *staker,
            token,
        })
        .collect()
}

/// One staker's pools for the account (`wallet.stakingOverview`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StakingOverview {
    /// Seconds an unstake waits, read from the staking contract.
    pub exit_window_seconds: u64,
    pub rows: Vec<StakingRow>,
}

/// The overview from a finished read of `overview_pairs`: `Unreadable` when the read failed,
/// was made for another account or network, or the staking contract's facts were unreadable.
pub fn overview(
    reading: Option<&StakingReading>,
    tokens: &[TokenConfig],
    account: &Felt,
    network: NetworkId,
    now: u64,
) -> Result<StakingOverview, StakingRefusal> {
    let reading = usable(reading, account, network).ok_or(StakingRefusal::Unreadable)?;
    let decimals = reading.rewards.decimals;
    let rows = reading
        .pools
        .iter()
        .filter_map(|pool| {
            let token = tracked(tokens, network, &pool.pair.token)?;
            row_view(pool, token, account, decimals, now)
        })
        .collect();
    Ok(StakingOverview {
        exit_window_seconds: reading.facts.as_ref().map_or(0, |f| f.exit_window_seconds),
        rows,
    })
}

/// The candidates of a positions page worth reading on chain, and whether the list can still be
/// whole: at most the policy's `max_checked_positions`, each in a token the account tracks on
/// `network`. More candidates, or one in an untracked token, make the list incomplete.
pub fn positions_pairs(
    page: &PositionsPage,
    tokens: &[TokenConfig],
    network: NetworkId,
    policy: &StakingPolicy,
) -> (Vec<StakingPair>, bool) {
    let mut complete = page.complete && page.positions.len() <= policy.max_checked_positions;
    let pairs = page
        .positions
        .iter()
        .take(policy.max_checked_positions)
        .filter(|pair| {
            let found = tracked(tokens, network, &pair.token).is_some();
            complete &= found;
            found
        })
        .copied()
        .collect();
    (pairs, complete)
}

/// `StakingPositionsView`'s ready state: the positions the chain confirmed. `complete` is false
/// whenever Starkscan's list was incomplete or truncated, or a listed position could not be
/// checked; the screen then never reads an empty list as "no stakes".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionsView {
    pub complete: bool,
    pub rows: Vec<StakingRow>,
}

// A row worth listing: the account holds a stake, rewards or a pending unstake in the pool.
fn holds(reading: &PoolReading) -> bool {
    matches!(
        &reading.outcome,
        PoolOutcome::Verified { member: Some(m), .. }
            if m.amount > 0 || m.unclaimed_rewards > 0 || m.unpool_amount > 0
    )
}

/// "Your stakes" from a finished read of `positions_pairs`' pairs and its `complete`: a
/// candidate the chain could not verify, or in a token the staking contract does not take, is
/// not shown and makes the list incomplete.
pub fn positions_view(
    reading: Option<&StakingReading>,
    tokens: &[TokenConfig],
    account: &Felt,
    network: NetworkId,
    now: u64,
    complete: bool,
) -> Result<PositionsView, StakingRefusal> {
    let reading = usable(reading, account, network).ok_or(StakingRefusal::PositionsUnreadable)?;
    let checked = reading
        .pools
        .iter()
        .all(|pool| matches!(pool.outcome, PoolOutcome::Verified { .. }));
    let rows = reading
        .pools
        .iter()
        .filter(|pool| holds(pool))
        .filter_map(|pool| {
            let token = tracked(tokens, network, &pool.pair.token)?;
            row_view(pool, token, account, reading.rewards.decimals, now)
        })
        .collect();
    Ok(PositionsView {
        complete: complete && checked,
        rows,
    })
}
