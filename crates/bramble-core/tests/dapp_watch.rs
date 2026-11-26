//! `DappSession`'s `wallet_watchAsset` (`src/dapp_watch.rs`) over a real vault, after the
//! extension's `tests/background/provider/watch-asset.test.ts` and
//! `approvals/approval-watch-asset.test.ts`: the chain's metadata, the card or already-tracked
//! notice, and a site's token stored as the site's, which no review trusts (audit 171).
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

use crate::common;

use std::sync::Arc;

use bramble_core::{
    DappCard, DappReply, DappSession, DappSettled, NetworkId, RpcResponse, TokenPreferences,
};
use common::imported;

const SITE: &str = "https://app.example";
const NET: NetworkId = NetworkId::Sepolia;
const NOW: f64 = 1_800_000_000_000.0;
const TOKEN: &str = "0x5678";
/// "Zed Token", "ZZT" and 18, as `name()`, `symbol()` and `decimals()` return them.
const NAME: &str = "0x5a656420546f6b656e";
const SYMBOL: &str = "0x5a5a54";

fn ask(dapp: &DappSession, params: &str, request_id: u64) -> DappReply {
    dapp.request(
        SITE.into(),
        "wallet_watchAsset".into(),
        Some(params.into()),
        NET,
        request_id,
    )
    .unwrap()
}

fn erc20(extra: &str) -> String {
    format!(r#"{{"options":{{"address":"{TOKEN}"{extra}}},"type":"ERC20"}}"#)
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

/// The host's replies to a watch read: `name`, `symbol` and `decimals` in that order.
fn metadata(dapp: &DappSession, reply: DappReply, request_id: u64, felts: [&str; 3]) -> DappReply {
    let DappReply::Read { network, requests } = reply else {
        panic!("not a read: {reply:?}");
    };
    assert_eq!(network, NET);
    assert_eq!(requests.len(), 3);
    let responses = requests
        .iter()
        .zip(felts)
        .map(|(request, felt)| RpcResponse {
            id: request.id,
            body: format!(
                r#"{{"jsonrpc":"2.0","id":{},"result":["{felt}"]}}"#,
                request.id
            ),
        })
        .collect();
    dapp.answer_read(request_id, responses)
}

fn tracked(tokens: &TokenPreferences) -> bool {
    tokens
        .tracked(NET)
        .iter()
        .any(|t| t.token.address.ends_with("5678"))
}

fn trusted(tokens: &TokenPreferences) -> bool {
    tokens
        .trusted_token_addresses(NET)
        .iter()
        .any(|address| address.ends_with("5678"))
}

#[test]
fn a_sites_token_is_tracked_as_the_sites_once_the_person_approves() {
    let (_dir, _clock, core) = imported();
    // No lock or grant gate: tracking a token touches no account.
    core.lock();
    let tokens = TokenPreferences::new(None);
    let dapp = DappSession::new(core, tokens.clone());

    let reply = metadata(&dapp, ask(&dapp, &erc20(""), 1), 1, [NAME, SYMBOL, "0x12"]);
    let card = pending(reply);
    // The same token again joins the open card, with no second read.
    assert_eq!(pending(ask(&dapp, &erc20(r#","symbol":"FAKE""#), 2)), card);
    let listed = dapp.pending_approvals(NOW);
    assert_eq!(listed.len(), 1);
    let DappCard::WatchAsset {
        token,
        already_tracked,
    } = &listed[0].card
    else {
        panic!("not a watch card: {:?}", listed[0].card);
    };
    assert!(!already_tracked);
    assert_eq!(
        (token.symbol.as_str(), token.name.as_str(), token.decimals),
        ("ZZT", "Zed Token", 18)
    );
    assert!(!tracked(&tokens));

    let ok = DappReply::Result {
        json: "true".into(),
    };
    assert_eq!(
        dapp.approve_watch_asset(card, NOW),
        [1, 2].map(|request_id| DappSettled {
            request_id,
            reply: ok.clone(),
        })
    );
    assert!(tracked(&tokens));
    assert!(!trusted(&tokens));
    assert!(tokens.persisted().unwrap().contains("dappTokenIds"));

    // Asked again, the card is a notice: closing it answers as tracked, and writes nothing.
    let notice = pending(metadata(
        &dapp,
        ask(&dapp, &erc20(""), 3),
        3,
        [NAME, SYMBOL, "0x12"],
    ));
    assert!(matches!(
        dapp.pending_approvals(NOW)[0].card,
        DappCard::WatchAsset {
            already_tracked: true,
            ..
        }
    ));
    assert_eq!(
        dapp.deny(notice, NOW),
        vec![DappSettled {
            request_id: 3,
            reply: ok,
        }]
    );
}

#[test]
fn a_token_the_chain_does_not_describe_is_refused() {
    let (_dir, _clock, core) = imported();
    let tokens = TokenPreferences::new(None);
    let dapp: Arc<DappSession> = DappSession::new(core, tokens.clone());
    // Decimals past 36: the site's own decimals are never used instead.
    let reply = metadata(
        &dapp,
        ask(&dapp, &erc20(r#","decimals":6"#), 1),
        1,
        [NAME, SYMBOL, "0x64"],
    );
    assert_eq!(code(&reply), 114);
    // A failed read is not a token.
    let DappReply::Read { .. } = ask(&dapp, &erc20(""), 2) else {
        panic!("not a read");
    };
    assert_eq!(code(&dapp.answer_read(2, Vec::new())), 114);
    // An unusable on-chain name falls back to the site's.
    let reply = metadata(
        &dapp,
        ask(&dapp, &erc20(r#","name":" Site Name ""#), 3),
        3,
        ["0x0", SYMBOL, "0x12"],
    );
    pending(reply);
    let DappCard::WatchAsset { token, .. } = &dapp.pending_approvals(NOW)[0].card else {
        panic!("not a watch card");
    };
    assert_eq!(token.name, "Site Name");
    // Collectibles are not shown on mobile.
    let nft = r#"{"options":{"address":"0x99","tokenId":"42"},"type":"ERC721"}"#;
    assert_eq!(code(&ask(&dapp, nft, 4)), 163);
    assert!(!tracked(&tokens));
}
