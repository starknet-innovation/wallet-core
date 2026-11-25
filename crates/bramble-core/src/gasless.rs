//! Bramble's gasless-fee choices, for `starknet_wallet_core::gasless`. Plain Rust until the swap:
//! no UniFFI export reads them yet.

use crate::catalogs::accounts::catalog;
use starknet_wallet_core::accounts::AccountFamily;
use starknet_wallet_core::gasless::{GaslessPolicy, GaslessRoute};
use starknet_wallet_core::portfolio::model::DisplayCurrency;
use starknet_wallet_core::{Felt, NetworkId};

/// `GASLESS_PAYMASTER_URLS`, `GASLESS_FEES_ENABLED` and `GASLESS_FORWARDER_ADDRESS`
/// (`transactions/src/gasless/gasless-config.ts`): the wallet gateway's routes, not AVNU's and not
/// the STRK20 paymaster. Mainnet is on (a product decision of 2026-10-28; the independent
/// security review is pending) with the forwarder its backend owner confirmed. Sepolia has no
/// known forwarder, so it stays off and pays in STRK. The forwarder is a trust anchor: every
/// execution the wallet signs is bound to it and pays its fee to it.
const ROUTES: [GaslessRoute; 2] = [
    GaslessRoute {
        network: NetworkId::Mainnet,
        url: "https://wallet.nodes.starknet.org/api/paymaster/mainnet",
        enabled: true,
        forwarder: Some(Felt::from_hex_unchecked(
            "0x127021a1b5a52d3174c2ab077c2b043c80369250d29428cee956d76ee51584f",
        )),
    },
    GaslessRoute {
        network: NetworkId::Sepolia,
        url: "https://wallet.nodes.starknet.org/api/paymaster/sepolia",
        enabled: false,
        forwarder: None,
    },
];

/// `GASLESS_MAX_WINDOW_SECONDS`, `GASLESS_SIGN_WINDOW_SECONDS`, the 7/2 margin
/// (`GASLESS_FEE_CEILING_NUMERATOR`/`DENOMINATOR`), `MAX_GASLESS_OFFERS`,
/// `GASLESS_SCREENING_RETRIES`, the shared paymaster deadline and reply bound
/// (`PAYMASTER_TIMEOUT_MS`, `PAYMASTER_MAX_RESPONSE_BYTES`), and the reference's pinned currency
/// and price scale (`REFERENCE_CURRENCY`, `PRICE_SCALE` in `strk20-fee-reference.ts`: 1e6 keeps
/// six significant figures on a sub-dollar token and a five-figure one inside the safe range).
pub const GASLESS_POLICY: GaslessPolicy = GaslessPolicy {
    routes: &ROUTES,
    max_window_seconds: 600,
    sign_window_seconds: 10,
    ceiling_numerator: 7,
    ceiling_denominator: 2,
    max_offers: 3,
    screening_retries: 2,
    deadline_ms: 20_000,
    max_body_bytes: 1024 * 1024,
    reference_currency: DisplayCurrency::Usd,
    price_scale: 1_000_000,
};

/// `accountSignsGaslessV2`'s classes: Argent 0.4 (Bramble's and Xverse's builds) and Bramble's
/// OpenZeppelin class. Argent 0.3.x is not verified for v2, and Braavos and multisig never are.
pub fn gasless_v2_classes() -> Vec<(AccountFamily, Felt)> {
    let catalog = catalog();
    vec![
        (AccountFamily::Argent, catalog.argent_default),
        (AccountFamily::Argent, catalog.argent_xverse),
        (AccountFamily::OpenZeppelin, catalog.open_zeppelin_class),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use starknet_wallet_core::felt;

    // `gasless-route.test.ts`: the pins, restated so an edit to one is a visible change.
    #[test]
    fn only_mainnet_is_ready_through_the_wallet_gateways_pinned_forwarder() {
        let (route, forwarder) = GASLESS_POLICY.ready_route(NetworkId::Mainnet).unwrap();
        assert_eq!(
            route.url,
            "https://wallet.nodes.starknet.org/api/paymaster/mainnet"
        );
        assert_eq!(
            felt::canonical(&forwarder),
            "0x127021a1b5a52d3174c2ab077c2b043c80369250d29428cee956d76ee51584f"
        );
        assert!(GASLESS_POLICY.ready_route(NetworkId::Sepolia).is_none());
        for route in GASLESS_POLICY.routes {
            assert!(!route.url.contains("avnu.fi") && !route.url.contains("/api/strk20"));
        }
    }

    #[test]
    fn the_v2_classes_are_argent_0_4_and_openzeppelin() {
        let classes: Vec<(AccountFamily, String)> = gasless_v2_classes()
            .iter()
            .map(|(family, class)| (*family, felt::canonical(class)))
            .collect();
        assert_eq!(
            classes,
            [
                (
                    AccountFamily::Argent,
                    "0x36078334509b514626504edc9fb252328d1a240e4e948bef8d0c08dff45927f".into()
                ),
                (
                    AccountFamily::Argent,
                    "0x663fc01a0dbe1bacc4cd2a4c856eb9784b255a20988aa33d4d52b6fc20bd024".into()
                ),
                (
                    AccountFamily::OpenZeppelin,
                    "0x1d1777db36cdd06dd62cfde77b1b6ae06412af95d57a13dc40ac77b8a702381".into()
                ),
            ]
        );
    }
}
