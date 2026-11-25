//! A dapp's requests to the wallet, whatever carried them (an in-app browser's bridge,
//! WalletConnect): the extension's provider rules (`packages/foundation/wallet-protocol/src/
//! provider/`) as sans-IO mechanism. The wallet's numbers (budgets, the versions it advertises)
//! are passed in.
//!
//! So far: the hostile-JSON budget (`budget`), the wallet API's error codes (`error`), and
//! reading a request's method and params (`request`; an invoke's calls in `invoke`, a chain in
//! `add_chain`, an asset in `watch_asset`), and which accounts a site may see (`grants`). The
//! approval queue, the blocklist and review follow.

pub mod add_chain;
pub mod budget;
pub mod error;
pub mod grants;
pub mod invoke;
pub mod request;
pub mod watch_asset;

pub use add_chain::ChainRequest;
pub use budget::JsonBudget;
pub use error::WalletRpcError;
pub use grants::OriginGrant;
pub use invoke::InvokeLimits;
pub use request::{read_request, Method, Request, RequestPolicy};
pub use watch_asset::{NftStandard, WatchAsset};
