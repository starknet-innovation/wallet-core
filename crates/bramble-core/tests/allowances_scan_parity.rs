//! Allowance discovery and tracked reads (P12) against the extension's `collectExternalAllowances`
//! and `collectTrackedAllowances` over a fake node (`fixtures/extension-allowances.jsonl`, `bun
//! run mobile:parity`). Each case is the transcript of every call the extension made with the
//! node's answer: the core must ask the same calls in the same order, and, fed the same answers,
//! reach the same result, or the same transport failure for the host to fail over on.

mod allowances_support;

use allowances_support::{cases, name, same_felt, text};
use bramble_core::ALLOWANCE_POLICY;
use serde_json::{json, Value};
use starknet_wallet_core::allowances::scan::{ExternalAllowanceScan, ScanStep};
use starknet_wallet_core::allowances::tracked::{
    TrackedAllowancePair, TrackedAllowanceRead, ALLOWANCE_SELECTOR,
};
use starknet_wallet_core::portfolio::wire::{RpcReply, RpcRequest};

/// What both the scan and the tracked read offer a host.
trait Reads {
    fn next(&mut self) -> Vec<RpcRequest>;
    fn take(&mut self, reply: RpcReply) -> ScanStep;
}

impl Reads for ExternalAllowanceScan {
    fn next(&mut self) -> Vec<RpcRequest> {
        self.next_calls()
    }
    fn take(&mut self, reply: RpcReply) -> ScanStep {
        self.accept(reply)
    }
}

impl Reads for TrackedAllowanceRead {
    fn next(&mut self) -> Vec<RpcRequest> {
        self.next_calls()
    }
    fn take(&mut self, reply: RpcReply) -> ScanStep {
        self.accept(reply)
    }
}

// The extension's call, as its `AllowanceRpc` saw it, against the JSON-RPC body the core sent.
fn assert_same_call(body: &Value, call: &Value, case: &str) {
    let params = &call["params"];
    match text(&call["method"]) {
        "getLatestBlockNumber" => assert_eq!(body["method"], "starknet_blockNumber", "{case}"),
        "getEvents" => {
            assert_eq!(body["method"], "starknet_getEvents", "{case}");
            let filter = &body["params"]["filter"];
            assert!(same_felt(&filter["address"], &params["address"]), "{case}");
            assert_eq!(filter["chunk_size"], params["chunk_size"], "{case}");
            let (sent, asked) = (
                filter["keys"].as_array().unwrap(),
                params["keys"].as_array().unwrap(),
            );
            assert_eq!(sent.len(), asked.len(), "{case}");
            for (sent, asked) in sent.iter().zip(asked) {
                let (sent, asked) = (sent.as_array().unwrap(), asked.as_array().unwrap());
                assert_eq!(sent.len(), asked.len(), "{case}");
                assert!(
                    sent.iter().zip(asked).all(|(s, a)| same_felt(s, a)),
                    "{case}"
                );
            }
            for bound in ["from_block", "to_block"] {
                assert_eq!(
                    filter[bound]["block_number"], params[bound],
                    "{case}: {bound}"
                );
            }
            assert_eq!(
                filter["continuation_token"], params["continuation_token"],
                "{case}"
            );
        }
        _ => {
            assert_eq!(body["method"], "starknet_call", "{case}");
            let request = &body["params"]["request"];
            assert_eq!(params["entrypoint"], "allowance", "{case}");
            assert_eq!(
                request["entry_point_selector"], ALLOWANCE_SELECTOR,
                "{case}"
            );
            assert!(
                same_felt(&request["contract_address"], &params["contractAddress"]),
                "{case}"
            );
            let (sent, asked) = (
                request["calldata"].as_array().unwrap(),
                params["calldata"].as_array().unwrap(),
            );
            assert_eq!(sent.len(), asked.len(), "{case}");
            assert!(
                sent.iter().zip(asked).all(|(s, a)| same_felt(s, a)),
                "{case}"
            );
        }
    }
}

// Replays the transcript; true when it ended on a transport failure.
fn replay(reads: &mut dyn Reads, case: &Value) -> bool {
    let label = name(case);
    for call in case["calls"].as_array().unwrap() {
        let sent = reads.next();
        assert_eq!(
            sent.len(),
            1,
            "{label}: one call at a time, as the extension awaits"
        );
        let body: Value = serde_json::from_str(&sent[0].body).unwrap();
        assert_same_call(&body, call, label);
        let id = sent[0].id;
        let answer = &call["reply"];
        let body = if answer.get("transport").is_some() {
            None
        } else if let Some(error) = answer.get("error") {
            Some(json!({ "jsonrpc": "2.0", "id": id, "error": error }).to_string())
        } else {
            Some(json!({ "jsonrpc": "2.0", "id": id, "result": answer["result"] }).to_string())
        };
        let step = reads.take(RpcReply {
            id,
            status: 200,
            body,
        });
        if step == ScanStep::Transport {
            assert!(
                answer.get("transport").is_some(),
                "{label}: a transport failure the extension did not see"
            );
            return true;
        }
    }
    assert!(
        reads.next().is_empty(),
        "{label}: the core asks more than the extension did"
    );
    false
}

#[test]
fn external_scans_ask_and_find_what_the_extension_does() {
    for case in cases("scan") {
        let tokens: Vec<String> = case["tokens"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| text(t).into())
            .collect();
        let mut scan = ExternalAllowanceScan::new(text(&case["owner"]), &tokens, ALLOWANCE_POLICY);
        if replay(&mut scan, &case) {
            assert_eq!(
                case["result"],
                json!({ "transport": true }),
                "{}",
                name(&case)
            );
            continue;
        }
        let result = scan.finish().unwrap();
        let allowances: Vec<Value> = result
            .allowances
            .iter()
            .map(|a| json!({ "amount": a.amount.to_decimal(), "spender": a.spender, "token": a.token }))
            .collect();
        let got = json!({ "allowances": allowances, "complete": result.complete });
        assert_eq!(got, case["result"], "{}", name(&case));
    }
}

#[test]
fn tracked_reads_ask_and_read_what_the_extension_does() {
    for case in cases("tracked") {
        let pairs: Vec<TrackedAllowancePair> = case["pairs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| TrackedAllowancePair {
                id: text(&p["id"]).into(),
                owner: text(&p["owner"]).into(),
                token: text(&p["token"]).into(),
                spender: text(&p["spender"]).into(),
            })
            .collect();
        let mut read = TrackedAllowanceRead::new(&pairs, ALLOWANCE_POLICY);
        if replay(&mut read, &case) {
            assert_eq!(
                case["result"],
                json!({ "transport": true }),
                "{}",
                name(&case)
            );
            continue;
        }
        let readings: Vec<Value> = read
            .finish()
            .unwrap()
            .iter()
            .map(|r| json!({ "amount": r.amount.to_decimal(), "id": r.id }))
            .collect();
        assert_eq!(Value::Array(readings), case["result"], "{}", name(&case));
    }
}
