//! The build-output policy (`swap-build-policy.ts`) and the route binding and floor
//! (`avnuRouteIntentError`, `reviewedMinBuyAmount` in `wallet-model/src/swap/avnu-route.ts`):
//! whether the call set AVNU returned is the swap the person reviewed. Every side of every
//! equality is wallet-side: the pair and amount from the bound quote, the beneficiary from the
//! account, the counterparty from the policy's pin, the floor from the reviewed quote. Pure: no
//! clock, no I/O.

use crate::send::avnu_route::{self, AvnuRoute};
use crate::send::call_decode::{
    call_summary, recipient_felt_index, uint256_at, Known, AVNU_MULTI_ROUTE_SWAP,
};
use crate::send::calls::Call;
use crate::send::u256::U256;
use crate::Felt;

/// `APPROVE_ENTRYPOINTS` (`verify-selectors.ts`): the selectors that grant an ERC-20 allowance.
/// `send::call_decode` holds the same list privately for its summaries.
pub const APPROVE_ENTRYPOINTS: [&str; 3] = ["approve", "increaseAllowance", "increase_allowance"];

/// `integrator_fee_amount_bps` in the pinned `multi_route_swap` layout (`avnu-route.ts`).
const INTEGRATOR_FEE_BPS_INDEX: usize = 9;
const BPS_DENOMINATOR: u64 = 10_000;

/// Why a build was refused, in the order the extension judges it. `Display` is its sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BuildPolicyError {
    #[error("swap build returned too many calls")]
    TooManyCalls,
    #[error("swap build requested an additive token allowance")]
    AdditiveAllowance,
    #[error("swap build touched an unexpected token approval")]
    UnexpectedApprovalToken,
    #[error("swap build requested an unexpected token allowance")]
    UnexpectedAllowance,
    #[error("swap build approved an unexpected spender")]
    UnexpectedSpender,
    #[error("swap build returned multiple token approvals")]
    MultipleApprovals,
    #[error("swap build omitted the sell token approval")]
    MissingApproval,
    #[error("swap build requested a direct token transfer")]
    DirectTransfer,
    #[error("swap build returned no swap call")]
    NoSwapCall,
    #[error("swap build called a contract that is not the avnu router")]
    NotRouter,
    #[error("swap build called an entrypoint that is not an avnu swap")]
    NotSwapEntrypoint,
    #[error("swap build returned more than one swap call")]
    MultipleSwapCalls,
    #[error("swap build returned a route the wallet could not decode")]
    UndecodableRoute,
    #[error("swap build routed a token you are not selling")]
    SellTokenMismatch,
    #[error("swap build routed an amount you did not review")]
    SellAmountMismatch,
    #[error("swap build bought a token you did not choose")]
    BuyTokenMismatch,
    #[error("swap build routed the proceeds to another account")]
    BeneficiaryMismatch,
    #[error("swap build accepted less than the minimum you reviewed")]
    BelowMinimum,
    #[error("swap build charged an aggregator fee beyond the wallet's cap")]
    FeeAboveCap,
}

/// `SwapIntent`, with the policy's two numbers the gate applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Intent {
    pub(super) sell_token: Felt,
    pub(super) sell_amount: U256,
    pub(super) buy_token: Felt,
    pub(super) taker: Felt,
    pub(super) router: Felt,
    pub(super) min_buy_amount: U256,
    pub(super) max_calls: usize,
    pub(super) max_fee_bps: u128,
}

fn is_approve(call: &Call) -> bool {
    APPROVE_ENTRYPOINTS.contains(&call.entrypoint.as_str())
}

/// The verifier's `kind === "transfer"`: a transfer-family selector whose calldata decodes. The
/// decoder's opaque line is the only one starting `Call `, and a transfer's shape does not depend
/// on the wallet's known tokens.
fn is_decoded_transfer(call: &Call) -> bool {
    call.entrypoint != AVNU_MULTI_ROUTE_SWAP
        && recipient_felt_index(&call.entrypoint).is_some()
        && !call_summary(
            &call.contract_address,
            &call.entrypoint,
            &call.calldata,
            &Known::default(),
        )
        .starts_with("Call ")
}

/// `approvalCallPolicyError`: an approval only as `approve`, on the sell token, for exactly the
/// quoted amount, naming the pinned router.
fn approval_error(call: &Call, intent: &Intent) -> Option<BuildPolicyError> {
    if !is_approve(call) {
        return None;
    }
    if call.entrypoint != "approve" {
        return Some(BuildPolicyError::AdditiveAllowance);
    }
    if call.contract_address != intent.sell_token {
        return Some(BuildPolicyError::UnexpectedApprovalToken);
    }
    if call.calldata.len() != 3 || uint256_at(&call.calldata, 1) != Some(intent.sell_amount) {
        return Some(BuildPolicyError::UnexpectedAllowance);
    }
    (call.calldata[0] != intent.router).then_some(BuildPolicyError::UnexpectedSpender)
}

/// `approvalSetError`: every approval acceptable, exactly one of them, and no direct transfer.
fn approval_set_error(calls: &[Call], intent: &Intent) -> Option<BuildPolicyError> {
    if let Some(error) = calls.iter().find_map(|call| approval_error(call, intent)) {
        return Some(error);
    }
    match calls.iter().filter(|call| is_approve(call)).count() {
        0 => Some(BuildPolicyError::MissingApproval),
        1 => calls
            .iter()
            .any(is_decoded_transfer)
            .then_some(BuildPolicyError::DirectTransfer),
        _ => Some(BuildPolicyError::MultipleApprovals),
    }
}

/// `avnuRouteIntentError`: the first field of the decoded route that disagrees with the intent.
fn route_intent_error(
    route: &AvnuRoute,
    fee_bps: &Felt,
    intent: &Intent,
) -> Option<BuildPolicyError> {
    if route.sell_token != intent.sell_token {
        Some(BuildPolicyError::SellTokenMismatch)
    } else if route.sell_amount != intent.sell_amount {
        Some(BuildPolicyError::SellAmountMismatch)
    } else if route.buy_token != intent.buy_token {
        Some(BuildPolicyError::BuyTokenMismatch)
    } else if route.beneficiary != intent.taker {
        Some(BuildPolicyError::BeneficiaryMismatch)
    } else if route.min_buy_amount < intent.min_buy_amount {
        Some(BuildPolicyError::BelowMinimum)
    } else if !u128::try_from(*fee_bps).is_ok_and(|bps| bps <= intent.max_fee_bps) {
        Some(BuildPolicyError::FeeAboveCap)
    } else {
        None
    }
}

/// `buildPolicy`: the complete call set is one acceptable approval plus exactly one swap call on
/// the pinned router through `multi_route_swap`, whose calldata decodes against the pinned layout
/// (`send::avnu_route`) and matches the intent field by field. Returns the decoded route.
pub(super) fn build_policy(calls: &[Call], intent: &Intent) -> Result<AvnuRoute, BuildPolicyError> {
    if calls.len() > intent.max_calls {
        return Err(BuildPolicyError::TooManyCalls);
    }
    if let Some(error) = approval_set_error(calls, intent) {
        return Err(error);
    }
    let swaps: Vec<&Call> = calls.iter().filter(|call| !is_approve(call)).collect();
    let Some(only) = swaps.first() else {
        return Err(BuildPolicyError::NoSwapCall);
    };
    for call in &swaps {
        if call.contract_address != intent.router {
            return Err(BuildPolicyError::NotRouter);
        }
        if call.entrypoint != AVNU_MULTI_ROUTE_SWAP {
            return Err(BuildPolicyError::NotSwapEntrypoint);
        }
    }
    if swaps.len() > 1 {
        return Err(BuildPolicyError::MultipleSwapCalls);
    }
    let route = avnu_route::decode(&only.calldata).ok_or(BuildPolicyError::UndecodableRoute)?;
    let fee_bps = only
        .calldata
        .get(INTEGRATOR_FEE_BPS_INDEX)
        .ok_or(BuildPolicyError::UndecodableRoute)?;
    match route_intent_error(&route, fee_bps, intent) {
        Some(error) => Err(error),
        None => Ok(route),
    }
}

/// `reviewedMinBuyAmount`: the least the reviewed quote entitles the person to, in base units:
/// the quoted buy amount less the slippage rounded up to a basis point, less `tolerance_bps` more
/// for the aggregator's floating-point minimum. `None` for a zero quote, a slippage outside
/// (0, 1), or a tolerance that reaches the whole amount. Exact for any u256 quote.
pub fn reviewed_min_buy_amount(quoted: U256, slippage: f64, tolerance_bps: u64) -> Option<U256> {
    // A slippage of 1 or more leaves no basis point to keep, which `kept` refuses below.
    if quoted == U256::ZERO || !slippage.is_finite() || slippage <= 0.0 {
        return None;
    }
    // Positive and finite after the checks above; `as` saturates a huge value, refused below.
    let slippage_bps = (slippage * BPS_DENOMINATOR as f64).ceil() as u64;
    let tolerance = slippage_bps.checked_add(tolerance_bps)?;
    let kept = BPS_DENOMINATOR
        .checked_sub(tolerance)
        .filter(|kept| *kept > 0)?;
    // floor(q * kept / 10_000) without a 2^256 overflow: q = whole * 10_000 + rest.
    let (whole, rest) = quoted.checked_div_rem_small(BPS_DENOMINATOR)?;
    let rest = u128::from(rest) * u128::from(kept) / u128::from(BPS_DENOMINATOR);
    whole
        .checked_mul_small(kept)?
        .checked_add(U256::from_u128(rest))
}

#[cfg(test)]
#[path = "policy_tests.rs"]
mod tests;
