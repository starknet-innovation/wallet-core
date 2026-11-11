//! Activity: the Starkscan lane, the RPC history scan, the local log's cadence, and the row
//! presentation, each a sans-IO port of the extension's TypeScript. The log itself is
//! `crate::activity`; catalogs (`crate::catalog`) are arguments.

pub mod activity;
pub mod activity_book;
pub mod activity_rules;
pub mod appearance;
pub mod classify;
pub mod events;
pub mod explorer;
pub mod starkscan;
pub mod time;
