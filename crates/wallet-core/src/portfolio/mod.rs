//! Portfolio: fiat prices and charts, tracked tokens, on-chain token reads and saved balances,
//! each a sans-IO port of the extension's TypeScript (see Bramble's
//! `mobile/docs/portfolio-activity.md`). Catalogs (`crate::catalog`) are arguments; the stores
//! (`PriceBook`, `BalanceSnapshots`, `TokenPreferences`) are plain values a wallet wraps as it
//! needs, and the user-facing copy that carries no rule stays with the wallet.

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

/// Schema tag of every persisted portfolio/activity document (`preferences.json`, `prices.json`,
/// `snapshots.json`, `activity.json`). A later change bumps it and adds one adjacent migration.
pub const SCHEMA: &str = "bramble.mobile.portfolio.v1";

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
