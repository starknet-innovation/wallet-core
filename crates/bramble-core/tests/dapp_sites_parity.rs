//! P16's `site`, `feedParse` and `feed` blocks (`fixtures/extension-dapp.jsonl`):
//!
//! - `site`: `is_blocked_origin` and `origin_insights` over Bramble's `SITE_LISTS` and the same
//!   feed snapshot block and warn as the extension's `isBlockedOrigin` and `originInsights`, each
//!   warning the extension's message key with its facts;
//! - `feedParse`: `parse_feed` and `normalize_domain` read each body and entry as
//!   `parsePhishingFeed` and `normalizePhishingDomain` do;
//! - `feed`: `PhishingFeed` under `PHISHING_FEED`, refreshed at the same clock against the same
//!   replies (a restart is a new feed over the same storage), fetches when the extension fetched
//!   and holds the same list and health after each step.

mod dapp_parity_support;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use bramble_core::dapp::{PHISHING_FEED, SITE_LISTS};
use dapp_parity_support::*;
use serde_json::{json, Value};
use starknet_wallet_core::dapp::blocklist::{is_blocked_origin, origin_insights};
use starknet_wallet_core::dapp::phishing_feed::{normalize_domain, parse_feed};
use starknet_wallet_core::dapp::{PhishingFeed, SiteContext, SiteWarning};
use starknet_wallet_core::host::Storage;
use starknet_wallet_core::portfolio::wire::HttpOutcome;
use starknet_wallet_core::Error;

/// A warning as `insight-messages.ts` keys it.
fn warning(warning: &SiteWarning) -> Value {
    let (key, facts) = match warning {
        SiteWarning::Unparseable => ("unparseable", json!({})),
        SiteWarning::Blocklisted => ("blocklisted", json!({})),
        SiteWarning::Punycode => ("punycode", json!({})),
        SiteWarning::Lookalike { target } => ("lookalike", json!({ "target": target })),
        SiteWarning::RiskyTld { tld } => ("riskyTld", json!({ "tld": tld })),
        SiteWarning::ConnectedBefore => ("connectedBefore", json!({})),
        SiteWarning::FirstSeen => ("firstSeen", json!({})),
        SiteWarning::NewSite => ("newSite", json!({})),
    };
    json!({ "key": format!("siteInsight.{key}"), "facts": facts })
}

#[test]
fn each_site_is_blocked_and_warned_about_as_the_extension_does() {
    for case in cases("site", 37) {
        let name = name(&case);
        let origin = text(&case["origin"]);
        let feed: HashSet<String> = case["feed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|domain| text(domain).to_owned())
            .collect();
        let context = &case["context"];
        let context = SiteContext {
            connected: context["connected"] == true,
            first_seen_ms: context["firstSeenAt"].as_f64(),
            now_ms: context["now"].as_f64().unwrap_or(0.0),
        };
        assert_eq!(
            is_blocked_origin(&SITE_LISTS, &feed, origin),
            case["blocked"] == true,
            "{name}"
        );
        let insight = origin_insights(&SITE_LISTS, &feed, origin, context);
        let warnings: Vec<Value> = insight.warnings.iter().map(warning).collect();
        assert_eq!(
            json!({ "blocked": insight.blocked, "warnings": warnings }),
            case["insight"],
            "{name}"
        );
    }
}

#[test]
fn each_feed_body_and_entry_is_read_as_the_extension_reads_it() {
    for case in cases("feedParse", 19) {
        let name = name(&case);
        if let Some(body) = case["body"].as_str() {
            assert_eq!(
                json!(parse_feed(&PHISHING_FEED, body.as_bytes())),
                case["domains"],
                "{name}"
            );
        } else {
            assert_eq!(
                json!(normalize_domain(text(&case["value"]))),
                case["normalized"],
                "{name}"
            );
        }
    }
}

/// The host's files, in memory.
#[derive(Default)]
struct Memory(RefCell<HashMap<String, Vec<u8>>>);

impl Storage for Memory {
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, Error> {
        Ok(self.0.borrow().get(key).cloned())
    }
    fn write(&self, key: &str, value: &[u8]) -> Result<(), Error> {
        self.0.borrow_mut().insert(key.into(), value.to_vec());
        Ok(())
    }
    fn rename(&self, from: &str, to: &str) -> Result<(), Error> {
        let value = self.0.borrow_mut().remove(from).expect("a value to move");
        self.0.borrow_mut().insert(to.into(), value);
        Ok(())
    }
}

/// What the host saw for a scripted reply: a body and its status, or a transport failure.
fn outcome(reply: &Value) -> HttpOutcome {
    if reply["throws"] == true {
        return HttpOutcome {
            status: 0,
            retry_after: None,
            body: None,
            timed_out: false,
        };
    }
    HttpOutcome {
        status: reply["status"].as_u64().unwrap() as u16,
        retry_after: None,
        body: Some(text(&reply["body"]).as_bytes().to_vec()),
        timed_out: false,
    }
}

#[test]
fn the_feed_refreshes_caches_and_reports_health_as_the_extension_does() {
    for case in cases("feed", 4) {
        let name = name(&case);
        let storage = Memory::default();
        let mut feed = PhishingFeed::new(PHISHING_FEED);
        for entry in case["steps"].as_array().unwrap() {
            let step = &entry["step"];
            if step["op"] == "restart" {
                feed = PhishingFeed::new(PHISHING_FEED);
                continue;
            }
            let at = step["at"].as_f64().unwrap();
            let fetched = match feed.refresh(&storage, at) {
                Some(request) => {
                    assert_eq!(request.url, PHISHING_FEED.url, "{name}");
                    feed.answer(&storage, &outcome(&step["reply"]), at);
                    true
                }
                None => false,
            };
            let mut domains: Vec<&String> = feed.domains().iter().collect();
            domains.sort();
            let health = feed.health(at);
            let ours = json!({
                "domains": domains,
                "fetched": fetched,
                "health": {
                    "ageHours": health.age_hours,
                    "cacheWriteFailed": health.cache_write_failed,
                    "consecutiveFailures": health.consecutive_failures,
                    "domains": health.domains,
                    "stale": health.stale,
                },
            });
            let theirs = json!({
                "domains": entry["domains"],
                "fetched": entry["fetched"],
                "health": entry["health"],
            });
            assert_eq!(ours, theirs, "{name}: {step}");
        }
    }
}
