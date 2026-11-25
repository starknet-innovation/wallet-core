//! A dapp's requests to the wallet, whatever carried them (an in-app browser's bridge,
//! WalletConnect): the extension's provider rules (`packages/foundation/wallet-protocol/src/
//! provider/`) as sans-IO mechanism. The wallet's numbers (budgets, the versions it advertises)
//! are passed in.
//!
//! So far: the hostile-JSON budget (`budget`), the wallet API's error codes (`error`), and
//! reading a request's method and params (`request`; an invoke's calls in `invoke`, a chain in
//! `add_chain`, an asset in `watch_asset`), and which accounts a site may see (`grants`), and the
//! approval queue (`queue`); whether a site is blocked and what else the wallet warns about it
//! (`blocklist`, over the live feed in `phishing_feed` and the first-seen record in
//! `origin_history`). Review follows.

pub mod add_chain;
pub mod blocklist;
pub mod budget;
pub mod error;
pub mod grants;
pub mod invoke;
pub mod origin_history;
pub mod phishing_feed;
pub mod queue;
pub mod request;
pub mod watch_asset;

pub use add_chain::ChainRequest;
pub use blocklist::{SiteContext, SiteInsight, SiteLists, SiteWarning};
pub use budget::JsonBudget;
pub use error::WalletRpcError;
pub use grants::OriginGrant;
pub use invoke::InvokeLimits;
pub use origin_history::OriginHistory;
pub use phishing_feed::{FeedHealth, FeedPolicy, PhishingFeed};
pub use queue::{ApprovalId, ApprovalQueue, QueueLimits};
pub use request::{read_request, Method, Request, RequestPolicy};
pub use watch_asset::{NftStandard, WatchAsset};
