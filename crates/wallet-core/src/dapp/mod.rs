//! A dapp's requests to the wallet, whatever carried them (an in-app browser's bridge,
//! WalletConnect): the extension's provider rules (`packages/foundation/wallet-protocol/src/
//! provider/`) as sans-IO mechanism. The wallet's numbers (budgets, the versions it advertises)
//! are passed in.
//!
//! So far: the hostile-JSON budget (`budget`), the wallet API's error codes (`error`), and
//! reading a request's method and params (`request`; an invoke's calls in `invoke`, a chain in
//! `add_chain`, an asset in `watch_asset`), which accounts a site may see (`grants`), the
//! approval queue (`queue`), connecting a site with the passive reads around it (`connect`), and
//! whether a site is blocked and what else the wallet warns about it (`blocklist`, over the live
//! feed in `phishing_feed` and the first-seen record in `origin_history`), and SNIP-12 typed data
//! (`typed_data`: the gate, the chain binding before Krusty hashes it, the card's summary), and
//! the invoke route's review of the calls a SNIP-9 outside execution in it authorizes
//! (`outside_execution`). Review follows.

pub mod add_chain;
pub mod blocklist;
pub mod budget;
pub mod chain;
pub mod connect;
pub mod error;
pub mod grants;
pub mod invoke;
pub mod origin_history;
pub mod outside_execution;
pub mod phishing_feed;
pub mod queue;
pub mod request;
pub mod typed_data;
mod typed_data_scalars;
mod typed_data_values;
pub mod watch_asset;

pub use add_chain::ChainRequest;
pub use blocklist::{SiteContext, SiteInsight, SiteLists, SiteWarning};
pub use budget::JsonBudget;
pub use connect::{Answer, DappAccount, Site, Unlocked, Versions};
pub use error::WalletRpcError;
pub use grants::OriginGrant;
pub use invoke::InvokeLimits;
pub use origin_history::OriginHistory;
pub use outside_execution::{review_outside_execution, OutsideExecutionReview, Stage};
pub use phishing_feed::{FeedHealth, FeedPolicy, PhishingFeed};
pub use queue::{ApprovalId, ApprovalQueue, QueueLimits};
pub use request::{read_request, Method, Request, RequestPolicy};
pub use typed_data::{describe_typed_data, CheckedTypedData, Revision, TypedDataRefusal};
pub use watch_asset::{NftStandard, WatchAsset};
