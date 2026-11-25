//! Shared fixtures, and `gasless-route.test.ts`'s route, ceiling and account-class cases. The
//! policy restates Bramble's numbers (its own copy is `bramble-core`'s `GASLESS_POLICY`).

use super::*;

pub(crate) const MAINNET_URL: &str = "https://wallet.nodes.starknet.org/api/paymaster/mainnet";

const ROUTES: [GaslessRoute; 2] = [
    GaslessRoute {
        network: NetworkId::Mainnet,
        url: MAINNET_URL,
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

pub(crate) const POLICY: GaslessPolicy = GaslessPolicy {
    routes: &ROUTES,
    max_window_seconds: 600,
    sign_window_seconds: 10,
    ceiling_numerator: 7,
    ceiling_denominator: 2,
    max_offers: 3,
    screening_retries: 2,
    deadline_ms: 20_000,
    max_body_bytes: 1024 * 1024,
    reference_currency: crate::portfolio::model::DisplayCurrency::Usd,
    price_scale: 1_000_000,
};

pub(crate) fn felt_of(text: &str) -> Felt {
    crate::felt::parse(text).unwrap()
}

pub(crate) fn call(to: &str, entrypoint: &str, calldata: &[&str]) -> crate::send::calls::Call {
    crate::send::calls::Call {
        contract_address: felt_of(to),
        entrypoint: entrypoint.into(),
        calldata: calldata.iter().map(|felt| felt_of(felt)).collect(),
    }
}

const ARGENT_04: &str = "0x36078334509b514626504edc9fb252328d1a240e4e948bef8d0c08dff45927f";
const ARGENT_030: &str = "0x1a736d6ed154502257f02b1ccdf4d9d1089f80811cd6acad48e6b6a9d1f2003";
const OZ: &str = "0x1d1777db36cdd06dd62cfde77b1b6ae06412af95d57a13dc40ac77b8a702381";

pub(crate) fn v2_classes() -> Vec<(AccountFamily, Felt)> {
    vec![
        (AccountFamily::Argent, felt_of(ARGENT_04)),
        (AccountFamily::OpenZeppelin, felt_of(OZ)),
    ]
}

#[test]
fn only_a_switched_on_route_with_a_pinned_forwarder_is_ready() {
    let (route, forwarder) = POLICY.ready_route(NetworkId::Mainnet).unwrap();
    assert_eq!(route.url, MAINNET_URL);
    assert_eq!(
        crate::felt::canonical(&forwarder),
        "0x127021a1b5a52d3174c2ab077c2b043c80369250d29428cee956d76ee51584f"
    );
    assert!(POLICY.ready_route(NetworkId::Sepolia).is_none());
    const PINNED_OFF: [GaslessRoute; 1] = [GaslessRoute {
        enabled: false,
        ..ROUTES[0]
    }];
    let off = GaslessPolicy {
        routes: &PINNED_OFF,
        ..POLICY
    };
    assert!(off.ready_route(NetworkId::Mainnet).is_none());
}

#[test]
fn the_ceiling_is_seven_halves_of_the_reference_rounded_up() {
    let ceiling = |value: u128| POLICY.fee_ceiling(U256::from_u128(value));
    assert_eq!(ceiling(1000), U256::from_u128(3500));
    assert_eq!(ceiling(1), U256::from_u128(4));
    assert_eq!(ceiling(0), U256::ZERO);
    let huge = U256 {
        high: u128::MAX / 3,
        low: 0,
    };
    assert_eq!(
        POLICY.fee_ceiling(huge),
        U256 {
            high: u128::MAX,
            low: u128::MAX
        }
    );
}

#[test]
fn argent_0_4_and_openzeppelin_sign_v2_and_argent_0_3_and_braavos_do_not() {
    let classes = v2_classes();
    let signs = |family, class: &str| signs_gasless_v2(family, &felt_of(class), &classes);
    assert!(signs(AccountFamily::Argent, ARGENT_04));
    assert!(signs(AccountFamily::OpenZeppelin, OZ));
    assert!(!signs(AccountFamily::Argent, ARGENT_030));
    assert!(!signs(AccountFamily::Argent, OZ));
    let braavos = [(AccountFamily::Braavos, felt_of(ARGENT_04))];
    assert!(!signs_gasless_v2(
        AccountFamily::Braavos,
        &felt_of(ARGENT_04),
        &braavos
    ));
}

#[test]
fn the_window_must_be_live_keep_its_floor_and_end_within_the_cap() {
    let live = |after, before, now| POLICY.window_is_live(after, before, now, 10);
    assert!(live(1, 1000, 500));
    assert!(!live(500, 1000, 500), "not yet open");
    assert!(!live(1, 1000, 1000), "closed");
    assert!(!live(1, 1000, 991), "under the floor");
    assert!(live(1, 1000, 990));
    assert!(!live(1, 1000, 399), "longer than the cap");
    assert!(live(1, 1000, 400));
    // Closed is closed even with no floor asked for.
    assert!(!POLICY.window_is_live(1, 1000, 1000, 0));
    assert!(!POLICY.window_is_live(1, 1000, 1001, 0));
    // A clock past `Number.MAX_SAFE_INTEGER` is refused even inside an otherwise live window.
    assert!(live(
        MAX_SAFE_INTEGER - 1,
        MAX_SAFE_INTEGER + 100,
        MAX_SAFE_INTEGER
    ));
    assert!(!live(
        MAX_SAFE_INTEGER,
        MAX_SAFE_INTEGER + 101,
        MAX_SAFE_INTEGER + 1
    ));
}

#[test]
fn every_refusal_reads_as_the_extensions_sentence() {
    assert_eq!(
        GaslessCode::ScreeningSubjectUnverifiable.to_string(),
        "Deploying on Mainnet needs STRK."
    );
    assert_eq!(
        GaslessCode::from_gateway("upstream_rejected"),
        Some(GaslessCode::UpstreamRejected)
    );
    assert_eq!(GaslessCode::from_gateway("unrecognised"), None);
}
