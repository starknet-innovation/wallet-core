//! Network and token catalogs as plain records. This crate bundles none: a wallet ships its own
//! (Bramble renders them from the extension into `generated/networks.json` and
//! `generated/token-registry.json`), parses and validates them, and passes them in. Every part of
//! the core that needs a chain's tokens, explorer or curated registry takes these records as an
//! argument.

use crate::NetworkId;

/// An ERC-20 a network's wallet home queries by default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenInfo {
    pub address: String,
    pub decimals: u8,
    pub name: String,
    pub symbol: String,
}

/// Public network metadata. RPC endpoints are tried in order; the host verifies
/// `starknet_chainId` against `chain_id` before trusting any response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkInfo {
    pub network: NetworkId,
    pub chain_id: String,
    pub explorer_base_url: String,
    pub name: String,
    pub rpc_urls: Vec<String>,
    pub tokens: Vec<TokenInfo>,
}

/// One curated registry token (`knownTokensForNetwork`'s source), on the network its
/// `network_id` names in the extension's spelling (`SN_MAIN`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryToken {
    pub address: String,
    pub decimals: u8,
    pub name: String,
    pub network_id: String,
    pub symbol: String,
}

/// A registry symbol's allow-listed second deployments (`LEGITIMATE_ALIASES`): contracts the spam
/// heuristic never reads as a spoof of `symbol` (Bramble's: bridged USDC.e for USDC).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenAlias {
    pub symbol: String,
    pub addresses: Vec<String>,
}

/// The curated token registry, the addresses (64-digit spelling) the wallet bundles a logo for,
/// and the allow-listed alternate deployments of its symbols.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TokenRegistry {
    pub tokens: Vec<RegistryToken>,
    pub logos: Vec<String>,
    pub aliases: Vec<TokenAlias>,
}

/// Both catalogs, borrowed, for the code that needs the networks and the registry together.
#[derive(Debug, Clone, Copy)]
pub struct Catalogs<'a> {
    pub networks: &'a [NetworkInfo],
    pub registry: &'a TokenRegistry,
}

/// Metadata for one network, when the catalog holds it.
pub fn network_info(networks: &[NetworkInfo], network: NetworkId) -> Option<&NetworkInfo> {
    networks.iter().find(|info| info.network == network)
}

/// A small catalog for this crate's unit tests: mainnet ETH and STRK, Sepolia STRK, and a
/// registry of mainnet ETH, STRK and USDC with STRK's logo. Addresses are Bramble's; the full
/// catalogs are exercised by `bramble-core`'s parity tests.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    pub(crate) const ETH: &str =
        "0x049d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7";
    pub(crate) const STRK: &str =
        "0x04718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d";
    const USDC: &str = "0x033068f6539f8e6e6b131e6b2b814e6c34a5224bc66947c47dab9dfee93b35fb";

    fn token(address: &str, symbol: &str, name: &str, decimals: u8) -> TokenInfo {
        TokenInfo {
            address: address.into(),
            decimals,
            name: name.into(),
            symbol: symbol.into(),
        }
    }

    fn network(network: NetworkId, tokens: Vec<TokenInfo>) -> NetworkInfo {
        NetworkInfo {
            network,
            chain_id: crate::felt::canonical(&network.chain_id()),
            explorer_base_url: "https://starkscan.co".into(),
            name: network.catalog_id().into(),
            rpc_urls: Vec::new(),
            tokens,
        }
    }

    pub(crate) fn networks() -> Vec<NetworkInfo> {
        vec![
            network(
                NetworkId::Mainnet,
                vec![
                    token(ETH, "ETH", "Ether", 18),
                    token(STRK, "STRK", "Starknet Token", 18),
                ],
            ),
            network(
                NetworkId::Sepolia,
                vec![token(STRK, "STRK", "Starknet Token", 18)],
            ),
        ]
    }

    pub(crate) fn registry() -> TokenRegistry {
        let entry = |address: &str, symbol: &str, name: &str, decimals: u8| RegistryToken {
            address: address.into(),
            decimals,
            name: name.into(),
            network_id: "SN_MAIN".into(),
            symbol: symbol.into(),
        };
        TokenRegistry {
            tokens: vec![
                entry(ETH, "ETH", "Ether", 18),
                entry(STRK, "STRK", "Starknet Token", 18),
                entry(USDC, "USDC", "USD Coin", 6),
            ],
            logos: vec![STRK.into()],
            aliases: Vec::new(),
        }
    }
}
