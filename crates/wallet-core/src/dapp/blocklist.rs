//! Whether a site is blocked, and what else the wallet warns about it: the extension's
//! `packages/features/permissions/src/blocklist.ts`. Pure: the wallet passes its lists
//! (`SiteLists`), the live feed's domains (`phishing_feed`) and the site's reputation
//! (`origin_history`). Only an exact known-bad domain, or a subdomain of one, blocks; lookalikes,
//! punycode and risky top-level domains are warnings the person still decides on.
//!
//! The warnings are codes, not text: the app renders them in the reader's language, as the
//! extension's `insight-messages.ts` does.

use std::collections::HashSet;

use crate::nostr::relay_url::is_custom_rpc_url;

const DAY_MS: f64 = 86_400_000.0;

/// The wallet's own lists: the bundled known-bad domains (always consulted, so protection never
/// depends on the feed), the sites phishing domains imitate, and the top-level domains that only
/// warn.
#[derive(Debug, Clone, Copy)]
pub struct SiteLists {
    pub blocked: &'static [&'static str],
    pub protected: &'static [&'static str],
    pub risky_tlds: &'static [&'static str],
}

/// `OriginContext`: what the wallet knows of the site.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SiteContext {
    /// A grant covers the site.
    pub connected: bool,
    /// When the wallet first saw it (`origin_history`), or `None` for never.
    pub first_seen_ms: Option<f64>,
    pub now_ms: f64,
}

/// One of `insight-messages.ts`'s warnings, with its facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SiteWarning {
    /// `siteInsight.unparseable`
    Unparseable,
    /// `siteInsight.blocklisted`
    Blocklisted,
    /// `siteInsight.punycode`
    Punycode,
    /// `siteInsight.lookalike`: the protected domain it resembles.
    Lookalike { target: String },
    /// `siteInsight.riskyTld`
    RiskyTld { tld: String },
    /// `siteInsight.connectedBefore`
    ConnectedBefore,
    /// `siteInsight.firstSeen`
    FirstSeen,
    /// `siteInsight.newSite`: first seen less than a day ago.
    NewSite,
}

/// `OriginInsight`: whether the site is blocked, and the warnings in the extension's order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteInsight {
    pub blocked: bool,
    pub warnings: Vec<SiteWarning>,
}

/// The host of a wallet origin (`https://host[:port]`, or `http` on loopback, as
/// `new URL(origin).origin` prints it), without one trailing dot so `example.com.` matches
/// `example.com`. `None` for anything else, including an origin with a path.
pub fn origin_host(origin: &str) -> Option<&str> {
    let authority = origin
        .strip_prefix("https://")
        .or_else(|| origin.strip_prefix("http://"))?;
    if authority.contains(['/', '?', '#']) || !is_custom_rpc_url(origin) {
        return None;
    }
    // The grammar admits only a bracketed IPv6 literal with colons, or one `:port`.
    let host = match authority.rfind(':').filter(|_| !authority.ends_with(']')) {
        Some(index) => &authority[..index],
        None => authority,
    };
    Some(host.strip_suffix('.').unwrap_or(host))
}

fn matches_base(host: &str, base: &str) -> bool {
    host == base
        || host
            .strip_suffix(base)
            .is_some_and(|head| head.ends_with('.'))
}

// The host or any parent domain is in the feed: O(labels) lookups, not O(entries).
fn in_feed(host: &str, feed: &HashSet<String>) -> bool {
    let mut candidate = host;
    loop {
        if feed.contains(candidate) {
            return true;
        }
        match candidate.split_once('.') {
            Some((_, parent)) if !parent.is_empty() => candidate = parent,
            _ => return false,
        }
    }
}

fn host_blocked(lists: &SiteLists, feed: &HashSet<String>, host: &str) -> bool {
    lists.blocked.iter().any(|base| matches_base(host, base)) || in_feed(host, feed)
}

/// `isBlockedOrigin`: the site's host, or a parent of it, is on the bundled list or the feed. An
/// origin that does not parse is not blocked here; the bridge refuses it before this.
pub fn is_blocked_origin(lists: &SiteLists, feed: &HashSet<String>, origin: &str) -> bool {
    origin_host(origin).is_some_and(|host| host_blocked(lists, feed, host))
}

/// `originInsights`: blocked or not, with the blocklist, punycode, lookalike, risky-TLD and
/// reputation warnings in that order.
pub fn origin_insights(
    lists: &SiteLists,
    feed: &HashSet<String>,
    origin: &str,
    context: SiteContext,
) -> SiteInsight {
    let Some(host) = origin_host(origin) else {
        return SiteInsight {
            blocked: false,
            warnings: vec![SiteWarning::Unparseable],
        };
    };
    let blocked = host_blocked(lists, feed, host);
    let mut warnings = Vec::new();
    if blocked {
        warnings.push(SiteWarning::Blocklisted);
    }
    if host.split('.').any(|label| label.starts_with("xn--")) {
        warnings.push(SiteWarning::Punycode);
    }
    if let Some(target) = lookalike_target(lists, host) {
        warnings.push(SiteWarning::Lookalike {
            target: target.to_owned(),
        });
    }
    if let Some(tld) = host
        .rsplit('.')
        .next()
        .filter(|tld| lists.risky_tlds.contains(tld))
    {
        warnings.push(SiteWarning::RiskyTld {
            tld: tld.to_owned(),
        });
    }
    if context.connected {
        warnings.push(SiteWarning::ConnectedBefore);
    } else if let Some(first) = context.first_seen_ms {
        if context.now_ms - first < DAY_MS {
            warnings.push(SiteWarning::NewSite);
        }
    } else {
        warnings.push(SiteWarning::FirstSeen);
    }
    SiteInsight { blocked, warnings }
}

fn lookalike_target(lists: &SiteLists, host: &str) -> Option<&'static str> {
    if lists.protected.iter().any(|base| matches_base(host, base)) {
        return None;
    }
    lists
        .protected
        .iter()
        .copied()
        .find(|domain| is_lookalike(host, domain))
}

fn is_lookalike(host: &str, domain: &str) -> bool {
    host != domain
        && (host.replace('-', "") == domain
            || fold_digits(host) == domain
            || host.contains(domain)
            || within_one_edit(host.as_bytes(), domain.as_bytes()))
}

// `DIGIT_FOLD`: `st4rknet.io` reads as `starknet.io`.
fn fold_digits(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '0' => 'o',
            '1' => 'l',
            '3' => 'e',
            '4' => 'a',
            '5' => 's',
            '6' | '8' => 'b',
            '7' => 't',
            '9' => 'g',
            other => other,
        })
        .collect()
}

// `levenshteinWithin1`. Hosts are ASCII, so bytes are the extension's UTF-16 units.
fn within_one_edit(a: &[u8], b: &[u8]) -> bool {
    if a.len().abs_diff(b.len()) > 1 {
        return false;
    }
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, &x) in a.iter().enumerate() {
        let mut current = vec![i + 1];
        for (j, &y) in b.iter().enumerate() {
            let cost = usize::from(x != y);
            current.push(
                (previous[j + 1] + 1)
                    .min(current[j] + 1)
                    .min(previous[j] + cost),
            );
        }
        previous = current;
    }
    previous[b.len()] <= 1
}

#[cfg(test)]
#[path = "blocklist_tests.rs"]
mod tests;
