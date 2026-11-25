//! The live phishing feed: the extension's `packages/features/permissions/src/phishing-feed.ts`
//! and its cache (`phishing-feed-cache.ts`), sans-IO. `refresh` promotes the cached list and
//! returns the request to make when it is older than the TTL; `answer` judges the reply, keeps
//! the last good list on any failure, and caches a new one. The bundled list in `SiteLists` is
//! consulted apart from this, so protection never drops below it.
//!
//! The wire format is `{ "blocklist": string[] }`, or MetaMask's `eth-phishing-detect`
//! `config.json`, whose `blacklist` is read as an alias and whose other fields are ignored. Each
//! entry blocks a domain and its subdomains. The cache is one file, not the extension's chunks:
//! those exist only for `chrome.storage`'s per-write budget.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::host::Storage;
use crate::js::num::js_trim;
use crate::portfolio::wire::{HttpMethod, HttpOutcome, HttpRequest, HTTP_DEADLINE_MS};

const HOUR_MS: f64 = 3_600_000.0;
/// RFC 1035's bound on a DNS name.
const MAX_DOMAIN_LENGTH: usize = 253;
const CACHE_SCHEMA: u32 = 1;

/// The wallet's feed: where it is, where it is cached, and its bounds.
#[derive(Debug, Clone, Copy)]
pub struct FeedPolicy {
    pub url: &'static str,
    pub file: &'static str,
    /// Refetch only when the list is older than this.
    pub ttl_ms: f64,
    /// `health` reports a list older than this stale.
    pub stale_after_ms: f64,
    pub max_domains: usize,
    /// Every domain counted as its JSON string: its characters, two quotes and a separator.
    pub max_domain_chars: usize,
    pub max_body_bytes: u64,
}

/// `PhishingFeedHealth`: counts and whole hours only, for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeedHealth {
    /// The age of the list in use, or `None` before one has loaded.
    pub age_hours: Option<u64>,
    pub cache_write_failed: bool,
    /// Fetches that failed since the last success.
    pub consecutive_failures: u32,
    pub domains: usize,
    /// Nothing has loaded, or the list is older than `stale_after_ms`.
    pub stale: bool,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Cache {
    schema_version: u32,
    source: String,
    fetched_at: f64,
    domains: Vec<String>,
}

/// `normalizePhishingDomain`: trimmed, lowercased, one trailing dot dropped; `None` unless a
/// dotted name of `[a-z0-9.-]`, at most 253 characters.
pub fn normalize_domain(value: &str) -> Option<String> {
    let lower = js_trim(value).to_lowercase();
    let domain = lower.strip_suffix('.').unwrap_or(&lower);
    (domain.len() <= MAX_DOMAIN_LENGTH
        && domain.contains('.')
        && domain
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'.' || b == b'-'))
    .then(|| domain.to_owned())
}

/// `parsePhishingFeed`: the normalized, de-duplicated domains, up to both caps, skipping
/// malformed entries. `None` when the body is not an object with a `blocklist` (or `blacklist`)
/// array; an empty list is a list.
pub fn parse_feed(policy: &FeedPolicy, body: &[u8]) -> Option<Vec<String>> {
    let Value::Object(body) = serde_json::from_slice::<Value>(body).ok()? else {
        return None;
    };
    // `body.blocklist ?? body.blacklist`
    let raw = match body.get("blocklist") {
        None | Some(Value::Null) => body.get("blacklist")?,
        Some(list) => list,
    }
    .as_array()?;
    let mut seen = HashSet::new();
    let mut domains = Vec::new();
    let mut chars = 0;
    for domain in raw
        .iter()
        .filter_map(|e| e.as_str().and_then(normalize_domain))
    {
        if seen.contains(&domain) {
            continue;
        }
        chars += domain.len() + 3;
        if chars > policy.max_domain_chars {
            break;
        }
        seen.insert(domain.clone());
        domains.push(domain);
        if domains.len() >= policy.max_domains {
            break;
        }
    }
    Some(domains)
}

/// `createPhishingFeed`: the list in use, and how it got there.
#[derive(Debug, Clone)]
pub struct PhishingFeed {
    policy: FeedPolicy,
    domains: HashSet<String>,
    loaded_at: Option<f64>,
    failures: u32,
    write_failed: bool,
}

impl PhishingFeed {
    pub fn new(policy: FeedPolicy) -> Self {
        Self {
            policy,
            domains: HashSet::new(),
            loaded_at: None,
            failures: 0,
            write_failed: false,
        }
    }

    /// `blockedDomains`: what `is_blocked_origin` checks besides the bundled list.
    pub fn domains(&self) -> &HashSet<String> {
        &self.domains
    }

    /// `refresh`'s first half: promotes the cached list, then returns the request to make when
    /// there is none or it is older than the TTL. The host makes it with no cookies, no referrer
    /// and no redirects, and passes the outcome to `answer`.
    pub fn refresh(&mut self, storage: &dyn Storage, now_ms: f64) -> Option<HttpRequest> {
        if let Some(cache) = self.read_cache(storage, now_ms) {
            let fresh = now_ms - cache.fetched_at < self.policy.ttl_ms;
            self.set(cache.domains, cache.fetched_at);
            if fresh {
                return None;
            }
        }
        Some(HttpRequest {
            method: HttpMethod::Get,
            url: self.policy.url.to_owned(),
            headers: Vec::new(),
            body: None,
            max_body_bytes: self.policy.max_body_bytes,
            deadline_ms: HTTP_DEADLINE_MS,
        })
    }

    /// `refresh`'s second half. A failed exchange, a non-2xx status, an over-budget or malformed
    /// body keeps the list in use; a good one replaces it and is cached. A failed cache write is
    /// recorded for `health`, and the new list is used either way.
    pub fn answer(&mut self, storage: &dyn Storage, outcome: &HttpOutcome, now_ms: f64) {
        let fetched = outcome
            .body
            .as_deref()
            .filter(|body| {
                (200..300).contains(&outcome.status)
                    && body.len() as u64 <= self.policy.max_body_bytes
            })
            .and_then(|body| parse_feed(&self.policy, body));
        let Some(domains) = fetched else {
            self.failures = self.failures.saturating_add(1);
            return;
        };
        self.failures = 0;
        let cache = Cache {
            schema_version: CACHE_SCHEMA,
            source: self.policy.url.to_owned(),
            fetched_at: now_ms,
            domains,
        };
        self.write_failed = serde_json::to_vec(&cache)
            .ok()
            .is_none_or(|body| storage.write(self.policy.file, &body).is_err());
        self.set(cache.domains, now_ms);
    }

    /// `health`.
    pub fn health(&self, now_ms: f64) -> FeedHealth {
        let age = self.loaded_at.map(|at| (now_ms - at).max(0.0));
        FeedHealth {
            // Whole hours of a non-negative, finite age.
            age_hours: age.map(|age| (age / HOUR_MS).floor() as u64),
            cache_write_failed: self.write_failed,
            consecutive_failures: self.failures,
            domains: self.domains.len(),
            stale: age.is_none_or(|age| age > self.policy.stale_after_ms),
        }
    }

    fn set(&mut self, domains: Vec<String>, fetched_at: f64) {
        self.domains = domains.into_iter().collect();
        self.loaded_at = Some(fetched_at);
    }

    // `readPhishingCache`: fails closed. Another source, a time after `now_ms`, a malformed,
    // duplicate or over-cap entry, or any read failure is no cache, never part of one.
    fn read_cache(&self, storage: &dyn Storage, now_ms: f64) -> Option<Cache> {
        let bytes = storage.read(self.policy.file).ok()??;
        let cache: Cache = serde_json::from_slice(&bytes).ok()?;
        let mut seen = HashSet::new();
        let valid = cache.schema_version == CACHE_SCHEMA
            && cache.source == self.policy.url
            && cache.fetched_at.is_finite()
            && (0.0..=now_ms).contains(&cache.fetched_at)
            && cache.domains.len() <= self.policy.max_domains
            && cache.domains.iter().all(|domain| {
                normalize_domain(domain).as_ref() == Some(domain) && seen.insert(domain.as_str())
            });
        valid.then_some(cache)
    }
}

#[cfg(test)]
#[path = "phishing_feed_tests.rs"]
mod tests;
