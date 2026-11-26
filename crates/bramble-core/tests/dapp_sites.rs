//! `DappSession`'s blocklist and site warnings (`src/dapp_sites.rs`), after the extension's
//! `tests/background/settings/background-blocklist-enforcement.test.ts` and
//! `approvals/approval-insights.test.ts`: a blocked site learns nothing and cannot be approved,
//! the live feed blocks once loaded and survives a restart through its cache, and a card carries
//! its site's warnings.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

mod common;

use bramble_core::{DappReply, DappSession, HttpMethod, HttpOutcome, NetworkId, SiteWarning};
use common::{account_id, imported, open, PASSWORD};

const NOW: f64 = 1_800_000_000_000.0;
const HOUR: f64 = 3_600_000.0;

fn ask(dapp: &DappSession, origin: &str, method: &str, request_id: u64) -> DappReply {
    dapp.request(
        origin.into(),
        method.into(),
        None,
        NetworkId::Sepolia,
        request_id,
    )
    .unwrap()
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

fn feed(body: &str) -> HttpOutcome {
    HttpOutcome {
        status: 200,
        retry_after: None,
        body: Some(body.as_bytes().to_vec()),
        timed_out: false,
    }
}

fn assert_learns_nothing(dapp: &DappSession, origin: &str) {
    for method in [
        "wallet_requestAccounts",
        "mc_requestAccounts",
        "wallet_requestChainId",
    ] {
        assert_eq!(
            code(&ask(dapp, origin, method, 1)),
            163,
            "{origin} {method}"
        );
    }
    let permissions = ask(dapp, origin, "wallet_getPermissions", 2);
    assert_eq!(permissions, DappReply::Result { json: "[]".into() });
}

// The bundled list blocks with no feed at all, subdomains included.
#[test]
fn a_bundled_blocked_site_learns_nothing_and_queues_no_card() {
    let (_dir, _clock, core) = imported();
    let dapp = DappSession::new(core);
    for origin in [
        "https://claim-starknet.com",
        "https://app.claim-starknet.com",
    ] {
        assert_learns_nothing(&dapp, origin);
    }
    assert!(dapp.pending_approvals(NOW).is_empty());
    assert_eq!(
        ask(
            &dapp,
            "https://notclaim-starknet.com",
            "wallet_requestChainId",
            3
        ),
        DappReply::Result {
            json: r#""0x534e5f5345504f4c4941""#.into()
        }
    );
}

// The feed blocks once answered, a failed refetch keeps it, and a restarted wallet loads it from
// the cache without asking the network while it is fresh.
#[test]
fn the_live_feed_blocks_and_survives_a_restart() {
    let (dir, clock, core) = imported();
    let dapp = DappSession::new(core.clone());
    let site = "https://app.evil.example";
    let request = dapp.refresh_phishing_feed(NOW).expect("nothing cached");
    assert_eq!(request.method, HttpMethod::Get);
    assert!(request
        .url
        .starts_with("https://raw.githubusercontent.com/MetaMask/"));
    pending(ask(&dapp, site, "wallet_requestAccounts", 1));

    dapp.answer_phishing_feed(feed(r#"{"blacklist":["Evil.Example."]}"#), NOW);
    assert_learns_nothing(&dapp, site);
    // The card it queued before is now flagged, and approving it is refused.
    let card = &dapp.pending_approvals(NOW)[0];
    assert!(card.blocked);
    assert_eq!(card.warnings[0], SiteWarning::Blocklisted);
    let settled = dapp.approve_connect(card.approval_id.clone(), account_id(&core), NOW);
    assert_eq!(code(&settled[0].reply), 163);

    dapp.answer_phishing_feed(
        HttpOutcome {
            status: 503,
            ..feed("")
        },
        NOW,
    );
    assert_learns_nothing(&dapp, site);

    drop(dapp);
    let core = open(dir.path(), &clock);
    let dapp = DappSession::new(core);
    assert!(dapp.refresh_phishing_feed(NOW + HOUR).is_none());
    assert_learns_nothing(&dapp, site);
    assert!(dapp.refresh_phishing_feed(NOW + 7.0 * HOUR).is_some());
}

// `enrichApprovals`: a card names its site's warnings; a decided card makes the site known, new
// for a day and then plain.
#[test]
fn a_card_carries_its_sites_warnings_and_the_wallet_remembers_it() {
    let (dir, _clock, core) = imported();
    let dapp = DappSession::new(core.clone());
    let site = "https://starknet.io.zip";
    let card = pending(ask(&dapp, site, "wallet_requestAccounts", 1));
    let listed = &dapp.pending_approvals(NOW)[0];
    assert!(!listed.blocked);
    assert_eq!(
        listed.warnings,
        [
            SiteWarning::Lookalike {
                target: "starknet.io".into()
            },
            SiteWarning::RiskyTld { tld: "zip".into() },
            SiteWarning::FirstSeen,
        ]
    );
    dapp.deny(card, NOW);

    pending(ask(&dapp, site, "wallet_requestAccounts", 2));
    let warnings = |now| dapp.pending_approvals(now)[0].warnings.clone();
    assert_eq!(warnings(NOW + HOUR).last(), Some(&SiteWarning::NewSite));
    assert_eq!(
        warnings(NOW + 25.0 * HOUR).last(),
        Some(&SiteWarning::RiskyTld { tld: "zip".into() })
    );

    // Deleting the wallet forgets the sites it saw.
    assert!(dir.path().join("origin-history.json").exists());
    core.delete_wallet(PASSWORD.into()).unwrap();
    assert!(!dir.path().join("origin-history.json").exists());
}
