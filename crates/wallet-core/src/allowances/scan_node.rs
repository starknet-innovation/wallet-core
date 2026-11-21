// The fake node the scan and tracked-read tests share: it filters by key position and block
// range and pages oldest-first as `starknet_getEvents` does, as the extension's tests' fake
// `AllowanceRpc` does, and answers `allowance` from a table.

use std::cell::RefCell;

use serde_json::{json, Value};

use super::discovery::APPROVAL_EVENT_KEY;
use crate::js::felt::canonical_felt;
use crate::portfolio::wire::{RpcReply, RpcRequest};
use crate::send::u256::U256;

pub(super) const OWNER: &str = "0x1234567890abcdef";
pub(super) const TOKEN: &str = "0x049d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7";
pub(super) const UNLIMITED_LOW: &str = "0xffffffffffffffffffffffffffffffff";

pub(super) fn canonical(value: &str) -> String {
    canonical_felt(value).unwrap()
}

pub(super) fn drainer() -> String {
    format!("0x7{}", "d".repeat(62))
}

#[derive(Clone)]
pub(super) struct Event {
    pub(super) block: u64,
    pub(super) keys: Vec<String>,
    pub(super) data: Vec<String>,
}

pub(super) fn cairo1(owner: &str, spender: &str, block: u64) -> Event {
    Event {
        block,
        keys: vec![APPROVAL_EVENT_KEY.into(), owner.into(), spender.into()],
        data: vec!["0x64".into(), "0x0".into()],
    }
}

pub(super) fn legacy(owner: &str, spender: &str, block: u64) -> Event {
    Event {
        block,
        keys: vec![APPROVAL_EVENT_KEY.into()],
        data: vec![owner.into(), spender.into(), "0x64".into(), "0x0".into()],
    }
}

// `count` approvals by strangers, one per block from `first`.
pub(super) fn strangers(
    count: usize,
    first: u64,
    make: fn(&str, &str, u64) -> Event,
) -> Vec<Event> {
    (0..count)
        .map(|i| {
            let owner = format!("0x{:x}", 0x9000 + i);
            let spender = format!("0x{:x}", 0x5000 + i);
            make(&owner, &spender, first + i as u64)
        })
        .collect()
}

/// A replacement answer, written for id 0, for the calls it matches.
pub(super) type Fault = fn(&Value) -> Option<RpcReply>;

pub(super) struct Node {
    pub(super) head: u64,
    pub(super) log: Vec<Event>,
    /// Live allowance `[low, high]` by canonical spender; anything else reads as zero.
    pub(super) amounts: Vec<(String, [String; 2])>,
    /// Replaces the node's answer to the calls it matches.
    pub(super) fault: Option<Fault>,
    /// Every events filter it was asked, in order.
    pub(super) filters: RefCell<Vec<Value>>,
}

pub(super) fn node(head: u64, log: Vec<Event>) -> Node {
    Node {
        head,
        log,
        amounts: vec![(canonical(&drainer()), [UNLIMITED_LOW.into(), "0x0".into()])],
        fault: None,
        filters: RefCell::new(Vec::new()),
    }
}

pub(super) fn ok(id: u64, result: Value) -> RpcReply {
    let body = json!({ "jsonrpc": "2.0", "id": id, "result": result });
    RpcReply {
        id,
        status: 200,
        body: Some(body.to_string()),
    }
}

pub(super) fn semantic(id: u64) -> RpcReply {
    let body = json!({ "jsonrpc": "2.0", "id": id, "error": { "code": 24, "message": "x" } });
    RpcReply {
        id,
        status: 200,
        body: Some(body.to_string()),
    }
}

pub(super) fn same(a: &str, b: &str) -> bool {
    canonical_felt(a) == canonical_felt(b)
}

impl Node {
    pub(super) fn answer(&self, call: &RpcRequest) -> RpcReply {
        let body: Value = serde_json::from_str(&call.body).unwrap();
        // A fault's reply is written for id 0; it answers this call's id.
        if let Some(reply) = self.fault.and_then(|fault| fault(&body)) {
            let body = reply
                .body
                .map(|text| match serde_json::from_str::<Value>(&text) {
                    Ok(mut value) if value.get("id").is_some() => {
                        value["id"] = json!(call.id);
                        value.to_string()
                    }
                    _ => text,
                });
            return RpcReply {
                id: call.id,
                body,
                ..reply
            };
        }
        match body["method"].as_str().unwrap() {
            "starknet_blockNumber" => ok(call.id, json!(self.head)),
            "starknet_getEvents" => ok(call.id, self.events(&body["params"]["filter"])),
            _ => {
                let calldata = &body["params"]["request"]["calldata"];
                let spender = canonical(calldata[1].as_str().unwrap());
                let amount = self.amounts.iter().find(|(s, _)| *s == spender);
                let words = amount.map_or(json!(["0x0", "0x0"]), |(_, w)| json!(w));
                ok(call.id, words)
            }
        }
    }

    fn events(&self, filter: &Value) -> Value {
        self.filters.borrow_mut().push(filter.clone());
        let keys: Vec<Vec<String>> = serde_json::from_value(filter["keys"].clone()).unwrap();
        let from = filter["from_block"]["block_number"].as_u64().unwrap_or(0);
        let to = filter["to_block"]["block_number"]
            .as_u64()
            .unwrap_or(self.head);
        let matching: Vec<&Event> = self
            .log
            .iter()
            .filter(|e| e.block >= from && e.block <= to)
            .filter(|e| {
                keys.iter().enumerate().all(|(position, group)| {
                    group.is_empty()
                        || e.keys
                            .get(position)
                            .is_some_and(|k| group.iter().any(|g| same(g, k)))
                })
            })
            .collect();
        let start: usize = filter["continuation_token"]
            .as_str()
            .map_or(0, |t| t.parse().unwrap());
        let end = start + filter["chunk_size"].as_u64().unwrap() as usize;
        let page: Vec<Value> = matching
            .iter()
            .skip(start)
            .take(end - start)
            .map(|e| json!({ "keys": e.keys, "data": e.data, "block_number": e.block }))
            .collect();
        if end < matching.len() {
            json!({ "events": page, "continuation_token": end.to_string() })
        } else {
            json!({ "events": page })
        }
    }
}

/// `UNLIMITED_LOW` as an amount.
pub(super) fn unlimited_low() -> U256 {
    U256::from_u128(u128::MAX)
}
