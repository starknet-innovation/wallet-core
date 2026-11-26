//! What `DappSession` knows about a site: whether the blocklist refuses it (Bramble's bundled
//! `SITE_LISTS` and the live phishing feed), and the warnings a card shows. The extension's
//! `background/index.ts` `originGuard` and `phishingFeed`, `approvals/approval-insights.ts`, and
//! the first-seen record its approve and deny keep.
//!
//! The feed lives in memory and in `PHISHING_FEED.file`. Until the host's first
//! `refresh_phishing_feed` only the bundled list blocks, so the host calls it at start and on a
//! timer; it is cheap while the cached list is fresh. Times are the host's wall clock (the core
//! has none), as `PriceBook`'s are.

use starknet_wallet_core::dapp::blocklist::{is_blocked_origin, origin_insights};
use starknet_wallet_core::dapp::{OriginHistory, SiteContext, SiteInsight, SiteWarning};

use crate::dapp::{MAX_ORIGIN_HISTORY, ORIGIN_HISTORY_FILE, SITE_LISTS};
use crate::dapp_session::DappSession;
use crate::files::Directory;
use crate::portfolio::wire::{HttpOutcome, HttpRequest};
use crate::wallet::guard;

/// One of the site's warnings, in the extension's order; the app renders it in the reader's
/// language (`insight-messages.ts`).
#[uniffi::remote(Enum)]
pub enum SiteWarning {
    Unparseable,
    Blocklisted,
    Punycode,
    /// The protected domain it resembles.
    Lookalike {
        target: String,
    },
    RiskyTld {
        tld: String,
    },
    ConnectedBefore,
    FirstSeen,
    /// First seen less than a day ago.
    NewSite,
}

impl DappSession {
    /// `originGuard.isBlocked`: the bundled list, or the feed as last loaded.
    pub(crate) fn blocked(&self, origin: &str) -> bool {
        is_blocked_origin(&SITE_LISTS, guard(&self.feed).domains(), origin)
    }

    fn with_history<T>(&self, f: impl FnOnce(&OriginHistory<'_>) -> T) -> T {
        let storage = Directory(self.wallet.data_dir());
        f(&OriginHistory {
            storage: &storage,
            file: ORIGIN_HISTORY_FILE,
            max_origins: MAX_ORIGIN_HISTORY,
        })
    }

    /// `enrichApprovals`: a card's site as the person is shown it. A card is never for a
    /// connected site, and an unreadable history reads as never seen.
    pub(crate) fn insight(&self, origin: &str, now_ms: f64) -> SiteInsight {
        let first_seen_ms = self
            .with_history(|history| history.first_seen(origin))
            .ok()
            .flatten();
        let context = SiteContext {
            connected: false,
            first_seen_ms,
            now_ms,
        };
        origin_insights(&SITE_LISTS, guard(&self.feed).domains(), origin, context)
    }

    /// Notes the site as seen once the person has decided its card. Best effort, as the
    /// extension's: a failed write only makes the site look new next time.
    // ponytail: the extension also records a site on every granted `requestAccounts`, which only
    // re-adds one pruned past `MAX_ORIGIN_HISTORY`; that needs the wall clock on `request`.
    pub(crate) fn record_seen(&self, origin: &str, now_ms: f64) {
        let _ = self.with_history(|history| history.record(origin, now_ms));
    }
}

#[uniffi::export]
impl DappSession {
    /// Loads the cached phishing list, then returns the request to make when there is none or it
    /// is older than six hours; `None` means the list is fresh. The host performs it exactly as
    /// described and passes the outcome to `answer_phishing_feed`.
    pub fn refresh_phishing_feed(&self, now_ms: f64) -> Option<HttpRequest> {
        let storage = Directory(self.wallet.data_dir());
        guard(&self.feed).refresh(&storage, now_ms)
    }

    /// Takes the feed's reply: a good list replaces the one in use and is cached; anything else
    /// keeps the last good list.
    pub fn answer_phishing_feed(&self, outcome: HttpOutcome, now_ms: f64) {
        let storage = Directory(self.wallet.data_dir());
        guard(&self.feed).answer(&storage, &outcome, now_ms);
    }

    /// Whether the blocklist refuses `origin` (the bundled list, or the feed as last loaded), for
    /// the in-app browser to stop a listed page before it renders. An origin it cannot read is
    /// not blocked here; the host decides what to load. A mobile addition: the extension blocks
    /// only a site's requests, and has no navigation check.
    pub fn is_blocked_site(&self, origin: String) -> bool {
        self.blocked(&origin)
    }
}
