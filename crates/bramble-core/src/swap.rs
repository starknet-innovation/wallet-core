//! Bramble's swap choices, for `starknet_wallet_core::swap`: the AVNU deployments it pins
//! (`AVNU_DEPLOYMENTS` in `transactions/src/swap/avnu.ts`), the slippage `readSwapPrepareInput`
//! defaults to and bounds (`swap-inputs.ts`), the route's fee cap and rounding tolerance
//! (`AVNU_MAX_INTEGRATOR_FEE_BPS`, `SLIPPAGE_ROUNDING_TOLERANCE_BPS` in
//! `wallet-model/src/swap/avnu-route.ts`) and the build bounds (`MAX_SWAP_CALLS`,
//! `AVNU_MAX_CALLDATA_LENGTH`, `readQuote`'s quote id length).

use starknet_wallet_core::swap::{AvnuDeployment, SwapPolicy};
use starknet_wallet_core::{Felt, NetworkId};

/// AVNU's published exchange contracts. A trust anchor: a build may approve and call only the
/// router pinned for its network, so changing one is a security review, not a config change.
const AVNU_DEPLOYMENTS: [AvnuDeployment; 2] = [
    AvnuDeployment {
        network: NetworkId::Mainnet,
        base_url: "https://starknet.api.avnu.fi",
        router: Felt::from_hex_unchecked(
            "0x04270219d365d6b017231b52e92b3fb5d7c8378b05e9abc97724537a80e93b0f",
        ),
    },
    AvnuDeployment {
        network: NetworkId::Sepolia,
        base_url: "https://sepolia.api.avnu.fi",
        router: Felt::from_hex_unchecked(
            "0x02c56e8b00dbe2a71e57472685378fc8988bba947e9a99b26a00fade2b4fe7c2",
        ),
    },
];

pub const SWAP_POLICY: SwapPolicy = SwapPolicy {
    deployments: &AVNU_DEPLOYMENTS,
    default_slippage: 0.005,
    max_slippage: 0.05,
    slippage_rounding_tolerance_bps: 1,
    max_integrator_fee_bps: 100,
    max_build_calls: 4,
    max_call_calldata: 2_000,
    max_quote_id_length: 120,
};

// A build's calls must fit the batch the plan accepts, checked when the crate builds.
const _: () = assert!(
    SWAP_POLICY.max_build_calls <= crate::MAX_INVOKE_CALLS
        && SWAP_POLICY.max_call_calldata <= crate::MAX_CALL_CALLDATA_FELTS
);

#[cfg(test)]
mod tests {
    use super::*;

    // `swap-build-policy.test.ts` "the pinned mainnet counterparty is AVNU's published exchange
    // contract": restated so an edit to the pin is a visible change.
    #[test]
    fn the_pinned_routers_are_avnus_published_exchanges() {
        let router = |network| {
            SWAP_POLICY
                .deployments
                .iter()
                .find(|deployment| deployment.network == network)
                .map(|deployment| starknet_wallet_core::felt::canonical(&deployment.router))
        };
        assert_eq!(
            router(NetworkId::Mainnet).as_deref(),
            Some("0x4270219d365d6b017231b52e92b3fb5d7c8378b05e9abc97724537a80e93b0f")
        );
        assert_eq!(
            router(NetworkId::Sepolia).as_deref(),
            Some("0x2c56e8b00dbe2a71e57472685378fc8988bba947e9a99b26a00fade2b4fe7c2")
        );
    }
}
