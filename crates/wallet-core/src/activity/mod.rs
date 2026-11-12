//! The wallet's one activity log: transactions this wallet submitted and on-chain history rows,
//! as the extension's `ActivityEntry`, in one file the core reads and writes through the host's
//! `Storage`. Public on-chain data only; see `store` for the file format and its guarantees and
//! `feed` for the history side.

pub mod feed;
pub mod observation;
pub mod store;

pub use store::ActivityLog;
