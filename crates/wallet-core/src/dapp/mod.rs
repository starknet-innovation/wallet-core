//! A dapp's requests to the wallet, whatever carried them (an in-app browser's bridge,
//! WalletConnect): the extension's provider rules (`packages/foundation/wallet-protocol/src/
//! provider/`) as sans-IO mechanism. The wallet's numbers (budgets, the versions it advertises)
//! are passed in.
//!
//! So far: the hostile-JSON budget (`budget`), the wallet API's error codes (`error`), and
//! reading a request's method and params (`request`, an invoke's calls in `invoke`). Grants, the approval queue, the blocklist
//! and review follow.

pub mod budget;
pub mod error;
pub mod invoke;
pub mod request;

pub use budget::JsonBudget;
pub use error::WalletRpcError;
pub use invoke::InvokeLimits;
pub use request::{read_request, Method, Request, RequestBudgets};
