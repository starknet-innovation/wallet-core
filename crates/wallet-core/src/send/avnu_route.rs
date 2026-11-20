//! The pinned AVNU `multi_route_swap` calldata layout (`decodeAvnuRoute` in the extension's
//! `packages/foundation/wallet-model/src/swap/avnu-route.ts`): the sell and buy legs, the
//! floor, the beneficiary and the integrator fee, accepted only when the route spans consume the
//! calldata exactly. Anything else is not a route, and the review shows the call as opaque.

use starknet_types_core::felt::Felt;

use super::call_decode::{uint256_at, AVNU_BENEFICIARY_FELT_INDEX};
use super::u256::U256;

/// `"branch"` as a short string: a route swap that carries alternative legs.
const BRANCH_MARKER: Felt = Felt::from_hex_unchecked("0x6272616e6368");
const SELL_TOKEN: usize = 0;
const SELL_AMOUNT: usize = 1;
const BUY_TOKEN: usize = 3;
const BUY_AMOUNT: usize = 4;
const MIN_BUY_AMOUNT: usize = 6;
const ROUTES_LENGTH: usize = 11;
const ROUTES_START: usize = 12;

/// `AvnuRoute`, the fields the review reads. The quoted buy amount is checked but not kept, and
/// the integrator fee felts are always present felts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvnuRoute {
    pub sell_token: Felt,
    pub sell_amount: U256,
    pub buy_token: Felt,
    pub min_buy_amount: U256,
    pub beneficiary: Felt,
}

/// `spanLength`: a length felt no longer than the calldata itself.
fn span_length(calldata: &[Felt], offset: usize) -> Option<usize> {
    usize::try_from(*calldata.get(offset)?)
        .ok()
        .filter(|length| *length <= calldata.len())
}

fn skip_felt_span(calldata: &[Felt], offset: usize) -> Option<usize> {
    let end = offset + 1 + span_length(calldata, offset)?;
    (end <= calldata.len()).then_some(end)
}

fn skip_each(
    calldata: &[Felt],
    offset: usize,
    count: usize,
    skip: fn(&[Felt], usize) -> Option<usize>,
) -> Option<usize> {
    (0..count).try_fold(offset, |cursor, _| skip(calldata, cursor))
}

fn skip_swap_leg(calldata: &[Felt], offset: usize) -> Option<usize> {
    skip_felt_span(calldata, offset + 2)
}

fn skip_route_swap(calldata: &[Felt], offset: usize) -> Option<usize> {
    if *calldata.get(offset)? != BRANCH_MARKER {
        return skip_swap_leg(calldata, offset);
    }
    let principal_end = skip_swap_leg(calldata, offset + 1)?;
    let alternatives = span_length(calldata, principal_end)?;
    skip_each(calldata, principal_end + 1, alternatives, skip_swap_leg)
}

fn skip_route(calldata: &[Felt], offset: usize) -> Option<usize> {
    skip_route_swap(calldata, offset + 2)
}

fn routes_end(calldata: &[Felt]) -> Option<usize> {
    let count = span_length(calldata, ROUTES_LENGTH).filter(|count| *count >= 1)?;
    skip_each(calldata, ROUTES_START, count, skip_route)
}

/// `decodeAvnuRoute` for `multi_route_swap` calldata.
pub fn decode(calldata: &[Felt]) -> Option<AvnuRoute> {
    if calldata.len() < ROUTES_START
        || uint256_at(calldata, BUY_AMOUNT).is_none()
        || routes_end(calldata)? != calldata.len()
    {
        return None;
    }
    Some(AvnuRoute {
        sell_token: calldata[SELL_TOKEN],
        sell_amount: uint256_at(calldata, SELL_AMOUNT)?,
        buy_token: calldata[BUY_TOKEN],
        min_buy_amount: uint256_at(calldata, MIN_BUY_AMOUNT)?,
        beneficiary: calldata[AVNU_BENEFICIARY_FELT_INDEX],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn felts(values: &[u64]) -> Vec<Felt> {
        values.iter().map(|value| Felt::from(*value)).collect()
    }

    /// Twelve header felts, one route of `[from, to, exchange, percent, len, ..params]`.
    fn route(params: &[u64]) -> Vec<Felt> {
        let mut calldata = felts(&[1, 5, 0, 2, 9, 0, 8, 0, 3, 0, 4, 1, 6, 7, 10, 11]);
        calldata.push(Felt::from(params.len()));
        calldata.extend(felts(params));
        calldata
    }

    #[test]
    fn a_route_must_consume_the_calldata_exactly() {
        let decoded = decode(&route(&[42])).unwrap();
        assert_eq!(decoded.sell_token, Felt::ONE);
        assert_eq!(decoded.sell_amount, U256::from_u128(5));
        assert_eq!(decoded.min_buy_amount, U256::from_u128(8));
        assert_eq!(decoded.beneficiary, Felt::from(3u8));
        let mut trailing = route(&[42]);
        trailing.push(Felt::ONE);
        assert_eq!(decode(&trailing), None);
        assert_eq!(decode(&route(&[42])[..15]), None);
    }

    #[test]
    fn a_branch_carries_its_alternative_legs() {
        // Route header, then the branch marker, one principal leg and one alternative.
        let mut calldata = felts(&[1, 5, 0, 2, 9, 0, 8, 0, 3, 0, 4, 1, 6, 7]);
        calldata.push(BRANCH_MARKER);
        calldata.extend(felts(&[10, 11, 0, 1, 20, 21, 1, 99]));
        assert!(decode(&calldata).is_some());
        calldata.pop();
        assert_eq!(decode(&calldata), None);
    }

    #[test]
    fn no_routes_and_wide_limbs_are_refused() {
        let mut none = felts(&[1, 5, 0, 2, 9, 0, 8, 0, 3, 0, 4, 0]);
        assert_eq!(decode(&none), None);
        none[11] = Felt::ONE;
        assert_eq!(decode(&none), None);
        let mut wide = route(&[]);
        wide[1] = Felt::from(u128::MAX) + Felt::ONE;
        assert_eq!(decode(&wide), None);
    }
}
