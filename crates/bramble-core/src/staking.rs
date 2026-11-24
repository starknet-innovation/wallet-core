//! Bramble's staking choices, for `starknet_wallet_core::staking`; mobile's exports over them are
//! `ffi_staking` and `ffi_staking_action`.

use starknet_wallet_core::staking::{StakingNetwork, StakingPolicy};
use starknet_wallet_core::NetworkId;

/// `STAKING_NETWORKS` (`transactions/src/staking/staking-networks.ts`: the staking contracts,
/// checked by the extension against docs.starknet.io and a live read on 2026-11-04; on for
/// Mainnet only, a product decision of 2026-11-06), Starkscan's directory and positions on
/// Mainnet only (`staking-directory-ops.ts`, `staking-positions-ops.ts`),
/// `STAKING_DIRECTORY_PAGE_SIZE`, `MAX_CHECKED_POSITIONS`, `MAX_CATCHING_UP_LAG_BLOCKS` and
/// `MAX_STAKING_PREPARATIONS`. The rewards token a read checks is the network catalog's STRK.
pub const STAKING_POLICY: StakingPolicy = StakingPolicy {
    networks: &[
        StakingNetwork {
            network: NetworkId::Mainnet,
            enabled: true,
            staking_contract: "0x00ca1702e64c81d9a07b86bd2c540188d92a2c73cf5cc0e508d949015e7e84a7",
            starkscan: true,
        },
        StakingNetwork {
            network: NetworkId::Sepolia,
            enabled: false,
            staking_contract: "0x03745ab04a431fc02871a139be6b93d9260b0ff3e779ad9c8b377183b23109f1",
            starkscan: false,
        },
    ],
    directory_page_size: 50,
    max_checked_positions: 16,
    max_catching_up_lag_blocks: 100,
    max_preparations: 20,
};

#[cfg(test)]
mod tests {
    use super::*;

    // `staking-calls.test.ts` "ships switched on for Mainnet only (paste-only), off elsewhere".
    #[test]
    fn staking_ships_switched_on_for_mainnet_only() {
        let mainnet = STAKING_POLICY.network(NetworkId::Mainnet).unwrap();
        assert_eq!(
            mainnet.staking_contract,
            "0x00ca1702e64c81d9a07b86bd2c540188d92a2c73cf5cc0e508d949015e7e84a7"
        );
        assert!(mainnet.starkscan);
        assert_eq!(STAKING_POLICY.network(NetworkId::Sepolia), None);
        // The rewards token a staking read checks is the catalog's STRK, on both networks.
        for network in [NetworkId::Mainnet, NetworkId::Sepolia] {
            let strk = crate::catalogs::networks::info(network)
                .tokens
                .iter()
                .find(|token| token.symbol == "STRK")
                .unwrap();
            assert_eq!(
                strk.address,
                "0x04718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d"
            );
        }
    }
}
