//! Stateful behaviour of the portfolio objects that the parity fixture cannot pin on its own:
//! persistence and schema guards, the balance-snapshot cadence and fiat formatting bounds, with
//! Bramble's catalogs as the arguments. Mobile's activity-book, history-cadence and Starkscan
//! cases follow with history (plan step 2.4c); its key-boundary check is
//! `starknet_wallet_core::portfolio`'s own unit test, over the core's sources.

mod portfolio_support;

use portfolio_support::catalogs;
use starknet_wallet_core::history::explorer::canonical_account_id;
use starknet_wallet_core::portfolio::fiat::format_fiat;
use starknet_wallet_core::portfolio::model::{DisplayCurrency, TokenBalance, TokenConfig};
use starknet_wallet_core::portfolio::prices::{PriceBook, PricePlan};
use starknet_wallet_core::portfolio::snapshots::{
    network_data_issue, BalanceHealth, BalancePlan, BalanceSnapshots,
};
use starknet_wallet_core::portfolio::tokens::TokenPreferences;
use starknet_wallet_core::NetworkId;

const ACCOUNT: &str = "0x208361e3911b1d93a704400b88e7e53a89171fce64dbb48f8479e4f44b398d9";
const STRK: &str = "0x04718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d";

// Mobile's `activity_account_id(NetworkId::Mainnet, ACCOUNT)`, which is this.
fn account_id() -> String {
    canonical_account_id(&format!("SN_MAIN:{ACCOUNT}")).unwrap()
}

fn tracked() -> Vec<TokenConfig> {
    TokenPreferences::new(None, catalogs())
        .tracked(NetworkId::Mainnet)
        .into_iter()
        .map(|t| t.token)
        .collect()
}

#[test]
fn caches_reject_unknown_or_tampered_documents() {
    for doc in [
        "not json",
        r#"{"schema":"bramble.mobile.portfolio.v9","prices":{"fetchedAt":1,"prices":{}}}"#,
        r#"{"schema":"bramble.mobile.portfolio.v1","prices":{"fetchedAt":1,"prices":{"x":{"usd":1}}}}"#,
        r#"{"schema":"bramble.mobile.portfolio.v1","prices":{"fetchedAt":1,"prices":{},"evil":1}}"#,
    ] {
        let book = PriceBook::new(Some(doc.into()));
        match book.plan(
            NetworkId::Mainnet,
            vec![STRK.into()],
            DisplayCurrency::Usd,
            10.0,
            true,
        ) {
            PricePlan::Serve { view } => {
                assert!(view.prices.is_empty() && view.as_of_ms.is_none(), "{doc}")
            }
            PricePlan::Fetch { .. } => panic!("cached-only never fetches"),
        }
    }
    let snapshots = BalanceSnapshots::new(Some(r#"{"schema":"bramble.mobile.portfolio.v1","snapshots":[{"accountId":"SN_MAIN:0x0ab","balances":[],"fetchedAt":1,"networkId":"SN_MAIN"}]}"#.into()));
    assert_eq!(
        snapshots.plan("SN_MAIN:0x0ab".into(), NetworkId::Mainnet, 2.0, true, false),
        BalancePlan::Empty
    );
}

#[test]
fn preferences_default_on_corruption_but_protect_newer_files() {
    let corrupt = TokenPreferences::new(
        Some(r#"{"schema":"bramble.mobile.portfolio.v1","tokens":"nope"}"#.into()),
        catalogs(),
    );
    assert!(!corrupt.read_only());
    assert_eq!(corrupt.tracked(NetworkId::Mainnet).len(), 2);
    let newer = TokenPreferences::new(
        Some(r#"{"schema":"bramble.mobile.portfolio.v2","tokens":[]}"#.into()),
        catalogs(),
    );
    assert!(newer.read_only() && newer.persisted().is_none());
    let mut prefs = TokenPreferences::new(None, catalogs());
    prefs.set_display_currency(DisplayCurrency::Gbp);
    let id = prefs.tracked(NetworkId::Mainnet)[0].token.id.clone();
    prefs.set_hidden(id.clone(), true).unwrap();
    prefs.set_hidden(id.clone(), true).unwrap();
    let mut again = TokenPreferences::new(prefs.persisted(), catalogs());
    assert_eq!(again.display_currency(), DisplayCurrency::Gbp);
    assert!(again.tracked(NetworkId::Mainnet)[0].hidden_from_home);
    assert!(again.set_hidden(String::new(), true).is_err());
    assert!(again.explorer_fallback_allowed(NetworkId::Sepolia));
    assert!(again.discovery_candidates(NetworkId::Sepolia).is_empty());
    assert!(again
        .discovery_candidates(NetworkId::Mainnet)
        .iter()
        .all(|t| t.symbol != "STRK"));
}

#[test]
fn balance_snapshots_follow_cached_balances() {
    let mut snapshots = BalanceSnapshots::new(None);
    let id = account_id();
    assert_eq!(
        snapshots.plan(id.clone(), NetworkId::Mainnet, 1_000.0, false, false),
        BalancePlan::Load
    );
    assert_eq!(snapshots.failed(id.clone(), NetworkId::Mainnet), None);
    let token = tracked()[0].clone();
    let row = TokenBalance {
        token,
        balance: "5".into(),
        formatted: "0.000000000000000005".into(),
    };
    let view = snapshots.loaded(id.clone(), NetworkId::Mainnet, vec![row.clone()], 1_000.0);
    assert!(!view.stale);
    match snapshots.plan(id.clone(), NetworkId::Mainnet, 30_000.0, false, false) {
        BalancePlan::Serve { view } => assert!(!view.stale && view.balances == vec![row.clone()]),
        other => panic!("fresh snapshot is served, got {other:?}"),
    }
    assert_eq!(
        snapshots.plan(id.clone(), NetworkId::Mainnet, 30_000.0, false, true),
        BalancePlan::Load
    );
    assert_eq!(
        snapshots.plan(id.clone(), NetworkId::Mainnet, 61_000.0, false, false),
        BalancePlan::Load
    );
    let failed = snapshots.failed(id.clone(), NetworkId::Mainnet).unwrap();
    assert!(failed.stale && failed.balances == vec![row]);
    let restored = BalanceSnapshots::new(Some(snapshots.persisted()));
    assert!(
        matches!(restored.plan(id, NetworkId::Mainnet, 90_000.0, true, false), BalancePlan::Serve { view } if view.stale)
    );
    let issue = network_data_issue(
        BalanceHealth::Loaded {
            stale: true,
            as_of_ms: Some(1_000.0),
        },
        None,
    );
    assert_eq!(
        issue.as_deref(),
        Some("Using saved balances (01 Jan, 00:00 UTC).")
    );
}

#[test]
fn formatted_fiat_never_leaks_exponents_or_nan() {
    let mut value = 1e-9;
    while value < 1e30 {
        for currency in [
            DisplayCurrency::Usd,
            DisplayCurrency::Eur,
            DisplayCurrency::Gbp,
        ] {
            let text = format_fiat(value, currency);
            assert!(
                !text.contains('e') && !text.contains("NaN") && !text.contains("inf"),
                "{value} → {text}"
            );
        }
        value *= 3.7;
    }
    assert_eq!(format_fiat(f64::NAN, DisplayCurrency::Usd), "$0.00");
    assert_eq!(format_fiat(f64::INFINITY, DisplayCurrency::Usd), "$0.00");
}
