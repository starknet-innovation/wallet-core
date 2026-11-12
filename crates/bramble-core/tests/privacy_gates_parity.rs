//! The privacy gates against the extension's, from `extension-privacy-rules.json` (written by
//! `bun run mobile:parity` from the extension's own code): Home, Receive, token detail, the
//! journey and the unified Home balances, over Bramble's catalogs (`networks.json`,
//! `privacy.json`).

mod portfolio_support;
mod privacy_support;

use portfolio_support::networks;
use privacy_support::{routes, rules};
use serde_json::Value;
use starknet_wallet_core::catalog::network_info;
use starknet_wallet_core::portfolio::fiat::{compact_amount, portfolio_amount};
use starknet_wallet_core::portfolio::model::{TokenBalance, TokenConfig};
use starknet_wallet_core::portfolio::registry::token_config_id;
use starknet_wallet_core::privacy::gates::{
    self, entry_outcome, private_line, receive_pool_notice, shield_tile, token_detail, Outcome,
};
use starknet_wallet_core::privacy::home::unified;
use starknet_wallet_core::privacy::types::{
    DetailAction, DetailSummary, PoolNotice, PrivacyBlocker, PrivacyEntry, PrivacyReading,
    PrivacyRegistration, PrivacyState, PrivacyStatus, PrivateBalance, PrivateLine, ShieldTile,
};
use starknet_wallet_core::privacy::{privacy_entry, shielded_balances_included};
use starknet_wallet_core::send::types::DeploymentState;
use starknet_wallet_core::{rpc, NetworkId};

fn text(value: &Value) -> &str {
    value.as_str().unwrap()
}

fn reading(name: &str) -> (NetworkId, PrivacyReading) {
    let settled = |registration| PrivacyReading::Settled { registration };
    match name {
        "idle" => (NetworkId::Mainnet, PrivacyReading::Idle),
        "loading" => (NetworkId::Mainnet, PrivacyReading::Loading),
        "failed" => (NetworkId::Mainnet, PrivacyReading::Failed),
        "unconfigured" => (NetworkId::Sepolia, PrivacyReading::Unconfigured),
        "unregistered" => (
            NetworkId::Mainnet,
            settled(PrivacyRegistration::Unregistered),
        ),
        "registered" => (NetworkId::Mainnet, settled(PrivacyRegistration::Registered)),
        other => panic!("reading {other}"),
    }
}

fn deployment(name: &str) -> DeploymentState {
    match name {
        "deployed" => DeploymentState::Deployed,
        "undeployed" => DeploymentState::Undeployed,
        _ => DeploymentState::Unknown,
    }
}

fn funded(name: &str) -> Option<bool> {
    match name {
        "zero" => Some(false),
        "funded" => Some(true),
        _ => None,
    }
}

/// The extension's journey outcome before the core's capabilities apply, spelled as the fixture
/// records it (`open-shield`, `activate`, `blocked:<kind>`, `check`, `discovering`); mobile's
/// `testing::privacy::entry_outcome` hook.
fn spelled_outcome(
    network: NetworkId,
    deployment: DeploymentState,
    strk_funded: Option<bool>,
    reading: PrivacyReading,
    after_read: bool,
) -> String {
    match entry_outcome(
        &routes(),
        network,
        deployment,
        strk_funded,
        reading,
        after_read,
    ) {
        Outcome::OpenShield => "open-shield".into(),
        Outcome::Activate => "activate".into(),
        Outcome::CheckDeployment => "check".into(),
        Outcome::Discovering => "discovering".into(),
        Outcome::Blocked(blocker) => {
            format!("blocked:{}", format!("{blocker:?}").to_lowercase())
        }
    }
}

const ETH: &str = "0x049d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7";
const STRK: &str = "0x04718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d";

/// A public Home row as the portfolio builds it for a bundled token (`HomeRows.visible`).
fn public_row(network: NetworkId, address: &str, balance: &str) -> TokenBalance {
    let token = network_info(networks(), network)
        .unwrap()
        .tokens
        .iter()
        .find(|token| {
            token
                .address
                .trim_start_matches("0x")
                .trim_start_matches('0')
                == address.trim_start_matches("0x").trim_start_matches('0')
        })
        .expect("a bundled token");
    let padded = format!("0x{:0>64}", address.trim_start_matches("0x"));
    TokenBalance {
        token: TokenConfig {
            id: token_config_id(network, token.symbol.clone(), padded.clone()),
            network,
            address: padded,
            symbol: token.symbol.clone(),
            name: token.name.clone(),
            decimals: token.decimals,
        },
        formatted: rpc::format_amount(balance, token.decimals, token.decimals).unwrap(),
        balance: balance.into(),
    }
}

fn registered_state(network: NetworkId, rules: &Value) -> PrivacyState {
    let entry = |address: &str, symbol: &str, balance: &str, formatted: &str| PrivateBalance {
        token_address: address.into(),
        symbol: symbol.into(),
        decimals: 18,
        balance: balance.into(),
        formatted: formatted.into(),
    };
    PrivacyState {
        network,
        account_id: "SN_MAIN:0x1".into(),
        registration: PrivacyRegistration::Registered,
        balances: vec![
            entry(
                ETH,
                "ETH",
                text(&rules["privateEth"]["balance"]),
                text(&rules["privateEth"]["formatted"]),
            ),
            entry(STRK, "STRK", "0", "0"),
        ],
        pool_fee_fri: "0x53444835ec580000".into(),
        pool_address: "0x0".into(),
    }
}

#[test]
fn static_gates_match_the_rendered_extension() {
    let rules = rules();
    let (catalog, routes) = (networks(), routes());
    for row in rules["static"].as_array().unwrap() {
        let (network, read) = reading(text(&row["reading"]));
        let dep = deployment(text(&row["deployment"]));
        let fund = funded(text(&row["strk"]));
        let state = Some(registered_state(network, &rules));
        let label = format!("{row}");
        let tile = &row["tile"];
        // One word on every tile; the accessible name carries the reason and tells them apart.
        assert_eq!(text(&tile["label"]), "Shield", "{label}");
        let expected_tile = match (text(&tile["action"]), text(&tile["aria"])) {
            ("none", _) => ShieldTile::NoStrkToken,
            ("send-shield", _) => ShieldTile::Ready,
            ("privacy-open", "Shield — add STRK to this account first") => ShieldTile::FundFirst,
            ("privacy-open", "Shield — deploy this account first") => ShieldTile::DeployFirst,
            ("privacy-open", "Activate private balances") => ShieldTile::Activate,
            other => panic!("unexpected tile {other:?}"),
        };
        assert_eq!(
            shield_tile(catalog, &routes, network, dep, fund, read),
            expected_tile,
            "{label}"
        );
        let expected_line = match text(&row["privateLine"]["text"]) {
            "Discovering…" => PrivateLine::Discovering,
            "Unavailable" => PrivateLine::Unavailable,
            "Register to start shielding" => PrivateLine::RegisterToStart,
            "Shield" => PrivateLine::Shield,
            amount => PrivateLine::Amount {
                text: amount.into(),
            },
        };
        assert_eq!(
            private_line(catalog, &routes, network, dep, read, ETH, state.as_ref()),
            expected_line,
            "{label}"
        );
        let status = gates::status(&routes, network, read);
        assert_eq!(
            status == PrivacyStatus::Unavailable,
            row["retryRow"].as_bool().unwrap(),
            "{label}"
        );
        let expected_status = match row["status"].as_str() {
            None => PrivacyStatus::Current,
            Some(s) if s.contains("refreshing") => PrivacyStatus::Refreshing,
            Some(_) => PrivacyStatus::Unavailable,
        };
        assert_eq!(status, expected_status, "{label}");
        let expected_notice = match text(&row["notice"]) {
            "fund-first" => PoolNotice::FundFirst,
            "deploy-first" => PoolNotice::DeployFirst,
            "register" => PoolNotice::Register,
            _ => PoolNotice::Hidden,
        };
        assert_eq!(
            receive_pool_notice(dep, fund, read),
            expected_notice,
            "{label}"
        );
        let detail = token_detail(catalog, &routes, network, dep, read, ETH, state.as_ref());
        let expected_summary = match row["detail"]["summary"].as_str() {
            None => DetailSummary::None,
            Some("Try again") => DetailSummary::Retry,
            Some("Register to start shielding") => DetailSummary::RegisterToStart,
            Some(amount) => DetailSummary::Amount {
                text: amount.into(),
            },
        };
        assert_eq!(detail.summary, expected_summary, "{label}");
        let actions: Vec<&str> = row["detail"]["actions"]
            .as_array()
            .unwrap()
            .iter()
            .map(text)
            .collect();
        let expected_action = match actions.as_slice() {
            [] => DetailAction::None,
            ["Go private"] => DetailAction::GoPrivate,
            ["Shield", "Unshield"] => DetailAction::ShieldUnshield,
            // One "Shield" label, two actions: an undeployed account's control opens the
            // journey; a deployed, unregistered one is the first shield itself (789).
            ["Shield"] if text(&row["deployment"]) == "undeployed" => DetailAction::Shield,
            ["Shield"] => DetailAction::ShieldFirst,
            other => panic!("unexpected actions {other:?}"),
        };
        assert_eq!(detail.action, expected_action, "{label}");
    }
}

#[test]
fn journey_outcomes_match_the_bound_extension() {
    let rules = rules();
    let routes = routes();
    for row in rules["journeys"].as_array().unwrap() {
        let (network, mut read) = reading(text(&row["reading"]));
        let mut dep = deployment(text(&row["deployment"]));
        let (_, loaded) = match text(&row["load"]) {
            "failed" => (NetworkId::Mainnet, PrivacyReading::Failed),
            other => reading(other),
        };
        let mut after_read = false;
        let outcome = loop {
            // The fixture binds the journey without a balance read, so funding is unknown.
            match spelled_outcome(network, dep, None, read, after_read).as_str() {
                "check" => match text(&row["deploymentRead"]) {
                    "throws" => break "blocked:service".to_string(),
                    other => dep = deployment(other),
                },
                "discovering" => {
                    assert!(!after_read, "{row}: discovering twice");
                    read = loaded;
                    after_read = true;
                }
                done => break done.to_string(),
            }
        };
        assert_eq!(outcome, text(&row["outcome"]), "{row}");
        if outcome == "open-shield" || outcome == "activate" {
            assert_eq!(
                privacy_entry(&routes, network, dep, None, read, after_read),
                PrivacyEntry::Blocked {
                    blocker: PrivacyBlocker::Unsupported
                }
            );
        }
    }
}

#[test]
fn unified_home_balances_match_the_extension() {
    let rules = rules();
    for entry in rules["unified"].as_array().unwrap() {
        let (network, read) = reading(text(&entry["reading"]));
        let state = Some(registered_state(network, &rules));
        let public: Vec<TokenBalance> = entry["public"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| public_row(network, text(&p["token"]), text(&p["balance"])))
            .collect();
        let rows = unified(networks(), network, &public, &[], read, state.as_ref());
        let expected = entry["rows"].as_array().unwrap();
        assert_eq!(rows.len(), expected.len(), "{entry}");
        for (mine, theirs) in rows.iter().zip(expected) {
            assert_eq!(
                mine.balance.token.address,
                text(&theirs["token"]),
                "{entry}"
            );
            assert_eq!(mine.balance.balance, text(&theirs["balance"]), "{entry}");
            assert_eq!(
                mine.balance.formatted,
                text(&theirs["formatted"]),
                "{entry}"
            );
            assert_eq!(
                portfolio_amount(mine.balance.formatted.clone()),
                text(&theirs["compact"]),
                "{entry}"
            );
        }
        assert_eq!(
            shielded_balances_included(read, state.as_ref(), &[]),
            entry["includesShielded"].as_bool().unwrap(),
            "{entry}"
        );
    }
    for entry in rules["compactAmounts"].as_array().unwrap() {
        let places = entry["places"].as_u64().map(|p| p as u32);
        assert_eq!(
            compact_amount(text(&entry["input"]).into(), places),
            text(&entry["text"]),
            "{entry}"
        );
    }
}

/// The extension's scope guard: a Mainnet reading never feeds Sepolia rows, although ETH and STRK
/// share addresses on both networks (the window between a network switch and the next read).
#[test]
fn a_reading_from_another_network_is_ignored() {
    let rules = rules();
    let state = Some(registered_state(NetworkId::Mainnet, &rules));
    let read = PrivacyReading::Settled {
        registration: PrivacyRegistration::Registered,
    };
    let public = vec![public_row(NetworkId::Sepolia, ETH, "1")];
    let rows = unified(
        networks(),
        NetworkId::Sepolia,
        &public,
        &[],
        read,
        state.as_ref(),
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].balance.balance, "1");
    assert!(!rows[0].private_only);
    let public = vec![public_row(NetworkId::Mainnet, ETH, "1")];
    let mainnet = unified(
        networks(),
        NetworkId::Mainnet,
        &public,
        &[],
        read,
        state.as_ref(),
    );
    let eth = mainnet
        .iter()
        .find(|row| row.balance.token.symbol == "ETH")
        .unwrap();
    assert_ne!(
        eth.balance.balance, "1",
        "the same reading counts on its own network"
    );
}
