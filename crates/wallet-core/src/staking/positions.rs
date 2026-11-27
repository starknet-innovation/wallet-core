//! Starkscan's staking positions of one address ("Your stakes",
//! `GET {base}/{chain}/staking/address/{address}?limit=1`, schema `StakingAddressView`), from
//! `staking-positions.ts` and the request rules of `staking-positions-ops.ts`.
//!
//! The answer is untrusted: it only says which validators and tokens to ask the chain about. It
//! never supplies an amount that is shown, a pool that is used, or anything that is signed.
//! Every field the wallet reads is checked, the list is bounded before it is walked, and
//! anything malformed refuses the whole answer. The list is complete only when it is not
//! truncated and its coverage is usable; anything else is incomplete, never "no stakes".

use std::collections::HashSet;

use super::coverage::{staking_coverage, Coverage};
use super::directory::{starkscan_felt, starkscan_raw};
use super::read::StakingPair;
use super::{StakingPolicy, StakingRefusal};
use crate::history::starkscan::{starkscan_get, starkscan_json};
use crate::js::json::JsValue;
use crate::portfolio::wire::{HttpOutcome, HttpRequest};
use crate::{felt, Felt, NetworkId};

/// `MAX_STAKING_POSITIONS`: the most positions Starkscan's schema allows in one answer.
pub const MAX_STAKING_POSITIONS: usize = 256;

/// One validated answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionsPage {
    /// True only for usable coverage with nothing truncated.
    pub complete: bool,
    /// Candidates for an on-chain read, no more.
    pub positions: Vec<StakingPair>,
}

fn position_of(value: &JsValue, account: &Felt) -> Option<StakingPair> {
    value.as_object()?;
    let staker = starkscan_felt(value.get("validatorAddress")).filter(|s| *s != Felt::ZERO)?;
    let token = starkscan_felt(value.get("tokenAddress"))?;
    starkscan_raw(value.get("pendingExitRaw"), false)?;
    let valid = starkscan_felt(value.get("poolAddress")).is_some()
        && starkscan_felt(value.get("address")) == Some(*account)
        && starkscan_raw(value.get("delegatedRaw"), false).is_some_and(|raw| raw.is_some())
        && value
            .get("status")
            .and_then(JsValue::as_str)
            .is_some_and(|status| status == "active" || status == "exiting");
    valid.then_some(StakingPair { staker, token })
}

/// `parseStakingPositions` for `account` on `network`: `None` when anything the wallet reads is
/// malformed or out of bounds, when it answers for another chain or address, or lists a
/// validator and token twice.
pub fn parse_positions(
    value: &JsValue,
    network: NetworkId,
    account: &Felt,
    policy: &StakingPolicy,
) -> Option<PositionsPage> {
    value.as_object()?;
    let coverage = staking_coverage(value.get("coverage"), policy.max_catching_up_lag_blocks)?;
    let truncated = match value.get("positionsTruncated")? {
        JsValue::Bool(flag) => *flag,
        _ => return None,
    };
    if value.get("chainId")?.as_str()? != network.catalog_id()
        || value.get("source")?.as_str()? != super::directory::SNAPSHOT_SOURCE
        || starkscan_felt(value.get("address")) != Some(*account)
    {
        return None;
    }
    let items = value
        .get("positions")?
        .as_array()
        .filter(|items| items.len() <= MAX_STAKING_POSITIONS)?;
    let mut seen = HashSet::new();
    let positions = items
        .iter()
        .map(|item| {
            position_of(item, account).filter(|pair| seen.insert((pair.staker, pair.token)))
        })
        .collect::<Option<Vec<_>>>()?;
    Some(PositionsPage {
        complete: coverage == Coverage::Usable && !truncated,
        positions,
    })
}

/// The positions request (`getStakingAddress`) for `account`: `Ok(None)` is "off", with no
/// request, where there is no directory on this network or the person's explorer choice keeps
/// this account's addresses from Starkscan (`explorer_allowed`, the extension's
/// `explorerFallbackAllowed`). `limit=1` only bounds the history, which the wallet doesn't read.
pub fn positions_request(
    network: NetworkId,
    policy: &StakingPolicy,
    api_base: &str,
    account: &Felt,
    explorer_allowed: bool,
) -> Result<Option<HttpRequest>, StakingRefusal> {
    let staking = policy.network(network).ok_or(StakingRefusal::Unavailable)?;
    if !staking.starkscan || !explorer_allowed {
        return Ok(None);
    }
    Ok(Some(starkscan_get(format!(
        "{api_base}/{}/staking/address/{}?limit=1",
        network.catalog_id(),
        felt::canonical(account)
    ))))
}

/// Judges a positions reply: any transport, status, size or schema failure, or an answer for
/// another chain or address, is `PositionsUnreadable`.
pub fn positions_page(
    outcome: &HttpOutcome,
    network: NetworkId,
    account: &Felt,
    policy: &StakingPolicy,
) -> Result<PositionsPage, StakingRefusal> {
    starkscan_json(outcome)
        .and_then(|value| parse_positions(&value, network, account, policy))
        .ok_or(StakingRefusal::PositionsUnreadable)
}
