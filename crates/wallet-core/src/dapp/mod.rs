//! A dapp's requests to the wallet, whatever carried them (an in-app browser's bridge,
//! WalletConnect): the extension's provider rules (`packages/foundation/wallet-protocol/src/
//! provider/`) as sans-IO mechanism. The wallet's numbers (budgets, the versions it advertises)
//! are passed in.
//!
//! So far: the hostile-JSON budget (`budget`), the wallet API's error codes (`error`), and
//! reading a request's method and params (`request`; an invoke's calls in `invoke`, a chain in
//! `add_chain`, an asset in `watch_asset`), which accounts a site may see (`grants`), the
//! approval queue (`queue`), and connecting a site with the passive reads around it (`connect`).
//! The blocklist and review follow.

pub mod add_chain;
pub mod budget;
pub mod connect;
pub mod error;
pub mod grants;
pub mod invoke;
pub mod queue;
pub mod request;
pub mod watch_asset;

pub use add_chain::ChainRequest;
pub use budget::JsonBudget;
pub use connect::{Answer, DappAccount, Site, Unlocked, Versions};
pub use error::WalletRpcError;
pub use grants::OriginGrant;
pub use invoke::InvokeLimits;
pub use queue::{ApprovalId, ApprovalQueue, QueueLimits};
pub use request::{read_request, Method, Request, RequestPolicy};
pub use watch_asset::{NftStandard, WatchAsset};
