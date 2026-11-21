//! Decoders for the native staking reads (`staking-decode.ts`: starkware-libs/starknet-staking,
//! layouts the extension checked against the deployed Mainnet ABIs) and the pool verification
//! rule. Every input is an untrusted RPC answer: a decoder returns `None` for anything that does
//! not match the layout exactly (wrong length, a non-felt, an out-of-range integer, an unknown
//! `Option` variant), and nothing is built from `None`.

use super::StakingRefusal;
use crate::{felt, Felt};

const U16_LIMIT: u128 = 1 << 16;
const U64_LIMIT: u128 = 1 << 64;
/// The longest span a decoder reads (`bounded(values, cursor, 64n)`).
const SPAN_LIMIT: u128 = 64;
// Cairo `Option` serialises its variant index first: Some = 0, None = 1.
const SOME: u128 = 0;
const NONE: u128 = 1;

// `felt`: a value in the extension's strict grammar (`isStarkFieldElement`).
fn felt_at(values: &[String], index: usize) -> Option<Felt> {
    felt::parse(values.get(index)?)
}

// `felt` as an integer, when it is below 2^128 (`UINT128_LIMIT`).
fn u128_at(values: &[String], index: usize) -> Option<u128> {
    u128::try_from(felt_at(values, index)?).ok()
}

// `bounded`: below `limit`.
fn bounded(values: &[String], index: usize, limit: u128) -> Option<u128> {
    u128_at(values, index).filter(|value| *value < limit)
}

// `address`: a non-zero felt.
fn address(values: &[String], index: usize) -> Option<Felt> {
    felt_at(values, index).filter(|value| *value != Felt::ZERO)
}

// The variant index of an `Option` at `index`, as the integer it is (any other felt is neither).
fn variant(values: &[String], index: usize) -> Option<u128> {
    u128_at(values, index)
}

/// One delegation pool a staker runs, as the staking contract reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StakerPool {
    pub amount: u128,
    pub pool: Felt,
    pub token: Felt,
}

/// `staker_pool_info(staker)`: the staker's commission (basis points, when set) and its pools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StakerPoolInfo {
    pub commission: Option<u16>,
    pub pools: Vec<StakerPool>,
}

/// `decodeStakerPoolInfo`: `Option<u16>` commission, then a span of
/// `(pool_contract, token_address, amount)`.
pub fn decode_staker_pool_info(values: &[String]) -> Option<StakerPoolInfo> {
    let (commission, cursor) = match variant(values, 0)? {
        SOME => (Some(bounded(values, 1, U16_LIMIT)? as u16), 2),
        NONE => (None, 1),
        _ => return None,
    };
    let count = bounded(values, cursor, SPAN_LIMIT)? as usize;
    if values.len() != cursor + 1 + count * 3 {
        return None;
    }
    let pools = (0..count)
        .map(|index| {
            let base = cursor + 1 + index * 3;
            Some(StakerPool {
                pool: address(values, base)?,
                token: address(values, base + 1)?,
                amount: u128_at(values, base + 2)?,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(StakerPoolInfo { commission, pools })
}

/// Whether the staker exists and is not leaving (`get_staker_info_v1`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StakerStatus {
    Active,
    Leaving,
    Unknown,
}

/// `decodeStakerStatus`: `Option<StakerInfoV1>`, whose fourth field is `Option<u64>`
/// `unstake_time`. Like the extension, the fields after the ones it reads are not checked.
pub fn decode_staker_status(values: &[String]) -> Option<StakerStatus> {
    match variant(values, 0)? {
        NONE => return (values.len() == 1).then_some(StakerStatus::Unknown),
        SOME => address(values, 1)?,
        _ => return None,
    };
    match variant(values, 3)? {
        NONE => Some(StakerStatus::Active),
        SOME => bounded(values, 4, U64_LIMIT).map(|_| StakerStatus::Leaving),
        _ => None,
    }
}

/// A pool's own `contract_parameters_v1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolParameters {
    pub commission: u16,
    pub staker: Felt,
    pub staker_removed: bool,
    pub staking_contract: Felt,
    pub token: Felt,
}

/// `decodePoolParameters`:
/// `(staker_address, staker_removed, staking_contract, token_address, commission)`.
pub fn decode_pool_parameters(values: &[String]) -> Option<PoolParameters> {
    if values.len() != 5 {
        return None;
    }
    Some(PoolParameters {
        staker: address(values, 0)?,
        staker_removed: bounded(values, 1, 2)? == 1,
        staking_contract: address(values, 2)?,
        token: address(values, 3)?,
        commission: bounded(values, 4, U16_LIMIT)? as u16,
    })
}

/// The account's membership in one pool. `unpool_time` is the Unix second the pending exit
/// becomes withdrawable, or `None` when no exit is pending.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolMember {
    pub amount: u128,
    pub reward_address: Felt,
    pub unclaimed_rewards: u128,
    pub unpool_amount: u128,
    pub unpool_time: Option<u64>,
}

/// `decodePoolMember` (`get_pool_member_info_v1`, `Option<PoolMemberInfoV1>`): `Some(None)` when
/// the account is not a member, `None` when the answer is malformed.
pub fn decode_pool_member(values: &[String]) -> Option<Option<PoolMember>> {
    match variant(values, 0)? {
        NONE => return (values.len() == 1).then_some(None),
        SOME => {}
        _ => return None,
    }
    let reward_address = address(values, 1)?;
    let amount = u128_at(values, 2)?;
    let unclaimed_rewards = u128_at(values, 3)?;
    bounded(values, 4, U16_LIMIT)?;
    let unpool_amount = u128_at(values, 5)?;
    let unpool_time = match variant(values, 6) {
        Some(NONE) if values.len() == 7 => None,
        Some(SOME) if values.len() == 8 => Some(bounded(values, 7, U64_LIMIT)? as u64),
        _ => return None,
    };
    Some(Some(PoolMember {
        amount,
        reward_address,
        unclaimed_rewards,
        unpool_amount,
        unpool_time,
    }))
}

/// `decodeActiveTokens`: the span of token addresses the staking contract accepts.
pub fn decode_active_tokens(values: &[String]) -> Option<Vec<Felt>> {
    let count = bounded(values, 0, SPAN_LIMIT)? as usize;
    if values.len() != count + 1 {
        return None;
    }
    (1..=count).map(|index| address(values, index)).collect()
}

/// `decodeExitWindow`: the staking contract's exit wait in seconds, the last field of its
/// `contract_parameters_v1`.
pub fn decode_exit_window(values: &[String]) -> Option<u64> {
    if values.len() != 6 {
        return None;
    }
    bounded(values, 5, U64_LIMIT).map(|seconds| seconds as u64)
}

/// Everything read for one validator and token, at one block.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StakingPoolReads {
    pub parameters: Option<PoolParameters>,
    pub pool_info: Option<StakerPoolInfo>,
    pub status: Option<StakerStatus>,
}

/// A pool the wallet verified for a validator and token. `accepting` is false while the
/// validator is leaving or was removed from the pool: members can still claim, exit and
/// withdraw, but no new stake is built into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifiedPool {
    pub accepting: bool,
    pub commission: u16,
    pub delegated: u128,
    pub pool: Felt,
}

/// `verifyStakingPool`: a pool is accepted only when the staking contract lists it as `staker`'s
/// pool for `token`, and the pool's own parameters name the same staker, staking contract and
/// token. Anything else is a refusal.
pub fn verify_staking_pool(
    reads: &StakingPoolReads,
    staker: &Felt,
    staking_contract: &Felt,
    token: &Felt,
) -> Result<VerifiedPool, StakingRefusal> {
    let (Some(pool_info), Some(status)) = (&reads.pool_info, reads.status) else {
        return Err(StakingRefusal::Unverified);
    };
    if status == StakerStatus::Unknown {
        return Err(StakingRefusal::Unverified);
    }
    let listed = pool_info
        .pools
        .iter()
        .find(|pool| pool.token == *token)
        .ok_or(StakingRefusal::NoPoolForToken)?;
    let parameters = reads
        .parameters
        .filter(|p| p.staker == *staker && p.staking_contract == *staking_contract)
        .filter(|p| p.token == *token)
        .ok_or(StakingRefusal::Unverified)?;
    Ok(VerifiedPool {
        accepting: status == StakerStatus::Active && !parameters.staker_removed,
        commission: pool_info.commission.unwrap_or(parameters.commission),
        delegated: listed.amount,
        pool: listed.pool,
    })
}
