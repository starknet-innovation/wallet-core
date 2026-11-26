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
///
/// The host must be present and spelled as the extension's URL parser serializes it: lowercase,
/// no IPv6 literal (refused here, stricter than the extension), and a port, if any, in decimal
/// without leading zeros, in range, and not the default 443.
pub fn is_service_url(url: &str) -> bool {
    url.len() <= MAX_SERVICE_URL_BYTES
        && url.strip_prefix("https://").is_some_and(|rest| {
            !url.ends_with('/')
                && url.bytes().all(|b| b.is_ascii_graphic())
                && !url.contains(['?', '#', '@', '\\'])
                && is_canonical_authority(rest.split('/').next().unwrap_or_default())
        })
}

fn is_canonical_authority(authority: &str) -> bool {
    let (host, port) = match authority.split_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    };
    !host.is_empty()
        && !host
            .bytes()
            .any(|b| b.is_ascii_uppercase() || b == b'[' || b == b']')
        && port.is_none_or(|port| {
            port.bytes().all(|b| b.is_ascii_digit())
                && (port == "0" || !port.starts_with('0'))
                && port.parse::<u16>().is_ok_and(|port| port != 443)
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

    /// The extension's `STRK20_LIMITS` the engine judge reads.
    pub(crate) fn proof_limits() -> crate::privacy::engine::ProofLimits {
        crate::privacy::engine::ProofLimits::CEILING
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
        assert!(is_service_url("https://d.example:8443/discovery"));
        assert!(is_service_url("https://d.example"));
        for refused in [
            "http://d.example/discovery",
            "https://",
            // No host (the extension's parser reads `https:///x` as host `x`, which is not
            // this spelling, and `https://:8443/x` as no URL at all).
            "https:///x",
            "https://:8443/x",
            // A port the parser would drop or respell, or refuse.
            "https://d.example:443/x",
            "https://d.example:/x",
            "https://d.example:08443/x",
            "https://d.example:65536/x",
            "https://d.example:84a3/x",
            "https://d.example:1:2/x",
            // A host the parser would lowercase, and IPv6 literals (refused outright).
            "https://D.example/x",
            "https://[::1]/x",
            "https://user:pw@d.example/x",
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
