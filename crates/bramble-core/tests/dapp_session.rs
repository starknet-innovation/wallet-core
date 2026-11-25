//! `DappSession` (`src/dapp_session.rs`), the connect slice the in-app browser's bridge calls:
//! the extension's connect flow end to end over a real vault, after
//! `tests/background/routing/background-messages.test.ts` ("creates pending approvals for
//! ungranted dapp account requests", "approves pending dapp account requests after unlock",
//! "returns no accounts without prompting for silent ungranted requests").
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

mod common;

use std::sync::Arc;

use bramble_core::{DappReply, DappSession, DappSettled, NetworkId, WalletCore};
use common::{account_id, imported, open, PASSWORD};

const SITE: &str = "https://app.example";
const NET: NetworkId = NetworkId::Sepolia;

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

fn address(core: &WalletCore) -> String {
    core.active_account().unwrap().unwrap().address
}

#[test]
fn a_site_asked_while_locked_connects_after_unlock_and_stays_connected() {
    let (dir, clock, core) = imported();
    let id = account_id(&core);
    let address = address(&core);
    core.lock();
    let dapp = DappSession::new(core.clone());

    // Locked looks unconnected: asked, never refused.
    assert_eq!(ask(&dapp, "wallet_getPermissions", None, 1), ok("[]"));
    assert_eq!(
        ask(
            &dapp,
            "wallet_requestAccounts",
            Some(r#"{"silent_mode":true}"#),
            2
        ),
        ok("[]")
    );
    let card = pending(ask(&dapp, "wallet_requestAccounts", None, 3));
    // Asking again joins the open card; Bramble's own method wants the accounts view.
    assert_eq!(pending(ask(&dapp, "mc_requestAccounts", None, 4)), card);
    let listed = dapp.pending_approvals();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].origin, SITE);

    core.unlock(PASSWORD.into()).unwrap();
    let settled = dapp.approve_connect(card.clone(), id.clone(), NET);
    assert_eq!(
        settled,
        vec![
            DappSettled {
                request_id: 3,
                reply: ok(&format!(r#"["{address}"]"#))
            },
            DappSettled {
                request_id: 4,
                reply: ok(&format!(
                    r#"{{"accounts":[{{"address":"{address}","networkId":"SN_SEPOLIA"}}],"ok":true}}"#
                )),
            },
        ]
    );
    assert!(dapp.pending_approvals().is_empty());
    // A card is decided once.
    assert!(dapp.approve_connect(card, id, NET).is_empty());

    // The grant is in the vault: a restarted wallet answers without asking.
    drop(dapp);
    let core = open(dir.path(), &clock);
    core.unlock(PASSWORD.into()).unwrap();
    let dapp = DappSession::new(core);
    assert_eq!(
        ask(&dapp, "wallet_requestAccounts", None, 5),
        ok(&format!(r#"["{address}"]"#))
    );
    assert_eq!(
        ask(&dapp, "wallet_getPermissions", None, 6),
        ok(r#"["accounts"]"#)
    );
    // Another network's page sees no account of this one.
    assert_eq!(
        dapp.request(
            SITE.into(),
            "mc_accounts".into(),
            None,
            NetworkId::Mainnet,
            7
        )
        .unwrap(),
        ok(r#"{"accounts":[],"ok":true}"#)
    );
}

#[test]
fn denying_refuses_every_waiting_request() {
    let (_dir, _clock, core) = imported();
    let dapp = DappSession::new(core);
    let card = pending(ask(&dapp, "wallet_requestAccounts", None, 1));
    pending(ask(&dapp, "wallet_requestAccounts", None, 2));
    let settled = dapp.deny_connect(card);
    assert_eq!(settled.len(), 2);
    assert!(settled.iter().all(|done| code(&done.reply) == 113));
    assert_eq!(ask(&dapp, "wallet_getPermissions", None, 3), ok("[]"));
}

#[test]
fn approving_for_an_account_no_longer_active_grants_nothing() {
    let (_dir, _clock, core) = imported();
    let dapp = DappSession::new(core.clone());
    let card = pending(ask(&dapp, "wallet_requestAccounts", None, 1));
    let settled = dapp.approve_connect(card, "SN_SEPOLIA:0x1".into(), NET);
    assert_eq!(code(&settled[0].reply), 163);
    assert_eq!(ask(&dapp, "wallet_getPermissions", None, 2), ok("[]"));

    // Locked at decision time: refused too, and the card is spent.
    let card = pending(ask(&dapp, "wallet_requestAccounts", None, 3));
    let id = account_id(&core);
    core.lock();
    let settled = dapp.approve_connect(card, id, NET);
    assert_eq!(code(&settled[0].reply), 163);
    assert!(dapp.pending_approvals().is_empty());
}

#[test]
fn an_expired_card_refuses_its_requests() {
    let (_dir, clock, core) = imported();
    let dapp = DappSession::new(core);
    pending(ask(&dapp, "wallet_requestAccounts", None, 9));
    assert!(dapp.settle_expired().is_empty());
    clock.advance(5 * 60 * 1_000);
    let settled = dapp.settle_expired();
    assert_eq!(settled.len(), 1);
    assert_eq!(settled[0].request_id, 9);
    assert_eq!(code(&settled[0].reply), 163);
    assert!(dapp.pending_approvals().is_empty());
}

#[test]
fn reads_refusals_and_what_this_slice_does_not_answer() {
    let (_dir, _clock, core) = imported();
    let dapp: Arc<DappSession> = DappSession::new(core);
    assert_eq!(
        ask(&dapp, "wallet_requestChainId", None, 1),
        ok(r#""0x534e5f5345504f4c4941""#)
    );
    assert_eq!(
        ask(&dapp, "wallet_supportedWalletApi", None, 2),
        ok(r#"["0.10.5"]"#)
    );
    assert_eq!(
        ask(&dapp, "wallet_supportedSpecs", None, 3),
        ok(r#"["0.10.0"]"#)
    );
    assert_eq!(
        code(&ask(
            &dapp,
            "wallet_supportedSpecs",
            Some(r#"{"api_version":"9.9"}"#),
            4
        )),
        162
    );
    assert_eq!(
        code(&ask(&dapp, "wallet_requestAccounts", Some("[]"), 5)),
        114
    );
    // Nothing that signs or writes is answered yet.
    assert_eq!(
        code(&ask(
            &dapp,
            "wallet_switchStarknetChain",
            Some(r#"{"chainId":"0x534e5f4d41494e"}"#),
            6
        )),
        163
    );
    // An origin the host could not have taken from a committed main frame.
    for origin in [
        "https://app.example/path",
        "app.example",
        "ftp://app.example",
        "",
    ] {
        let reply = dapp
            .request(origin.into(), "wallet_requestChainId".into(), None, NET, 7)
            .unwrap();
        assert_eq!(code(&reply), 114, "{origin}");
    }
    assert!(dapp.pending_approvals().is_empty());
}
