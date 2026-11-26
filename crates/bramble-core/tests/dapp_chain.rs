//! `DappSession`'s chain methods (`src/dapp_chain.rs`) over a real vault: add-chain answered at
//! once, switch-chain on a card, and deployment data after the host's chain read, after the
//! extension's `tests/background/provider/add-chain-ops.test.ts`,
//! `transactions/chain-ops-switch.test.ts` and `portfolio/background-discovery-messages.test.ts`.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

mod common;

use bramble_core::{
    DappCard, DappReply, DappSession, DappSettled, NetworkId, RpcResponse, TokenPreferences,
    WalletCore,
};
use common::{account_id, imported, PASSWORD};

const SITE: &str = "https://app.example";
const NET: NetworkId = NetworkId::Sepolia;
const NOW: f64 = 1_800_000_000_000.0;
const MAINNET: &str = "0x534e5f4d41494e";
const SEPOLIA: &str = "0x534e5f5345504f4c4941";

fn ask(dapp: &DappSession, method: &str, params: Option<&str>, request_id: u64) -> DappReply {
    dapp.request(
        SITE.into(),
        method.into(),
        params.map(str::to_owned),
        NET,
        request_id,
    )
    .unwrap()
}

fn ok(json: &str) -> DappReply {
    DappReply::Result { json: json.into() }
}

fn code(reply: &DappReply) -> i64 {
    match reply {
        DappReply::Error { code, .. } => *code,
        other => panic!("not an error: {other:?}"),
    }
}

fn pending(reply: DappReply) -> String {
    match reply {
        DappReply::Pending { approval_id } => approval_id,
        other => panic!("not pending: {other:?}"),
    }
}

fn connect(dapp: &DappSession, core: &WalletCore, request_id: u64) {
    let card = pending(ask(dapp, "wallet_requestAccounts", None, request_id));
    let settled = dapp.approve_connect(card, account_id(core), NOW);
    assert!(matches!(settled[0].reply, DappReply::Result { .. }));
}

fn switch_to(chain_id: &str) -> String {
    format!(r#"{{"chainId":"{chain_id}"}}"#)
}

#[test]
fn add_chain_is_answered_at_once_for_a_chain_the_wallet_has() {
    let (_dir, _clock, core) = imported();
    core.lock();
    let dapp = DappSession::new(core, TokenPreferences::new(None));
    let chain = |chain_id: &str| {
        format!(r#"{{"chain_id":"{chain_id}","chain_name":"Somewhere","id":"somewhere"}}"#)
    };
    // No grant, no unlock and no card: a built-in chain is already added.
    assert_eq!(
        ask(&dapp, "wallet_addStarknetChain", Some(&chain(MAINNET)), 1),
        ok("true")
    );
    assert_eq!(
        code(&ask(
            &dapp,
            "wallet_addStarknetChain",
            Some(&chain("0x4b4154414e41")),
            2
        )),
        117
    );
    assert!(dapp.pending_approvals(NOW).is_empty());
}

#[test]
fn an_approved_switch_moves_the_wallet_and_asks_to_reconnect_outside_the_grant() {
    let (_dir, _clock, core) = imported();
    let sepolia_id = account_id(&core);
    let dapp = DappSession::new(core.clone(), TokenPreferences::new(None));
    // An unconnected site may not ask; an unknown chain is 117 first.
    assert_eq!(
        code(&ask(
            &dapp,
            "wallet_switchStarknetChain",
            Some(&switch_to(MAINNET)),
            1
        )),
        163
    );
    assert_eq!(
        code(&ask(
            &dapp,
            "wallet_switchStarknetChain",
            Some(&switch_to("0x1")),
            2
        )),
        117
    );
    connect(&dapp, &core, 3);

    let card = pending(ask(
        &dapp,
        "wallet_switchStarknetChain",
        Some(&switch_to(MAINNET)),
        4,
    ));
    // The same chain joins the open card.
    assert_eq!(
        pending(ask(
            &dapp,
            "wallet_switchStarknetChain",
            Some(&switch_to(MAINNET)),
            5
        )),
        card
    );
    let listed = dapp.pending_approvals(NOW);
    assert_eq!(listed.len(), 1);
    assert_eq!(
        listed[0].card,
        DappCard::SwitchChain {
            network: NetworkId::Mainnet
        }
    );
    // Nothing moves before the person approves.
    assert_eq!(core.active_account().unwrap().unwrap().id, sepolia_id);

    assert_eq!(
        dapp.approve_switch_chain(card.clone(), NOW),
        [4, 5].map(|request_id| DappSettled {
            request_id,
            reply: ok("true"),
        })
    );
    assert!(dapp.approve_switch_chain(card, NOW).is_empty());
    let mainnet = core.active_account().unwrap().unwrap();
    assert_eq!(mainnet.network, NetworkId::Mainnet);
    // The site never saw the Mainnet account: it is asked to connect again, and sees nothing
    // until the person approves.
    assert_eq!(
        ask(&dapp, "wallet_requestChainId", None, 6),
        ok(&format!(r#""{MAINNET}""#))
    );
    assert_eq!(ask(&dapp, "wallet_getPermissions", None, 7), ok("[]"));
    let reconnect = dapp.pending_approvals(NOW);
    assert_eq!(reconnect.len(), 1);
    assert_eq!(reconnect[0].card, DappCard::Connect);
    dapp.approve_connect(reconnect[0].approval_id.clone(), mainnet.id.clone(), NOW);

    // Back to Sepolia: the grant holds an account there, so the switch lands on it.
    let card = pending(ask(
        &dapp,
        "wallet_switchStarknetChain",
        Some(&switch_to(SEPOLIA)),
        8,
    ));
    dapp.approve_switch_chain(card, NOW);
    assert_eq!(core.active_account().unwrap().unwrap().id, sepolia_id);
    assert!(dapp.pending_approvals(NOW).is_empty());
}

#[test]
fn a_switch_is_refused_once_the_site_is_disconnected_or_the_wallet_locked() {
    let (_dir, _clock, core) = imported();
    let id = account_id(&core);
    let dapp = DappSession::new(core.clone(), TokenPreferences::new(None));
    connect(&dapp, &core, 1);
    let card = pending(ask(
        &dapp,
        "wallet_switchStarknetChain",
        Some(&switch_to(MAINNET)),
        2,
    ));
    core.lock();
    assert_eq!(code(&dapp.approve_switch_chain(card, NOW)[0].reply), 163);
    core.unlock(PASSWORD.into()).unwrap();
    assert_eq!(core.active_account().unwrap().unwrap().id, id);
    assert!(dapp.pending_approvals(NOW).is_empty());
}

fn read_requests(reply: DappReply) -> (NetworkId, Vec<bramble_core::RpcRequest>) {
    match reply {
        DappReply::Read { network, requests } => (network, requests),
        other => panic!("not a read: {other:?}"),
    }
}

fn answer(dapp: &DappSession, request_id: u64, body: &str) -> DappReply {
    dapp.answer_read(
        request_id,
        vec![RpcResponse {
            id: 1,
            body: body.into(),
        }],
    )
}

const UNDEPLOYED: &str =
    r#"{"jsonrpc":"2.0","id":1,"error":{"code":20,"message":"Contract not found"}}"#;
const DEPLOYED: &str = r#"{"jsonrpc":"2.0","id":1,"result":"0x1"}"#;

#[test]
fn deployment_data_is_answered_after_the_chain_says_the_account_is_undeployed() {
    let (_dir, _clock, core) = imported();
    let dapp = DappSession::new(core.clone(), TokenPreferences::new(None));
    // Not connected: refused before any read.
    assert_eq!(code(&ask(&dapp, "wallet_deploymentData", None, 1)), 163);
    assert_eq!(
        code(&ask(
            &dapp,
            "wallet_deploymentData",
            Some(r#"{"api_version":"9.9"}"#),
            2
        )),
        162
    );
    connect(&dapp, &core, 3);
    let address = core.active_account().unwrap().unwrap().address;

    let (network, requests) = read_requests(ask(&dapp, "wallet_deploymentData", None, 4));
    assert_eq!(network, NET);
    assert_eq!(requests.len(), 1);
    assert!(requests[0].body.contains("starknet_getClassHashAt"));
    let DappReply::Result { json } = answer(&dapp, 4, UNDEPLOYED) else {
        panic!("no deployment data");
    };
    let padded = format!("0x{:0>64}", address.trim_start_matches("0x"));
    assert!(
        json.starts_with(&format!(r#"{{"address":"{padded}","calldata":["#)),
        "{json}"
    );
    assert!(json.ends_with(r#","version":1}"#), "{json}");
    // Answered once.
    assert_eq!(code(&answer(&dapp, 4, UNDEPLOYED)), 163);

    let read = |request_id| read_requests(ask(&dapp, "wallet_deploymentData", None, request_id));
    read(5);
    assert_eq!(code(&answer(&dapp, 5, DEPLOYED)), 115);
    // A failed read is not undeployed.
    read(6);
    assert_eq!(code(&answer(&dapp, 6, "")), 163);
    // A lock while the read was out refuses it.
    read(7);
    core.lock();
    assert_eq!(code(&answer(&dapp, 7, UNDEPLOYED)), 163);
    core.unlock(PASSWORD.into()).unwrap();
    // A page that went takes its read with it.
    read(8);
    dapp.withdraw(vec![8]);
    assert_eq!(code(&answer(&dapp, 8, UNDEPLOYED)), 163);
    // A site holds at most five reads open, as it holds five cards; a reused id is refused.
    for request_id in 9..14 {
        read(request_id);
    }
    assert_eq!(code(&ask(&dapp, "wallet_deploymentData", None, 14)), 163);
    assert_eq!(code(&ask(&dapp, "wallet_deploymentData", None, 9)), 163);
    assert!(matches!(
        answer(&dapp, 9, UNDEPLOYED),
        DappReply::Result { .. }
    ));
    read(14);
}
