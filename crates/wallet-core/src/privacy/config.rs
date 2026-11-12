//! STRK20 routing and limits as plain records. This crate bundles none: a wallet renders its own
//! (Bramble's `generated/privacy.json`, from the extension's `DEFAULT_STRK20_NETWORKS`,
//! `STRK20_LIMITS`, `STRK20_WAIT_BUDGETS` and `MAX_SERVICE_ATTEMPTS`), parses and validates it,
//! and passes the records in, as it does its network catalog (`crate::catalog`).
//!
//! The one rule kept here as well as in the wallet's loader: a network with OHTTP on is **not
//! configured**. This core has no OHTTP client, so it never puts the viewing key in a plaintext
//! body for such a network, whatever routing it is handed (Bramble's `mobile/docs/privacy.md`
//! §1.1).

use crate::{Felt, NetworkId};

/// One network's privacy routing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub network: NetworkId,
    /// The extension's `ohttp` flag. A route with it on is never used.
    pub ohttp: bool,
    /// `https://…`, no trailing slash; the SDK appends `/v1/sync/…`.
    pub discovery_url: String,
    /// As configured (the extension's `starknet_call` sends this spelling).
    pub pool_address: String,
    pub pool: Felt,
}

/// The extension's limits the judges enforce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    pub balances: usize,
    pub discovered_notes: usize,
    pub max_pool_fee: u128,
    pub max_service_attempts: u32,
    pub tokens: usize,
    pub discovery_budget_ms: u64,
}

/// The extension's bound on a service URL (`isServiceUrl` in `@bramble/core/service-url`).
pub const MAX_SERVICE_URL_BYTES: usize = 2048;

/// Whether `url` is a service URL the viewing key may be posted to (`isServiceUrl`): `https`
/// only, printable ASCII, no trailing slash, and no query, fragment, credentials or backslash.
/// A wallet's loader checks this when it reads its routing; the plan checks it again before it
/// will build a body, so no routing it is handed can send the key over plaintext.
pub fn is_service_url(url: &str) -> bool {
    url.len() <= MAX_SERVICE_URL_BYTES
        && url.strip_prefix("https://").is_some_and(|rest| {
            !rest.is_empty()
                && !url.ends_with('/')
                && url.bytes().all(|b| b.is_ascii_graphic())
                && !url.contains(['?', '#', '@', '\\'])
        })
}

/// The plain (non-OHTTP) route for `network`, if `routes` has one.
pub fn route_for(routes: &[Route], network: NetworkId) -> Option<&Route> {
    routes
        .iter()
        .find(|route| route.network == network)
        .filter(|route| !route.ohttp)
}

/// Bramble's routing and limits for this crate's unit tests (`generated/privacy.json` at the
/// fixtures' commit); `bramble-core`'s tests read the file itself.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    pub(crate) const POOL: &str =
        "0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a";

    pub(crate) fn limits() -> Limits {
        Limits {
            balances: 64,
            discovered_notes: 4096,
            max_pool_fee: u128::MAX,
            max_service_attempts: 3,
            tokens: 64,
            discovery_budget_ms: 60_000,
        }
    }

    pub(crate) fn mainnet(ohttp: bool) -> Route {
        Route {
            network: NetworkId::Mainnet,
            ohttp,
            discovery_url: "https://d.example/discovery".into(),
            pool_address: POOL.into(),
            pool: Felt::from_hex(POOL).unwrap(),
        }
    }

    pub(crate) fn routes() -> Vec<Route> {
        vec![mainnet(false)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ohttp_network_is_never_routed() {
        assert!(route_for(&testing::routes(), NetworkId::Mainnet).is_some());
        assert!(route_for(&testing::routes(), NetworkId::Sepolia).is_none());
        // Even with a URL and a pool in place, `ohttp: true` refuses it.
        assert!(route_for(&[testing::mainnet(true)], NetworkId::Mainnet).is_none());
    }

    #[test]
    fn the_key_is_only_posted_to_an_https_service_url() {
        assert!(is_service_url("https://d.example/discovery"));
        for refused in [
            "http://d.example/discovery",
            "https://",
            "https://d.example/",
            "https://d.example/?x=1",
            "https://user@d.example",
            "https://d.example/#x",
            "https://d.example\\x",
            "https://d.example/a b",
            "ws://d.example",
            "",
        ] {
            assert!(!is_service_url(refused), "{refused}");
        }
    }
}
