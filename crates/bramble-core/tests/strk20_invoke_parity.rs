//! The proof-bearing STRK20 INVOKE (SNIP-36) against `extension-strk20-invoke.jsonl` (see
//! `fixtures/README.md`), every line: each `hash` recomputed from its inputs and its signature
//! verified under the account's key; each `rpc` transcript driven through `send::plan::Plan` (a
//! batch made proof-bearing by `Invoke::with_proof`), every body it sends compared with the
//! extension's and the transaction hash with the one it got back; each `refusal` through
//! `send::proof_invoke::check_shape`; each `maturity` case through `ProvingWait`.
//!
//! The plan's review proves the chain first, where the extension's `estimateInvokeFee` does not,
//! so its review requests are matched by method; the submit is held to the recorded order. After
//! the write the plan confirms through its own status checkpoints (the receipt only when the
//! wallet wants the fee), where the extension's `recordSubmission` reads status and receipt for
//! the maturity store: both bodies are compared through `rpc_bodies`.

use serde_json::{json, Value};
use starknet_wallet_core::send::calls::{execute_calldata, Call};
use starknet_wallet_core::send::fees::{Bound, Bounds};
use starknet_wallet_core::send::judge;
use starknet_wallet_core::send::plan::{Account, Authorization, Invoke, Kind, Plan};
use starknet_wallet_core::send::proof_invoke::{
    check_shape, Maturity, PriorWrite, Proof, ProofSubmission, ProvingWait, WaitRead, WaitStep,
};
use starknet_wallet_core::send::review_flags::ReviewContext;
use starknet_wallet_core::send::rpc_bodies;
use starknet_wallet_core::send::sign::{self, Deployment};
use starknet_wallet_core::send::types::{SendFailureCode as Code, SendStep, StepReply};
use starknet_wallet_core::{felt, stark_signature_is_valid, Felt, NetworkId};

use crate::plan_support::{f, Host, NOW, OZ_CLASS};

/// The extension's maturity numbers (`official-sdk-maturity.ts`): `PROVING_BLOCK_DEPTH`,
/// `MAX_MATURITY_POLLS` with a one-second pause, and `waitForTransaction`'s 240 status reads 250
/// ms apart for a prior write.
const MATURITY: Maturity = Maturity {
    depth: 10,
    max_polls: 120,
    poll_pause_ms: 1_000,
    status_reads: 240,
    status_pause_ms: 250,
};

fn lines() -> Vec<Value> {
    let path = format!(
        "{}/tests/fixtures/extension-strk20-invoke.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn of_kind(kind: &str) -> Vec<Value> {
    lines().into_iter().filter(|l| l["kind"] == kind).collect()
}

fn felts(value: &Value) -> Vec<Felt> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|v| f(v.as_str().unwrap()))
        .collect()
}

fn calls(line: &Value) -> Vec<Call> {
    line["calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|call| Call {
            contract_address: f(call["contractAddress"].as_str().unwrap()),
            entrypoint: call["entrypoint"].as_str().unwrap().into(),
            calldata: felts(&call["calldata"]),
        })
        .collect()
}

fn proof(line: &Value) -> Option<Proof> {
    let proof = &line["proof"];
    (!proof.is_null()).then(|| Proof {
        data: proof["data"].as_str().unwrap().into(),
        output: felts(&proof["output"]),
        proof_facts: felts(&proof["proof_facts"]),
    })
}

fn network(line: &Value) -> NetworkId {
    NetworkId::from_catalog_id(line["networkId"].as_str().unwrap()).unwrap()
}

fn bounds(value: &Value) -> Bounds {
    let hex = |v: &Value| u128::from_str_radix(v.as_str().unwrap().trim_start_matches("0x"), 16);
    let bound = |name: &str| Bound {
        max_amount: hex(&value[name]["max_amount"]).unwrap(),
        max_price_per_unit: hex(&value[name]["max_price_per_unit"]).unwrap(),
    };
    Bounds {
        l1_gas: bound("l1_gas"),
        l2_gas: bound("l2_gas"),
        l1_data_gas: bound("l1_data_gas"),
    }
}

#[test]
fn every_hash_is_the_extensions_and_its_signature_verifies() {
    let hashes = of_kind("hash");
    assert_eq!(hashes.len(), 7);
    let mut facts_seen = Vec::new();
    for line in &hashes {
        let id = line["id"].as_str().unwrap();
        let inputs = &line["inputs"];
        let network = network(line);
        // What `sign::invoke_hash_with_proof` fixes rather than takes.
        assert_eq!(inputs["version"], "0x3", "{id}");
        assert_eq!(inputs["tip"], "0x0", "{id}");
        assert_eq!(inputs["paymasterData"], json!([]), "{id}");
        assert_eq!(inputs["accountDeploymentData"], json!([]), "{id}");
        assert_eq!(inputs["nonceDataAvailabilityMode"], "L1", "{id}");
        assert_eq!(inputs["feeDataAvailabilityMode"], "L1", "{id}");
        assert_eq!(f(inputs["chainId"].as_str().unwrap()), network.chain_id());
        let calldata = execute_calldata(&calls(line));
        assert_eq!(calldata, felts(&inputs["compiledCalldata"]), "{id}");
        let facts = match (&inputs["proofFacts"], proof(line)) {
            (Value::Null, None) => Vec::new(),
            (facts, Some(proof)) => {
                assert_eq!(felts(facts), proof.proof_facts, "{id}");
                proof.proof_facts
            }
            _ => panic!("{id}: facts without a proof"),
        };
        let sender = f(inputs["senderAddress"].as_str().unwrap());
        let nonce = f(inputs["nonce"].as_str().unwrap());
        let bounds = bounds(&inputs["resourceBounds"]);
        let hash =
            sign::invoke_hash_with_proof(network, &sender, &calldata, &nonce, &bounds, &facts)
                .unwrap();
        assert_eq!(hash, f(line["hash"].as_str().unwrap()), "{id}");
        if facts.is_empty() {
            // No facts is today's hash exactly.
            let plain = sign::invoke_hash(network, &sender, &calldata, &nonce, &bounds).unwrap();
            assert_eq!(plain, hash, "{id}");
        }
        let signature = &line["signature"];
        assert_eq!(f(signature["hash"].as_str().unwrap()), hash, "{id}");
        assert!(
            stark_signature_is_valid(
                &f(line["accountPublicKey"].as_str().unwrap()),
                &hash,
                &f(signature["r"].as_str().unwrap()),
                &f(signature["s"].as_str().unwrap()),
            ),
            "{id}"
        );
        facts_seen.push(facts.len());
    }
    facts_seen.sort_unstable();
    facts_seen.dedup();
    assert_eq!(facts_seen, [0, 1, 9]);
}

/// The published phrase's OpenZeppelin account 0 on the line's network.
fn account(host: &Host, network: NetworkId) -> Account {
    let public_key = host.public_key();
    let deployment = Deployment {
        class_hash: f(OZ_CLASS),
        salt: Felt::ZERO,
        constructor_calldata: vec![public_key],
    };
    Account {
        network,
        address: deployment.address().unwrap(),
        public_key,
        authorization: Authorization::StarkPair,
        deployment: Some(deployment),
    }
}

fn reply(id: u64, recorded: &Value) -> StepReply {
    let mut body = recorded.clone();
    body["id"] = json!(id);
    StepReply::Rpc {
        id,
        body: body.to_string(),
        now_ms: NOW,
    }
}

/// The body the plan sent equals the recorded one but for its id; starknet.js sends a
/// no-argument method without params where the core sends `[]`.
fn assert_body(sent: &str, recorded: &Value, name: &str) {
    let sent: Value = serde_json::from_str(sent).unwrap();
    assert_eq!(sent["method"], recorded["method"], "{name}");
    let params = match &recorded["params"] {
        Value::Null if sent["params"] == json!([]) => json!([]),
        params => params.clone(),
    };
    assert_eq!(sent["params"], params, "{name}: {}", recorded["method"]);
}

fn rpc(step: &SendStep) -> (u64, &str) {
    match step {
        SendStep::Rpc { id, body, .. } => (*id, body.as_str()),
        other => panic!("expected a request, got {other:?}"),
    }
}

#[test]
fn every_transcript_is_the_plans_wire() {
    let transcripts = of_kind("rpc");
    assert_eq!(transcripts.len(), 7);
    for line in &transcripts {
        let name = line["id"].as_str().unwrap();
        let host = Host::new();
        let account = account(&host, network(line));
        let calls = calls(line);
        let pool = calls.last().unwrap().contract_address;
        let mut invoke = Invoke::reviewed(
            &account,
            calls,
            &bramble_core::SEND_POLICY,
            &ReviewContext::default(),
        )
        .unwrap();
        if let Some(proof) = proof(line) {
            let submission = ProofSubmission {
                proof,
                prior: None,
                maturity: MATURITY,
            };
            invoke = invoke.with_proof(&pool, submission).unwrap();
        }
        let kind = Kind::Invoke(Box::new(invoke));
        let mut plan = Plan::new(account, kind, bramble_core::SEND_POLICY);

        // Review: the same requests, each answered as the extension's was.
        let review = line["review"].as_array().unwrap();
        let mut step = plan.advance(&host, StepReply::Start).unwrap();
        let mut answered = 0;
        while let SendStep::Rpc { id, body, .. } = &step {
            let method = serde_json::from_str::<Value>(body).unwrap()["method"].clone();
            let recorded = review
                .iter()
                .find(|e| e["body"]["method"] == method)
                .unwrap_or_else(|| panic!("{name}: review sent {method}"));
            assert_body(body, &recorded["body"], name);
            answered += 1;
            step = plan.advance(&host, reply(*id, &recorded["reply"])).unwrap();
        }
        assert_eq!(answered, review.len(), "{name}");
        assert_eq!(step, SendStep::Review, "{name}");
        let fee = plan.current_review().unwrap().fee.clone().unwrap();
        assert_eq!(fee.max_spend, line["reviewedFee"]["overallFee"], "{name}");

        // Submit: the recorded order, through the write.
        let submit = line["submit"].as_array().unwrap();
        let mut step = plan.advance(&host, StepReply::Confirm).unwrap();
        let mut exchanges = submit.iter();
        for exchange in exchanges.by_ref() {
            let (id, body) = rpc(&step);
            assert_body(body, &exchange["body"], name);
            step = plan.advance(&host, reply(id, &exchange["reply"])).unwrap();
            if exchange["body"]["method"] == "starknet_addInvokeTransaction" {
                break;
            }
        }
        let hash = f(line["transactionHash"].as_str().unwrap());
        assert_eq!(
            step,
            SendStep::Broadcast {
                hash: felt::canonical(&hash)
            },
            "{name}"
        );
        assert_eq!(*host.signed.borrow(), [hash], "{name}");
        assert_eq!(plan.prior_accepted_block(), None, "{name}");

        // After the write: the plan's status checkpoint, and the receipt the extension reads.
        let rest: Vec<&Value> = exchanges.collect();
        if rest.is_empty() {
            assert!(proof(line).is_none(), "{name}");
            continue;
        }
        let mut step = plan.advance(&host, StepReply::Proceed).unwrap();
        assert!(matches!(step, SendStep::Wait { .. }), "{name}");
        step = plan.advance(&host, StepReply::WaitElapsed).unwrap();
        let (id, body) = rpc(&step);
        assert_body(body, &rest[0]["body"], name);
        let step = plan.advance(&host, reply(id, &rest[0]["reply"])).unwrap();
        assert!(matches!(step, SendStep::Finished { .. }), "{name}");
        assert_body(
            &rpc_bodies::transaction_receipt(1, &hash),
            &rest[1]["body"],
            name,
        );
        assert_eq!(rest.len(), 2, "{name}");
    }
}

#[test]
fn every_refusal_is_the_shape_rule() {
    let refusals = of_kind("refusal");
    assert_eq!(refusals.len(), 8);
    for line in &refusals {
        let name = line["id"].as_str().unwrap();
        assert_eq!(
            line["outcome"]["error"]["message"],
            "strk20 private submission unavailable"
        );
        assert_eq!(line["requests"], json!([]), "{name}");
        let pool = f(line["pool"].as_str().unwrap());
        let refusal = check_shape(&pool, &calls(line), &proof(line).unwrap()).unwrap_err();
        assert_eq!(refusal.to_string(), "strk20 private submission unavailable");
        // Nor can a reviewed batch carry it, so no plan exists to send anything.
        let host = Host::new();
        let account = account(&host, NetworkId::Mainnet);
        let context = ReviewContext::default();
        let policy = bramble_core::SEND_POLICY;
        if let Ok(invoke) = Invoke::reviewed(&account, calls(line), &policy, &context) {
            let submission = ProofSubmission {
                proof: proof(line).unwrap(),
                prior: None,
                maturity: MATURITY,
            };
            assert!(invoke.with_proof(&pool, submission).is_err(), "{name}");
        }
    }
}

fn prior(value: &Value) -> Option<PriorWrite> {
    if value.is_null() {
        return None;
    }
    Some(match value.get("acceptedBlock") {
        Some(block) => PriorWrite::Accepted(block.as_u64().unwrap()),
        None => {
            assert_eq!(value["state"], "submitted");
            PriorWrite::Submitted(f(value["transactionHash"].as_str().unwrap()))
        }
    })
}

#[test]
fn every_maturity_case_is_the_proving_block_wait() {
    let cases = of_kind("maturity");
    assert_eq!(cases.len(), 5);
    let mut capped = false;
    for line in &cases {
        let name = line["id"].as_str().unwrap();
        assert_eq!(line["provingBlockDepth"], MATURITY.depth, "{name}");
        let pauses: Vec<u64> = line["pausesAfterRequest"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_u64().unwrap())
            .collect();
        let mut wait = ProvingWait::new(MATURITY, prior(&line["prior"]));
        let mut step = wait.start();
        for (index, exchange) in line["requests"].as_array().unwrap().iter().enumerate() {
            let WaitStep::Read(read) = step else {
                panic!("{name}: request {index} not asked for: {step:?}");
            };
            let id = exchange["body"]["id"].as_u64().unwrap();
            let sent: Value = serde_json::from_str(&read.body(id)).unwrap();
            assert_eq!(sent, exchange["body"], "{name}");
            step = wait.on_reply(judge::answer(&exchange["reply"].to_string(), id));
            if pauses.contains(&(index as u64 + 1)) {
                let expected = match read {
                    WaitRead::Status(_) => MATURITY.status_pause_ms,
                    _ => MATURITY.poll_pause_ms,
                };
                assert_eq!(step, WaitStep::Pause(expected), "{name}: after {index}");
                step = wait.after_pause();
            }
        }
        match line["outcome"].get("provingBlock") {
            Some(block) => assert_eq!(step, WaitStep::Ready(block.as_u64().unwrap()), "{name}"),
            None => {
                assert_eq!(
                    line["outcome"]["error"]["message"],
                    "strk20 services unavailable"
                );
                assert_eq!(step, WaitStep::Failed, "{name}");
                // 121 reads and 120 pauses.
                assert_eq!(pauses.len() as u32, MATURITY.max_polls, "{name}");
                capped = true;
            }
        }
        let stored = line["stored"]["acceptedBlock"].as_u64();
        assert_eq!(wait.prior_accepted_block(), stored, "{name}");
    }
    assert!(capped);
}

/// A prior write that reverted stops the plan at `Confirm`'s wait: nothing is estimated again or
/// signed ("strk20 services unavailable" is the core's `PreparationUnavailable`).
#[test]
fn a_failed_wait_stops_the_plan_before_signing() {
    let line = &of_kind("rpc")[1];
    assert!(proof(line).is_some());
    let host = Host::new();
    let account = account(&host, network(line));
    let calls = calls(line);
    let pool = calls.last().unwrap().contract_address;
    let prior = Felt::from(0x5afe_u32);
    let submission = ProofSubmission {
        proof: proof(line).unwrap(),
        prior: Some(PriorWrite::Submitted(prior)),
        maturity: MATURITY,
    };
    let context = ReviewContext::default();
    let invoke = Invoke::reviewed(&account, calls, &bramble_core::SEND_POLICY, &context)
        .unwrap()
        .with_proof(&pool, submission)
        .unwrap();
    let mut plan = Plan::new(
        account,
        Kind::Invoke(Box::new(invoke)),
        bramble_core::SEND_POLICY,
    );
    let review = line["review"].as_array().unwrap();
    let mut step = plan.advance(&host, StepReply::Start).unwrap();
    while let SendStep::Rpc { id, body, .. } = &step {
        let method = serde_json::from_str::<Value>(body).unwrap()["method"].clone();
        let recorded = review
            .iter()
            .find(|e| e["body"]["method"] == method)
            .unwrap();
        step = plan.advance(&host, reply(*id, &recorded["reply"])).unwrap();
    }
    let step = plan.advance(&host, StepReply::Confirm).unwrap();
    let (id, body) = rpc(&step);
    assert_eq!(body, rpc_bodies::transaction_status(id, &prior));
    let reverted = json!({"jsonrpc": "2.0", "result": {"execution_status": "REVERTED", "finality_status": "ACCEPTED_ON_L2"}});
    let step = plan.advance(&host, reply(id, &reverted)).unwrap();
    let SendStep::Failed { failure } = step else {
        panic!("{step:?}");
    };
    assert_eq!(failure.code, Code::PreparationUnavailable);
    assert_eq!(failure.hash, None);
    assert_eq!(plan.signatures_made(), 0);
}
