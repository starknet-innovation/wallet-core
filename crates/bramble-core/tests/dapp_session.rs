//! `DappSession` (`src/dapp_session.rs`), the connect slice the in-app browser's bridge calls:
//! the extension's connect flow end to end over a real vault, after
//! `tests/background/routing/background-messages.test.ts` ("creates pending approvals for
//! ungranted dapp account requests", "approves pending dapp account requests after unlock",
//! "returns no accounts without prompting for silent ungranted requests").
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

mod common;

use std::sync::Arc;

use bramble_core::{DappReply, DappSession, DappSettled, NetworkId, TokenPreferences, WalletCore};
use common::{account_id, imported, open, PASSWORD};

const SITE: &str = "https://app.example";
const NET: NetworkId = NetworkId::Sepolia;
/// The host's wall clock.
const NOW: f64 = 1_800_000_000_000.0;

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
    let dapp = DappSession::new(core.clone(), TokenPreferences::new(None));

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
    let listed = dapp.pending_approvals(NOW);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].origin, SITE);

    core.unlock(PASSWORD.into()).unwrap();
    let settled = dapp.approve_connect(card.clone(), id.clone(), NOW);
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
    assert!(dapp.pending_approvals(NOW).is_empty());
    // A card is decided once.
    assert!(dapp.approve_connect(card, id, NOW).is_empty());

    // The grant is in the vault: a restarted wallet answers without asking.
    drop(dapp);
    let core = open(dir.path(), &clock);
    core.unlock(PASSWORD.into()).unwrap();
    let dapp = DappSession::new(core.clone(), TokenPreferences::new(None));
    assert_eq!(
        ask(&dapp, "wallet_requestAccounts", None, 5),
        ok(&format!(r#"["{address}"]"#))
    );
    assert_eq!(
        ask(&dapp, "wallet_getPermissions", None, 6),
        ok(r#"["accounts"]"#)
    );
    // The network is the active account's, whatever the browser shows, so the chain a site is
    // told and the account it sees never disagree.
    let on_mainnet = |method: &str| {
        dapp.request(SITE.into(), method.into(), None, NetworkId::Mainnet, 7)
            .unwrap()
    };
    assert_eq!(
        on_mainnet("mc_accounts"),
        ok(&format!(
            r#"{{"accounts":[{{"address":"{address}","networkId":"SN_SEPOLIA"}}],"ok":true}}"#
        ))
    );
    assert_eq!(
        on_mainnet("wallet_requestChainId"),
        ok(r#""0x534e5f5345504f4c4941""#)
    );
    // Locked, only the shown network is known.
    core.lock();
    assert_eq!(
        on_mainnet("wallet_requestChainId"),
        ok(r#""0x534e5f4d41494e""#)
    );
    assert_eq!(
        on_mainnet("mc_accounts"),
        ok(r#"{"accounts":[],"ok":true}"#)
    );
}

#[test]
fn denying_refuses_every_waiting_request() {
    let (_dir, _clock, core) = imported();
    let dapp = DappSession::new(core, TokenPreferences::new(None));
    let card = pending(ask(&dapp, "wallet_requestAccounts", None, 1));
    pending(ask(&dapp, "mc_requestAccounts", None, 2));
    let refused = || DappReply::Error {
        code: 113,
        message: "The user refused the operation".into(),
        data: None,
    };
    assert_eq!(
        dapp.deny(card, NOW),
        [1, 2].map(|request_id| DappSettled {
            request_id,
            reply: refused()
        })
    );
    assert_eq!(ask(&dapp, "wallet_getPermissions", None, 3), ok("[]"));
}

#[test]
fn approving_for_an_account_no_longer_active_grants_nothing() {
    let (_dir, _clock, core) = imported();
    let dapp = DappSession::new(core.clone(), TokenPreferences::new(None));
    let card = pending(ask(&dapp, "wallet_requestAccounts", None, 1));
    let settled = dapp.approve_connect(card, "SN_SEPOLIA:0x1".into(), NOW);
    assert_eq!(code(&settled[0].reply), 163);
    assert_eq!(ask(&dapp, "wallet_getPermissions", None, 2), ok("[]"));

    // Locked at decision time: refused too, and the card is spent.
    let card = pending(ask(&dapp, "wallet_requestAccounts", None, 3));
    let id = account_id(&core);
    core.lock();
    let settled = dapp.approve_connect(card, id, NOW);
    assert_eq!(code(&settled[0].reply), 163);
    assert!(dapp.pending_approvals(NOW).is_empty());
}

#[test]
fn an_expired_card_refuses_its_requests() {
    let (_dir, clock, core) = imported();
    let dapp = DappSession::new(core, TokenPreferences::new(None));
    pending(ask(&dapp, "wallet_requestAccounts", None, 9));
    assert!(dapp.settle_expired().is_empty());
    clock.advance(5 * 60 * 1_000);
    let settled = dapp.settle_expired();
    assert_eq!(settled.len(), 1);
    assert_eq!(settled[0].request_id, 9);
    // `DROPPED_APPROVAL_RESPONSE`: on the wire, an expiry is a deny.
    assert_eq!(code(&settled[0].reply), 113);
    assert!(dapp.pending_approvals(NOW).is_empty());
}

#[test]
fn reads_refusals_and_what_this_slice_does_not_answer() {
    let (_dir, _clock, core) = imported();
    let dapp: Arc<DappSession> = DappSession::new(core, TokenPreferences::new(None));
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
    assert!(dapp.pending_approvals(NOW).is_empty());
}

// `MAX_WAITERS_PER_APPROVAL`, and the queue's cap: a connect card is one per site, so a full
// queue has no site holding two and nothing to evict; the newcomer is refused.
#[test]
fn a_card_holds_eight_requests_and_the_queue_fifty_sites() {
    let (_dir, _clock, core) = imported();
    let dapp = DappSession::new(core, TokenPreferences::new(None));
    let card = pending(ask(&dapp, "wallet_requestAccounts", None, 1));
    for request_id in 2..=8 {
        assert_eq!(
            pending(ask(&dapp, "wallet_requestAccounts", None, request_id)),
            card
        );
    }
    assert_eq!(code(&ask(&dapp, "wallet_requestAccounts", None, 9)), 163);
    assert_eq!(dapp.deny(card, NOW).len(), 8);

    let from = |site: usize| {
        dapp.request(
            format!("https://app-{site}.example"),
            "wallet_requestAccounts".into(),
            None,
            NET,
            site as u64,
        )
        .unwrap()
    };
    for site in 0..50 {
        pending(from(site));
    }
    assert_eq!(code(&from(50)), 163);
    assert_eq!(dapp.pending_approvals(NOW).len(), 50);
    assert!(dapp.settle_expired().is_empty());
}

// A grant whose vault write fails is rolled back: the site is not connected in the session
// either, and its requests are refused.
#[cfg(unix)]
#[test]
fn a_failed_grant_write_connects_nothing() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, _clock, core) = imported();
    let id = account_id(&core);
    let dapp = DappSession::new(core.clone(), TokenPreferences::new(None));
    let card = pending(ask(&dapp, "wallet_requestAccounts", None, 1));
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
    let settled = dapp.approve_connect(card, id, NOW);
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(code(&settled[0].reply), 163);
    assert_eq!(ask(&dapp, "wallet_getPermissions", None, 2), ok("[]"));
    core.lock();
    core.unlock(PASSWORD.into()).unwrap();
    assert_eq!(ask(&dapp, "wallet_getPermissions", None, 3), ok("[]"));
}

// `withdraw`, a mobile divergence: a page that has gone takes its requests with it, and a card no
// page waits on any more is withdrawn, so a late approval grants nothing.
#[test]
fn a_withdrawn_page_leaves_a_shared_card_and_a_lone_card_is_withdrawn() {
    let (_dir, _clock, core) = imported();
    let id = account_id(&core);
    let dapp = DappSession::new(core.clone(), TokenPreferences::new(None));
    // Two pages of the site share one connect card; the first goes.
    let card = pending(ask(&dapp, "wallet_requestAccounts", None, 1));
    assert_eq!(pending(ask(&dapp, "wallet_requestAccounts", None, 2)), card);
    dapp.withdraw(vec![1]);
    assert_eq!(dapp.pending_approvals(NOW).len(), 1);
    let settled = dapp.approve_connect(card, id.clone(), NOW);
    assert_eq!(
        settled.iter().map(|s| s.request_id).collect::<Vec<_>>(),
        [2]
    );

    // A page alone on its card: withdrawing it drops the card, and approving it grants nothing.
    let other = "https://other.example";
    let ask_other = |method: &str, request_id| {
        dapp.request(other.into(), method.into(), None, NET, request_id)
            .unwrap()
    };
    let card = pending(ask_other("wallet_requestAccounts", 3));
    dapp.withdraw(vec![3, 99]);
    assert!(dapp.pending_approvals(NOW).is_empty());
    assert!(dapp.approve_connect(card, id, NOW).is_empty());
    assert!(dapp.settle_expired().is_empty());
    assert_eq!(ask_other("wallet_getPermissions", 4), ok("[]"));
}
