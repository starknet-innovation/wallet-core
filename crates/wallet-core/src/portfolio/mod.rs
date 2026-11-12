//! Portfolio: fiat prices and charts, tracked tokens, on-chain token reads and saved balances,
//! each a sans-IO port of the extension's TypeScript (see Bramble's
//! `mobile/docs/portfolio-activity.md`). Catalogs (`crate::catalog`) and the wallet's choices
//! (`PortfolioPolicy`) are arguments; the stores (`PriceBook`, `BalanceSnapshots`,
//! `TokenPreferences`) are plain values a wallet wraps as it needs, and the user-facing copy that
//! carries no rule stays with the wallet.

pub mod fiat;
pub mod model;
pub(crate) mod ordered;
mod price_cache;
pub mod prices;
pub mod registry;
pub mod snapshots;
pub mod token_meta;
pub mod tokens;
pub mod wire;

use crate::NetworkId;

/// A wallet's portfolio choices, given to each store it builds (Bramble's: `bramble-core`'s
/// `PORTFOLIO_POLICY`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortfolioPolicy {
    /// Schema tag of every persisted portfolio document (`preferences.json`, `prices.json`,
    /// `snapshots.json`). A document under another tag reads as empty (preferences: as defaults,
    /// never overwritten). A later change bumps it and adds one adjacent migration.
    pub schema: &'static str,
    /// The CoinGecko API base (`https://…/api/v3`) the price and chart requests are built on.
    pub price_api_base: &'static str,
    /// The one network with fiat prices; every other network is served none. One, because the
    /// price cache is keyed by address alone and ETH and STRK share their addresses across
    /// networks (Sherlock 185).
    pub price_network: NetworkId,
}

/// A policy for this crate's unit tests; `bramble-core`'s tests run Bramble's.
#[cfg(test)]
pub(crate) const TEST_POLICY: PortfolioPolicy = PortfolioPolicy {
    schema: "wallet.portfolio.v1",
    price_api_base: "https://api.coingecko.com/api/v3",
    price_network: NetworkId::Mainnet,
};

#[cfg(test)]
mod tests {
    // Mobile's `portfolio_and_history_never_touch_key_material` (`tests/portfolio_state.rs`), over
    // its files and the activity log's.
    #[test]
    fn portfolio_and_history_never_touch_key_material() {
        let sources = [
            include_str!("fiat.rs"),
            include_str!("prices.rs"),
            include_str!("price_cache.rs"),
            include_str!("registry.rs"),
            include_str!("snapshots.rs"),
            include_str!("token_meta.rs"),
            include_str!("tokens.rs"),
            include_str!("wire.rs"),
            include_str!("../history/activity.rs"),
            include_str!("../history/activity_book.rs"),
            include_str!("../history/activity_rules.rs"),
            include_str!("../history/appearance.rs"),
            include_str!("../history/classify.rs"),
            include_str!("../history/events.rs"),
            include_str!("../history/events/dating.rs"),
            include_str!("../history/explorer.rs"),
            include_str!("../history/starkscan.rs"),
            include_str!("../activity/feed.rs"),
            include_str!("../activity/observation.rs"),
            include_str!("../activity/store.rs"),
        ];
        for source in sources {
            for forbidden in [
                "WalletCore",
                "crate::vault",
                "crate::kms",
                "crate::wallet",
                "Zeroizing",
                "mnemonic",
                "private_key",
                "viewing_key",
            ] {
                assert!(
                    !source.contains(forbidden),
                    "portfolio/history code must not reference {forbidden}"
                );
            }
        }
    }
}
