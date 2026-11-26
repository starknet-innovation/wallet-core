//! P16's `sign` block (`fixtures/extension-dapp.jsonl`): `wallet_signTypedData` sent through the
//! extension's real background handler, each case stopped (or not) by one of its gates before a
//! card. Here `DappSession::request` over a real vault in the same state answers the page as the
//! extension did and queues the same cards:
//!
//! - locked, blocked, not connected, connected to another account: refused (163);
//! - the document's chain against the account's network, either way: refused (114);
//! - a lock landing mid-request: the core runs every check under one hold of the session, so a
//!   lock lands before the request or after its card; the extension refuses, as a lock before;
//! - a presented multisig: listed as a deviation (below).
//!
//! The extension's stub accounts are made up, so each case is rebuilt from the published phrase:
//! the "first" account is OZ index 0 on the case's network, "second" another Sepolia account, and
//! the blocked site is one the core's blocklist names (the extension blocked its site by guard).

#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

use crate::common;
use crate::dapp_parity_support;

use bramble_core::{DappCard, DappReply, DappSession, NetworkId, TokenPreferences, WalletCore};
use common::dapp::{pending_id, NOW};
use common::{account_id, imported};
use dapp_parity_support::*;
use serde_json::{json, Value};

/// A site on the core's blocklist, standing for the extension's guard-blocked site.
const BLOCKED_SITE: &str = "https://starknet-airdrop.com";

const DEVIATIONS: &[(&str, &str)] = &[(
    "a presented multisig needing two signers",
    "the core has no multisig dapp lane: an app cannot present a multisig to a site, so the \
     vault account behind it is connected and its card queues, where the extension refuses",
)];

fn network(chain_id: &str) -> NetworkId {
    match chain_id {
        "0x534e5f4d41494e" => NetworkId::Mainnet,
        "0x534e5f5345504f4c4941" => NetworkId::Sepolia,
        other => panic!("unknown chain {other}"),
    }
}

/// The page's answer as the fixture writes it.
fn page(reply: &DappReply) -> Value {
    match reply {
        DappReply::Pending { .. } => json!("pending"),
        DappReply::Error {
            code,
            message,
            data,
        } => {
            let mut error = json!({ "code": code, "message": message });
            if let Some(data) = data {
                error["data"] = json!(data);
            }
            json!({ "error": error })
        }
        DappReply::Result { json } => {
            json!({ "result": serde_json::from_str::<Value>(json).unwrap() })
        }
        DappReply::Read { .. } => panic!("a typed-data request never asks the host for a read"),
    }
}

fn kind(card: &DappCard) -> &'static str {
    match card {
        DappCard::Connect => "connect",
        DappCard::SignTypedData { .. } => "sign-typed-data",
        DappCard::SwitchChain { .. } => "switch-chain",
        DappCard::WatchAsset { .. } => "watch-asset",
        DappCard::Invoke { .. } => "invoke",
    }
}

/// `origin` connected to the active account.
fn connect(dapp: &DappSession, core: &WalletCore, origin: &str, shown: NetworkId) {
    let reply = dapp
        .request(
            origin.into(),
            "wallet_requestAccounts".into(),
            None,
            shown,
            0,
        )
        .unwrap();
    let settled = dapp.approve_connect(pending_id(reply), account_id(core), NOW);
    assert_eq!(settled.len(), 1);
}

#[test]
fn each_sign_request_is_gated_as_the_extension_gated_it() {
    let mut deviations = Deviations::new(DEVIATIONS);
    for case in cases("sign", 11) {
        let name = name(&case);
        let state = &case["state"];
        let shown = network(text(&state["chainId"]));
        let (_dir, _clock, core) = imported();
        let first = match text(&state["active"]["networkId"]) {
            "SN_MAIN" => {
                let id = core.add_regular_account(NetworkId::Mainnet).unwrap().id;
                core.select_account(id.clone()).unwrap();
                id
            }
            _ => account_id(&core),
        };
        let dapp = DappSession::new(core.clone(), TokenPreferences::new(None));
        let origin = text(&state["origin"]);
        let granted = state["grants"][0]["accountIds"].as_array().unwrap();
        if !granted.is_empty() {
            connect(&dapp, &core, origin, shown);
        }
        // "second": the active account is one the grant never names.
        if text(&state["active"]["id"]).ends_with("0xfeedface") {
            let second = core.add_regular_account(NetworkId::Sepolia).unwrap().id;
            core.select_account(second.clone()).unwrap();
            assert_ne!(second, first, "{name}");
        }
        // A lock mid-request lands, in the core, before it.
        if state["locked"] == true || !state["lockAt"].is_null() {
            core.lock();
        }
        let asking = if state["blocked"] == true {
            BLOCKED_SITE
        } else {
            origin
        };
        let params = json!({ "typed_data": case["typedData"] }).to_string();
        let reply = dapp
            .request(
                asking.into(),
                "wallet_signTypedData".into(),
                Some(params),
                shown,
                1,
            )
            .unwrap();
        let cards: Vec<&str> = dapp
            .pending_approvals(NOW)
            .iter()
            .map(|approval| kind(&approval.card))
            .collect();
        let ours = json!({ "answer": page(&reply), "cards": cards });
        let theirs = json!({ "answer": case["answer"], "cards": case["cards"] });
        deviations.check(&name, ours, theirs);
    }
    deviations.finish();
}
