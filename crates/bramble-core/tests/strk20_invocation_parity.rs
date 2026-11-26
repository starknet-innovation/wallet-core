//! STRK20 invocation and prover parity (S4 of issue #182): every job in
//! `extension-strk20-jobs.jsonl` that reached the signer, replayed through `privacy::invocation`
//! and `privacy::prover`. From the recorded `compile_actions` calldata, signature, pool nonce,
//! pool and proving block, the invocation and every prove body must be the SDK's bytes; each
//! recorded prover reply must judge as the SDK took it, and the accepted one must give the SDK's
//! `apply_actions` call and proof.

use serde_json::Value;
use starknet_wallet_core::privacy::invocation::{pool_nonce, pool_nonce_request, ProofInvocation};
use starknet_wallet_core::privacy::proof::ProofLimits;
use starknet_wallet_core::privacy::prover::{
    judge_prove_reply, prove_body, ProveOutcome, ProverRetry,
};
use starknet_wallet_core::{felt, Felt, SecretFelt};

/// The generator's pinned clock, the SDK's `Date.now()` JSON-RPC id.
const PROVE_ID: u64 = 1_800_000_000_000;

/// Bramble's `STRK20_PROVER_REQUEST` with the SDK's backoff defaults.
const BRAMBLE_RETRY: ProverRetry = ProverRetry {
    max_retries: 3,
    request_timeout_ms: 30_000,
    base_delay_ms: 1_000,
    max_delay_ms: 30_000,
};

fn jobs() -> Vec<(&'static str, Value)> {
    include_str!("fixtures/extension-strk20-jobs.jsonl")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| (line, serde_json::from_str(line).unwrap()))
        .collect()
}

fn hex(value: &Value) -> Felt {
    felt::parse(value.as_str().unwrap()).unwrap()
}

fn hexes(value: &Value) -> Vec<Felt> {
    value.as_array().unwrap().iter().map(hex).collect()
}

fn requests<'a>(job: &'a Value, route: &str) -> Vec<&'a Value> {
    job["requests"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|request| request["route"] == route)
        .collect()
}

#[test]
fn invocations_prove_bodies_and_replies_are_the_sdks() {
    let mut replayed = 0;
    for (line, job) in jobs() {
        let name = job["id"].as_str().unwrap();
        let Some(signing) = job["signing"].as_array().and_then(|s| s.first()) else {
            continue;
        };
        let pool = hex(&job["inputs"]["pool"]);
        let block = job["inputs"]["provingBlock"].as_u64().unwrap();

        // The nonce: starknet.js writes `contract_address` before `block_id`; `rpc_bodies` writes
        // the same members in sorted key order. Equal as JSON (the node reads JSON; the body is
        // public), not as bytes.
        let [nonce_request] = requests(&job, "nonce")[..] else {
            panic!("{name}: one nonce read");
        };
        let ours: Value = serde_json::from_str(&pool_nonce_request(1, &pool)).unwrap();
        let theirs: Value = serde_json::from_str(nonce_request["body"].as_str().unwrap()).unwrap();
        assert_eq!(ours, theirs, "{name}: nonce body");
        let nonce = pool_nonce(nonce_request["reply"]["body"].as_str().unwrap(), 1)
            .unwrap_or_else(|| panic!("{name}: nonce reply"));

        // The signer received the `compile_actions` calldata in decimal.
        let call = &signing["calls"][0];
        assert_eq!(call["entrypoint"], "compile_actions", "{name}");
        assert_eq!(hex(&call["contractAddress"]), pool, "{name}");
        let compile_actions: Vec<SecretFelt> = call["calldata"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| SecretFelt::new(Felt::from_dec_str(value.as_str().unwrap()).unwrap()))
            .collect();
        let invocation =
            ProofInvocation::new(pool, &compile_actions, nonce, hexes(&signing["signature"]));

        // The line is `JSON.stringify` output, so the recorded invocation's bytes are in it.
        let json = String::from_utf8(invocation.to_json().to_vec()).unwrap();
        assert!(
            line.contains(&format!("\"invocation\":{json},")),
            "{name}: invocation JSON"
        );

        let provers = requests(&job, "prover");
        for prover in &provers {
            let body = prove_body(PROVE_ID, block, &invocation);
            assert_eq!(
                std::str::from_utf8(&body).unwrap(),
                prover["body"].as_str().unwrap(),
                "{name}: prove body"
            );
        }
        let outcomes: Vec<ProveOutcome> = provers
            .iter()
            .map(|prover| {
                let reply = &prover["reply"];
                judge_prove_reply(
                    u16::try_from(reply["status"].as_u64().unwrap()).unwrap(),
                    reply["body"].as_str().unwrap().as_bytes(),
                    PROVE_ID,
                    &pool,
                    &ProofLimits::CEILING,
                )
            })
            .collect();
        replayed += 1;

        let Some(expected) = job["outcome"].get("callAndProof") else {
            assert!(provers.is_empty(), "{name}: refused before proving");
            continue;
        };
        let (last, retried) = outcomes.split_last().unwrap();
        for (attempt, outcome) in retried.iter().enumerate() {
            assert_eq!(*outcome, ProveOutcome::Busy, "{name}: attempt {attempt}");
            let attempt = u32::try_from(attempt).unwrap();
            assert_eq!(
                BRAMBLE_RETRY.delay_ms(attempt),
                Some(1_000 << attempt),
                "{name}: backoff"
            );
        }
        let ProveOutcome::Proved(proved) = last else {
            panic!("{name}: {last:?}");
        };
        let call = &expected["call"];
        assert_eq!(
            proved.call.contract_address,
            hex(&call["contract_address"]),
            "{name}"
        );
        assert_eq!(
            proved.call.entrypoint,
            call["entry_point"].as_str().unwrap(),
            "{name}"
        );
        assert_eq!(proved.call.calldata, hexes(&call["calldata"]), "{name}");
        let proof = &expected["proof"];
        assert_eq!(proved.proof.data, proof["data"].as_str().unwrap(), "{name}");
        assert_eq!(proved.proof.output, hexes(&proof["output"]), "{name}");
        assert_eq!(
            proved.proof.proof_facts,
            hexes(&proof["proof_facts"]),
            "{name}"
        );
        // Felts here, but the SDK's spellings are canonical, so nothing was respelled.
        let spelled: Vec<String> = proved.call.calldata.iter().map(felt::canonical).collect();
        assert_eq!(serde_json::json!(spelled), call["calldata"], "{name}");
    }
    assert_eq!(replayed, 12, "every job that reached the signer");
}

#[test]
fn the_busy_job_retries_twice_and_the_screened_job_carries_its_attestation() {
    let jobs = jobs();
    let job = |id: &str| &jobs.iter().find(|(_, job)| job["id"] == id).unwrap().1;
    let busy = requests(job("prover-busy-retry"), "prover");
    assert_eq!(busy.len(), 3);
    assert_eq!(busy[0]["reply"]["status"], 200);
    assert!(busy[0]["reply"]["body"]
        .as_str()
        .unwrap()
        .contains("-32005"));
    assert_eq!(busy[1]["reply"]["status"], 503);
    let screened = &job("screening-attestation")["outcome"]["callAndProof"]["call"]["calldata"];
    let tail: Vec<&str> = screened.as_array().unwrap()[screened.as_array().unwrap().len() - 4..]
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect();
    // `Some(ScreeningAttestation { issued_at: 1800000000, signature: (0x51, 0x52) })`.
    assert_eq!(tail, ["0x0", "0x6b49d200", "0x51", "0x52"]);
}
