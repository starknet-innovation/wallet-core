//! Ported from the extension's `tests/background/provider/add-chain-ops.test.ts`,
//! `transactions/chain-ops-switch.test.ts` and `portfolio/background-discovery-messages.test.ts`:
//! each gate in the extension's order, and the result shapes `results.ts` gives the page.

use super::*;
use crate::dapp::grants::OriginGrant;
use crate::dapp::Unlocked;
use starknet_types_core::felt::Felt;

const SITE: &str = "https://app.example";
const MAINNET: &str = "0x534e5f4d41494e";
const SEPOLIA: &str = "0x534e5f5345504f4c4941";
const VERSIONS: Versions = Versions {
    specs: &["0.10.0"],
    wallet_api: &["0.10.5"],
};

fn account() -> DappAccount {
    DappAccount {
        id: "SN_MAIN:0x1".into(),
        address: "0x1".into(),
        network_id: "SN_MAIN".into(),
    }
}

fn granted(ids: &[&str]) -> Vec<OriginGrant> {
    vec![OriginGrant {
        account_ids: ids.iter().map(|id| (*id).to_owned()).collect(),
        origin: SITE.into(),
    }]
}

fn site<'a>(blocked: bool, unlocked: Option<Unlocked<'a>>) -> Site<'a> {
    Site {
        origin: SITE,
        blocked,
        unlocked,
        chain_id: MAINNET,
    }
}

#[test]
fn a_chain_id_names_a_network_by_value_in_any_spelling() {
    assert_eq!(network_for_chain_id(MAINNET), Some(NetworkId::Mainnet));
    assert_eq!(
        network_for_chain_id("0x0534E5F5345504F4C4941"),
        Some(NetworkId::Sepolia)
    );
    assert_eq!(network_for_chain_id("0x1"), None);
    assert_eq!(network_for_chain_id("SN_MAIN"), None);
}

#[test]
fn add_chain_answers_a_known_chain_and_refuses_the_rest() {
    let open = site(false, None);
    assert_eq!(add_chain(&open, SEPOLIA), Ok(JsValue::Bool(true)));
    assert_eq!(add_chain(&open, "0x1234").unwrap_err().code, 117);
    // Blocked before the chain is looked at.
    assert_eq!(add_chain(&site(true, None), SEPOLIA).unwrap_err().code, 163);
}

#[test]
fn switch_chain_needs_an_unblocked_known_chain_and_a_connected_site() {
    let account = account();
    let grants = granted(&[&account.id]);
    let unlocked = Unlocked {
        grants: &grants,
        active: Some(&account),
    };
    assert_eq!(
        switch_chain(&site(false, Some(unlocked)), SEPOLIA),
        Ok(NetworkId::Sepolia)
    );
    // Blocked first, even when connected and the chain is unknown.
    assert_eq!(
        switch_chain(&site(true, Some(unlocked)), "0x1")
            .unwrap_err()
            .code,
        163
    );
    // An unknown chain is 117 before the grant is read, so an unconnected site learns it too.
    assert_eq!(
        switch_chain(&site(false, None), "0x1").unwrap_err().code,
        117
    );
    // Locked, or not connected: "origin not connected".
    assert_eq!(
        switch_chain(&site(false, None), SEPOLIA).unwrap_err().code,
        163
    );
    let other = granted(&["SN_MAIN:0x2"]);
    let unconnected = Unlocked {
        grants: &other,
        active: Some(&account),
    };
    assert_eq!(
        switch_chain(&site(false, Some(unconnected)), SEPOLIA)
            .unwrap_err()
            .code,
        163
    );
}

#[test]
fn a_switch_lands_on_the_account_the_grant_holds_there() {
    let grants = granted(&["SN_MAIN:0x1", "SN_SEPOLIA:0x9"]);
    let accounts = [
        ("SN_SEPOLIA:0x8", NetworkId::Sepolia),
        ("SN_SEPOLIA:0x9", NetworkId::Sepolia),
        ("SN_MAIN:0x1", NetworkId::Mainnet),
    ];
    assert_eq!(
        held_account_on(&grants, accounts, SITE, NetworkId::Sepolia),
        Some("SN_SEPOLIA:0x9")
    );
    assert_eq!(
        held_account_on(
            &granted(&["SN_MAIN:0x1"]),
            accounts,
            SITE,
            NetworkId::Sepolia
        ),
        None
    );
    assert_eq!(
        held_account_on(
            &grants,
            accounts,
            "https://other.example",
            NetworkId::Sepolia
        ),
        None
    );
    assert_eq!(switch_dedupe(NetworkId::Sepolia), "switch-chain:SN_SEPOLIA");
}

#[test]
fn deployment_data_gates_run_in_the_extensions_order() {
    let account = account();
    let grants = granted(&[&account.id]);
    let unlocked = Unlocked {
        grants: &grants,
        active: Some(&account),
    };
    let gate = |site: &Site<'_>, version| {
        deployment_data_gate(site, version, &VERSIONS)
            .map(|account| account.id.clone())
            .map_err(|error| error.code)
    };
    assert_eq!(
        gate(&site(false, Some(unlocked)), None),
        Ok(account.id.clone())
    );
    assert_eq!(
        gate(&site(false, Some(unlocked)), Some("0.10.5")),
        Ok(account.id.clone())
    );
    assert_eq!(gate(&site(true, Some(unlocked)), Some("9.9")), Err(163));
    // The version is checked before the lock: a locked wallet still says 162.
    assert_eq!(gate(&site(false, None), Some("9.9")), Err(162));
    assert_eq!(gate(&site(false, None), None), Err(163));
}

#[test]
fn deployment_data_is_padded_and_bound_to_the_stored_address() {
    let deployment = Deployment {
        class_hash: Felt::from_hex_unchecked("0xabc"),
        salt: Felt::ZERO,
        constructor_calldata: vec![Felt::from_hex_unchecked("0x7")],
    };
    let address = felt::canonical(&deployment.address().unwrap());
    let pad = |hex: &str| format!("0x{:0>64}", hex.trim_start_matches("0x"));
    let expected = JsValue::Object(vec![
        ("address".into(), JsValue::String(pad(&address))),
        (
            "calldata".into(),
            JsValue::Array(vec![JsValue::String(pad("7"))]),
        ),
        ("class_hash".into(), JsValue::String(pad("abc"))),
        ("salt".into(), JsValue::String(pad("0"))),
        ("version".into(), JsValue::Number(1.0)),
    ]);
    let undeployed = DeploymentState::Undeployed;
    assert_eq!(
        deployment_data(&address, Some(&deployment), undeployed),
        Ok(expected)
    );
    let code = |address: &str, deployment, state| {
        deployment_data(address, deployment, state)
            .unwrap_err()
            .code
    };
    assert_eq!(
        code(&address, Some(&deployment), DeploymentState::Deployed),
        115
    );
    assert_eq!(
        code(&address, Some(&deployment), DeploymentState::Unknown),
        163
    );
    assert_eq!(code(&address, None, undeployed), 116);
    // Parameters that do not reproduce the stored address are not offered.
    assert_eq!(code("0x1", Some(&deployment), undeployed), 116);
}
