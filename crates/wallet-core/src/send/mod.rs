//! Sending, sans-IO: the mechanism under an ERC-20 transfer and an account's own deployment.
//! The core builds every JSON-RPC body (`rpc_bodies`) and judges every reply (`judge`); the host
//! moves bytes. Hashing goes through Krusty (`sign`); signing itself is `kms::sign_stark_hash`.
//!
//! Product policy is an argument: the fee margin starknet.js adds to an estimate and the
//! headroom a submit-time fee may take over the reviewed one are the wallet's numbers (Bramble's
//! live in `bramble-core`). `plan::Plan` sequences the steps; the wallet lends it a
//! `plan::PlanHost` for its session, keys and activity log.
//!
//! Parity with the extension is pinned by `extension-transactions-*.json`, in `bramble-core`'s
//! `tests/transactions_{rules,wire,plan}_parity.rs`.

pub mod calls;
pub mod fees;
pub mod judge;
pub mod plan;
mod plan_deploy;
pub mod plan_probe;
mod plan_review;
mod plan_submit;
pub mod rpc_bodies;
pub mod sign;
pub mod types;
pub mod u256;
