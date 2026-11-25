//! Ported from the extension's `packages/features/permissions/tests/phishing-feed.test.ts`,
//! `phishing-feed-health.test.ts`, `phishing-feed-body-bounds.test.ts` and
//! `phishing-feed-store-errors.test.ts`. The chunk-layout cases have no counterpart: the cache
//! here is one file.

use super::*;
use crate::host::testing::Memory;
use crate::Error;

const URL: &str = "https://feed.example/config.json";
const FILE: &str = "phishing-feed.json";
const TTL: f64 = 6.0 * HOUR_MS;

const POLICY: FeedPolicy = FeedPolicy {
    url: URL,
    file: FILE,
    ttl_ms: TTL,
    stale_after_ms: 4.0 * TTL,
    max_domains: 150_000,
    max_domain_chars: 4 * 1024 * 1024,
    max_body_bytes: 8 * 1024 * 1024,
};

fn ok(body: &str) -> HttpOutcome {
    HttpOutcome {
        status: 200,
        retry_after: None,
        body: Some(body.as_bytes().to_vec()),
        timed_out: false,
    }
}

fn parsed(body: &str) -> Option<Vec<String>> {
    parse_feed(&POLICY, body.as_bytes())
}

fn sorted(feed: &PhishingFeed) -> Vec<String> {
    let mut domains: Vec<String> = feed.domains().iter().cloned().collect();
    domains.sort();
    domains
}

// A feed that fetched `domains` at `at` into `storage`.
fn fetched(storage: &Memory, domains: &str, at: f64) -> PhishingFeed {
    let mut feed = PhishingFeed::new(POLICY);
    assert!(feed.refresh(storage, at).is_some());
    feed.answer(storage, &ok(&format!(r#"{{"blocklist":{domains}}}"#)), at);
    feed
}

#[test]
fn parses_lowercases_strips_trailing_dots_and_dedupes() {
    assert_eq!(
        parsed(r#"{"blocklist":["Evil.COM","evil.com."," phish.example\n"]}"#),
        Some(vec!["evil.com".into(), "phish.example".into()])
    );
    assert_eq!(
        parsed(r#"{"blacklist":["drainer.xyz"],"fuzzylist":["x"]}"#),
        Some(vec!["drainer.xyz".into()])
    );
    assert_eq!(
        parsed(r#"{"blocklist":null,"blacklist":["drainer.xyz"]}"#),
        Some(vec!["drainer.xyz".into()])
    );
    assert_eq!(
        parsed(r#"{"blocklist":["good.com",42,"no-dot","bad_domain.com","a.b.c"]}"#),
        Some(vec!["good.com".into(), "a.b.c".into()])
    );
    assert_eq!(parsed(r#"{"blocklist":[]}"#), Some(vec![]));
    for body in [
        "null",
        r#"{"nope":true}"#,
        r#"{"blocklist":"x"}"#,
        "[]",
        "{",
        "",
    ] {
        assert_eq!(parsed(body), None, "{body}");
    }
}

#[test]
fn stops_at_either_cap() {
    let few = FeedPolicy {
        max_domains: 2,
        ..POLICY
    };
    let body = br#"{"blocklist":["a.com","b.com","c.com"]}"#;
    assert_eq!(parse_feed(&few, body).map(|d| d.len()), Some(2));
    // `a.com` costs 8, so 17 characters hold two.
    let short = FeedPolicy {
        max_domain_chars: 17,
        ..POLICY
    };
    assert_eq!(parse_feed(&short, body).map(|d| d.len()), Some(2));
    let long = format!("{}.com", "a".repeat(250));
    assert_eq!(normalize_domain(&long), None);
    assert!(normalize_domain(&format!("{}.com", "a".repeat(249))).is_some());
}

#[test]
fn fetches_caches_and_serves_the_cache_until_the_ttl() {
    let storage = Memory::default();
    let feed = fetched(&storage, r#"["feed-bad.com"]"#, 1_000.0);
    assert_eq!(sorted(&feed), ["feed-bad.com"]);
    let stored = storage.0.borrow().get(FILE).cloned().unwrap_or_default();
    let value: Value = serde_json::from_slice(&stored).unwrap_or_default();
    assert_eq!(value["source"], URL);
    assert_eq!(value["domains"][0], "feed-bad.com");

    // A new instance reads the cache and asks nothing while it is fresh.
    let mut restarted = PhishingFeed::new(POLICY);
    assert_eq!(restarted.refresh(&storage, 1_000.0 + TTL - 1.0), None);
    assert_eq!(sorted(&restarted), ["feed-bad.com"]);
    // Past the TTL it asks, still holding the cached list meanwhile.
    let request = restarted.refresh(&storage, 1_000.0 + TTL);
    assert_eq!(request.as_ref().map(|r| r.url.as_str()), Some(URL));
    assert_eq!(
        request.map(|r| r.max_body_bytes),
        Some(POLICY.max_body_bytes)
    );
    assert_eq!(sorted(&restarted), ["feed-bad.com"]);
    restarted.answer(
        &storage,
        &ok(r#"{"blocklist":["fresh.com"]}"#),
        1_000.0 + TTL,
    );
    assert_eq!(sorted(&restarted), ["fresh.com"]);
}

#[test]
fn a_failed_or_malformed_answer_keeps_the_last_good_list() {
    let storage = Memory::default();
    let mut feed = fetched(&storage, r#"["cached-bad.com"]"#, 0.0);
    let failures = [
        HttpOutcome {
            status: 0,
            retry_after: None,
            body: None,
            timed_out: true,
        },
        HttpOutcome {
            status: 500,
            ..ok(r#"{"blocklist":["x.com"]}"#)
        },
        ok("not json"),
        ok(r#"{"nope":1}"#),
        // Over the body budget.
        ok(&format!(
            r#"{{"blocklist":["{}.com"]}}"#,
            "a".repeat(9 * 1024 * 1024)
        )),
    ];
    for outcome in &failures {
        feed.answer(&storage, outcome, TTL);
        assert_eq!(sorted(&feed), ["cached-bad.com"]);
    }
    assert_eq!(feed.health(TTL).consecutive_failures, 5);
    // With no cache, a failure leaves the feed empty.
    let mut empty = PhishingFeed::new(POLICY);
    let storage = Memory::default();
    assert!(empty.refresh(&storage, 0.0).is_some());
    empty.answer(&storage, &failures[0], 0.0);
    assert!(empty.domains().is_empty());
}

#[test]
fn a_cache_that_does_not_check_out_is_no_cache() {
    let good = format!(
        r#"{{"schemaVersion":1,"source":"{URL}","fetchedAt":10000,"domains":["cached-bad.com"]}}"#
    );
    let bad = [
        good.replace(r#""domains""#, r#""extra":true,"domains""#),
        good.replace(r#""schemaVersion":1"#, r#""schemaVersion":2"#),
        good.replace(URL, "https://attacker.example/list.json"),
        good.replace("10000", "20001"),
        good.replace(r#"["cached-bad.com"]"#, r#"["good.example","com"]"#),
        good.replace(r#"["cached-bad.com"]"#, r#"["bad.example","bad.example"]"#),
        good.replace(r#"["cached-bad.com"]"#, r#"["BAD.EXAMPLE"]"#),
        "[]".to_owned(),
    ];
    let storage = Memory::default();
    storage
        .0
        .borrow_mut()
        .insert(FILE.into(), good.into_bytes());
    let mut feed = PhishingFeed::new(POLICY);
    assert_eq!(feed.refresh(&storage, 20_000.0), None);
    assert_eq!(sorted(&feed), ["cached-bad.com"]);
    for cache in bad {
        let storage = Memory::default();
        storage
            .0
            .borrow_mut()
            .insert(FILE.into(), cache.clone().into_bytes());
        let mut feed = PhishingFeed::new(POLICY);
        assert!(feed.refresh(&storage, 20_000.0).is_some(), "{cache}");
        assert!(feed.domains().is_empty(), "{cache}");
    }
}

#[test]
fn health_counts_hours_failures_and_staleness() {
    let feed = PhishingFeed::new(POLICY);
    assert_eq!(
        feed.health(0.0),
        FeedHealth {
            age_hours: None,
            cache_write_failed: false,
            consecutive_failures: 0,
            domains: 0,
            stale: true,
        }
    );
    let storage = Memory::default();
    let mut feed = fetched(&storage, r#"["a.com","b.com"]"#, 0.0);
    assert_eq!(feed.health(0.0).age_hours, Some(0));
    assert!(!feed.health(24.0 * HOUR_MS).stale);
    assert!(feed.health(24.0 * HOUR_MS + 1.0).stale);
    // Failures keep the list's real age and reset on success.
    feed.answer(&storage, &ok("x"), 7.0 * HOUR_MS);
    feed.answer(&storage, &ok("x"), 7.0 * HOUR_MS);
    let health = feed.health(7.0 * HOUR_MS);
    assert_eq!(
        (health.age_hours, health.consecutive_failures),
        (Some(7), 2)
    );
    feed.answer(&storage, &ok(r#"{"blocklist":[]}"#), 8.0 * HOUR_MS);
    let health = feed.health(8.0 * HOUR_MS);
    assert_eq!(
        (health.age_hours, health.consecutive_failures),
        (Some(0), 0)
    );
    // A list restored from the cache reports the cache's age.
    let storage = Memory::default();
    let _ = fetched(&storage, r#"["a.com","b.com"]"#, 0.0);
    let mut restarted = PhishingFeed::new(POLICY);
    assert_eq!(restarted.refresh(&storage, 3.0 * HOUR_MS), None);
    assert_eq!(restarted.health(3.0 * HOUR_MS).age_hours, Some(3));
}

struct Full;

impl Storage for Full {
    fn read(&self, _: &str) -> Result<Option<Vec<u8>>, Error> {
        Ok(None)
    }
    fn write(&self, _: &str, _: &[u8]) -> Result<(), Error> {
        Err(Error::storage("full"))
    }
    fn rename(&self, _: &str, _: &str) -> Result<(), Error> {
        Err(Error::storage("full"))
    }
}

#[test]
fn a_failed_cache_write_is_reported_and_the_list_still_used() {
    let mut feed = PhishingFeed::new(POLICY);
    assert!(feed.refresh(&Full, 0.0).is_some());
    feed.answer(&Full, &ok(r#"{"blocklist":["a.com","b.com"]}"#), 0.0);
    let health = feed.health(0.0);
    assert!(health.cache_write_failed);
    assert_eq!((health.domains, health.stale), (2, false));
    let storage = Memory::default();
    feed.answer(&storage, &ok(r#"{"blocklist":["a.com"]}"#), 0.0);
    assert!(!feed.health(0.0).cache_write_failed);
}
