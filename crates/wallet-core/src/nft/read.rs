//! The extension's entrypoint fallbacks (`rpc-assets.ts`), sans-IO: try one entrypoint, and on a
//! node's refusal or an answer of the wrong shape try the next spelling; a transport-class
//! failure abandons the endpoint (`withRpcFallback`), and the host restarts the read on the next
//! one with `start`.

use serde_json::json;
use starknet_types_core::felt::Felt;

use crate::js::json::JsValue;
use crate::portfolio::wire::{classify, rpc_call, Reply, RpcReply, RpcRequest};

/// Where a read stands after a reply.
#[derive(Debug, Clone, PartialEq)]
pub enum ReadStep<T> {
    /// Send this call next, on the same endpoint.
    Request(RpcRequest),
    /// The read is answered.
    Done(T),
    /// This endpoint failed: restart the read on the next one (`start`).
    Transport,
    /// No entrypoint answered with a usable value: the read failed.
    Failed,
}

/// What one reply says about the rung it answered.
pub(super) enum Rung {
    /// The call's result array.
    Answer(Vec<JsValue>),
    /// The node refused this entrypoint, or answered with something that is not an array.
    Skip,
    Transport,
}

/// A contract's entrypoints, tried in order, each with its own calldata.
#[derive(Debug, Clone)]
pub(super) struct Ladder {
    contract: Felt,
    rungs: Vec<(&'static str, Vec<Felt>)>,
    at: usize,
    id: u64,
}

impl Ladder {
    pub(super) fn new(contract: Felt, rungs: Vec<(&'static str, Vec<Felt>)>) -> Self {
        Self {
            contract,
            rungs,
            at: 0,
            id: 0,
        }
    }

    /// The index of the entrypoint the last request called.
    pub(super) fn at(&self) -> usize {
        self.at
    }

    /// The first entrypoint's call, under `id`.
    pub(super) fn start(&mut self, id: u64) -> RpcRequest {
        self.at = 0;
        self.request(id)
    }

    fn request(&mut self, id: u64) -> RpcRequest {
        self.id = id;
        // `at` stays below the rung count (`start`, `next`); the fallback is never reached.
        let (entrypoint, calldata) = self
            .rungs
            .get(self.at)
            .map_or(("", &[][..]), |(name, data)| (*name, data.as_slice()));
        let calldata: Vec<String> = calldata.iter().map(crate::felt::canonical).collect();
        rpc_call(
            id,
            "starknet_call",
            json!({
                "block_id": "latest",
                "request": {
                    "calldata": calldata,
                    "contract_address": crate::felt::canonical(&self.contract),
                    "entry_point_selector": crate::felt::canonical(&crate::kms::selector(entrypoint)),
                },
            }),
        )
    }

    /// Classifies the reply to the last request. A `null` result is transport-class: indexing
    /// it throws a `TypeError` in the extension, which `isTransportRpcError` fails over on.
    pub(super) fn rung(&self, reply: &RpcReply) -> Rung {
        match classify(reply, self.id) {
            Reply::Transport | Reply::Result(JsValue::Null) => Rung::Transport,
            Reply::Semantic => Rung::Skip,
            Reply::Result(JsValue::Array(items)) => Rung::Answer(items),
            Reply::Result(_) => Rung::Skip,
        }
    }

    /// Moves to the next entrypoint, or fails when none is left.
    pub(super) fn next<T>(&mut self, next_id: u64) -> ReadStep<T> {
        self.at += 1;
        if self.at < self.rungs.len() {
            ReadStep::Request(self.request(next_id))
        } else {
            ReadStep::Failed
        }
    }
}
