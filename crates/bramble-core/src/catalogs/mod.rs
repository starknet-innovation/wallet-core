//! Bramble's catalogs, embedded from `generated/` (rendered from the extension by Bramble's
//! `bun run mobile:assets`, copied here unchanged) and parsed once on first use: the networks and
//! token registry (`networks`), the account classes, families and recovery bounds (`accounts`,
//! with the class rules in `account_address`), and the multisig and coordination constants
//! (`multisig`). `starknet_wallet_core` takes each as an argument. A bundled file that does not
//! parse or validate panics on first use, loudly, as mobile's loaders do. Plain Rust for now; the
//! UniFFI exports arrive with the swap (docs/PLAN.md, 2.7).

pub mod account_address;
pub mod accounts;
pub mod multisig;
pub mod networks;

pub use networks::catalogs;
