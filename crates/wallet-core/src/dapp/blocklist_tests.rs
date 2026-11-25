//! Ported from the extension's `packages/features/permissions/tests/blocklist.test.ts`,
//! `blocklist-trailing-dot.test.ts` and the feed cases of `phishing-feed.test.ts`, over a subset
//! of Bramble's lists (the full lists are `bramble-core`'s `SITE_LISTS`).

use super::*;

const LISTS: SiteLists = SiteLists {
    blocked: &[
        "airdrop-starknet.com",
        "braavos-airdrop.com",
        "starknet-airdrop.com",
    ],
    protected: &["argent.xyz", "starknet.io"],
    risky_tlds: &["zip", "top"],
};

fn none() -> HashSet<String> {
    HashSet::new()
}

fn blocked(origin: &str) -> bool {
    is_blocked_origin(&LISTS, &none(), origin)
}

const KNOWN: SiteContext = SiteContext {
    connected: false,
    first_seen_ms: Some(0.0),
    now_ms: DAY_MS + 1.0,
};

fn warnings(origin: &str) -> Vec<SiteWarning> {
    origin_insights(&LISTS, &none(), origin, KNOWN).warnings
}

fn lookalike(target: &str) -> SiteWarning {
    SiteWarning::Lookalike {
        target: target.into(),
    }
}

#[test]
fn blocks_bundled_domains_their_subdomains_and_dotted_forms() {
    for origin in [
        "https://starknet-airdrop.com",
        "https://www.starknet-airdrop.com",
        "https://braavos-airdrop.com",
        "https://starknet-airdrop.com.",
        "https://www.airdrop-starknet.com.",
        "https://starknet-airdrop.com:8443",
    ] {
        assert!(blocked(origin), "{origin}");
    }
}

#[test]
fn leaves_legitimate_lookalike_suffixes_and_unparseable_origins_alone() {
    for origin in [
        "https://starknet.io",
        "https://app.argent.xyz",
        "https://example.com.",
        // A suffix that is not a label boundary.
        "https://notstarknet-airdrop.com",
        "not a url",
        "https://starknet-airdrop.com/path",
        "http://starknet-airdrop.com",
    ] {
        assert!(!blocked(origin), "{origin}");
    }
}

#[test]
fn origin_host_reads_what_the_url_parser_prints() {
    assert_eq!(
        origin_host("https://a.example.com:8443"),
        Some("a.example.com")
    );
    assert_eq!(origin_host("https://example.com."), Some("example.com"));
    assert_eq!(origin_host("http://localhost:5173"), Some("localhost"));
    assert_eq!(origin_host("https://[::1]"), Some("[::1]"));
    assert_eq!(origin_host("https://Example.com"), None);
    assert_eq!(origin_host("https://example.com/"), None);
    assert_eq!(origin_host("https://u@example.com"), None);
}

#[test]
fn a_feed_domain_blocks_itself_and_its_subdomains() {
    let feed: HashSet<String> = ["starknet-rewards-2026.com".to_owned()].into();
    assert!(is_blocked_origin(
        &LISTS,
        &feed,
        "https://starknet-rewards-2026.com"
    ));
    assert!(is_blocked_origin(
        &LISTS,
        &feed,
        "https://login.starknet-rewards-2026.com"
    ));
    assert!(!is_blocked_origin(&LISTS, &feed, "https://legit-dapp.xyz"));
    assert!(!is_blocked_origin(&LISTS, &feed, "https://2026.com"));
    // The bundled list still blocks with an empty feed.
    assert!(is_blocked_origin(
        &LISTS,
        &none(),
        "https://braavos-airdrop.com"
    ));
    let insight = origin_insights(&LISTS, &feed, "https://starknet-rewards-2026.com", KNOWN);
    assert!(insight.blocked);
    assert_eq!(insight.warnings, [SiteWarning::Blocklisted]);
}

#[test]
fn a_blocked_site_is_flagged_with_the_blocklist_warning() {
    let insight = origin_insights(&LISTS, &none(), "https://starknet-airdrop.com", KNOWN);
    assert!(insight.blocked);
    assert_eq!(insight.warnings, [SiteWarning::Blocklisted]);
}

#[test]
fn warns_on_punycode_lookalikes_and_risky_tlds() {
    assert_eq!(
        warnings("https://xn--pple-43d.com"),
        [SiteWarning::Punycode]
    );
    // Digit substitution, a hyphen, one edit, a brand smuggled into another domain.
    for origin in [
        "https://st4rknet.io",
        "https://star-knet.io",
        "https://starkne.io",
        "https://starknet.io.evil-phish.com",
    ] {
        assert_eq!(warnings(origin), [lookalike("starknet.io")], "{origin}");
    }
    assert_eq!(warnings("https://argnt.xyz"), [lookalike("argent.xyz")]);
    assert_eq!(
        warnings("https://random-dapp.zip"),
        [SiteWarning::RiskyTld { tld: "zip".into() }]
    );
}

#[test]
fn never_flags_a_protected_domain_or_its_subdomains() {
    let connected = SiteContext {
        connected: true,
        ..KNOWN
    };
    for origin in ["https://app.starknet.io", "https://argent.xyz"] {
        let insight = origin_insights(&LISTS, &none(), origin, connected);
        assert!(!insight.blocked);
        assert_eq!(insight.warnings, [SiteWarning::ConnectedBefore], "{origin}");
    }
}

#[test]
fn reports_first_seen_prior_connection_and_recency() {
    let at = |context| origin_insights(&LISTS, &none(), "https://example.com", context).warnings;
    let unseen = SiteContext {
        connected: false,
        first_seen_ms: None,
        now_ms: 5.0,
    };
    assert_eq!(at(unseen), [SiteWarning::FirstSeen]);
    assert_eq!(
        at(SiteContext {
            connected: true,
            ..unseen
        }),
        [SiteWarning::ConnectedBefore]
    );
    let recent = SiteContext {
        connected: false,
        first_seen_ms: Some(1_000.0),
        now_ms: 1_000.0 + DAY_MS - 1.0,
    };
    assert_eq!(at(recent), [SiteWarning::NewSite]);
    // A known, long-seen, reputable site is quiet.
    assert_eq!(at(KNOWN), []);
}

#[test]
fn an_unparseable_origin_warns_without_blocking() {
    let insight = origin_insights(&LISTS, &none(), "not a url", KNOWN);
    assert!(!insight.blocked);
    assert_eq!(insight.warnings, [SiteWarning::Unparseable]);
}

#[test]
fn edit_distance_is_at_most_one() {
    assert!(within_one_edit(b"starknet.io", b"starknet.io"));
    // A transposition is two edits.
    assert!(!within_one_edit(b"starknet.io", b"starkent.io"));
    assert!(within_one_edit(b"starknet.io", b"starknt.io"));
    assert!(within_one_edit(b"starknet.io", b"starknet.iox"));
    assert!(!within_one_edit(b"starknet.io", b"starknet.ioxx"));
}
