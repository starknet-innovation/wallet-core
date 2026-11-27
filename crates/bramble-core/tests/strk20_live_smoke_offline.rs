//! The live smoke harness (`strk20_live_smoke`) run offline: the same `run` over a stub transport
//! that answers from recorded jobs of `fixtures/extension-strk20-jobs.jsonl`, with the recorded
//! random bytes. The dev wallet is the repository's published parity phrase (no funds), whose
//! OpenZeppelin account 0 is the account those jobs ran for, so the discovery bodies the harness
//! derives must equal the recorded ones byte for byte: the viewing key is the wallet's.
//!
//! The recorded jobs are mainnet's, so these runs also carry the mainnet acknowledgement.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

use std::collections::{BTreeMap, HashMap, VecDeque};

use serde_json::Value;
use starknet_wallet_core::accounts::Ladder;
use starknet_wallet_core::{account_key, felt};

use crate::common::{live, PHRASE};
use crate::strk20_job_parity::stream;
use crate::strk20_live_smoke::*;
use crate::strk20_support::proof_jobs;

fn job(id: &str) -> Value {
    proof_jobs()
        .into_iter()
        .find(|job| job["id"] == id)
        .unwrap()
}

/// The environment a person would set to run `job` live.
fn env_for(job: &Value) -> HashMap<&'static str, String> {
    let inputs = &job["inputs"];
    let text = |key: &str| inputs[key].as_str().unwrap().to_owned();
    let mut env = HashMap::from([
        (NETWORK, "SN_MAIN".to_owned()),
        (MAINNET_ACK, MAINNET_ACK_TEXT.to_owned()),
        (POOL, text("pool")),
        (DISCOVERY_URL, text("discoveryUrl")),
        (PROVER_URL, text("proverUrl")),
        (RPC_URL, text("rpcUrl")),
        (MNEMONIC, PHRASE.to_owned()),
        (ADDRESS, text("account")),
        (SEND_VIEWING_KEY, SEND_TEXT.to_owned()),
    ]);
    let deposit = inputs["actions"].as_array().unwrap().first();
    let job = match (inputs["builder"].as_str().unwrap(), deposit) {
        ("registration", None) => "register",
        ("activate", Some(_)) => "register-deposit",
        (_, Some(_)) => "deposit",
        other => panic!("not a smoke job: {other:?}"),
    };
    env.insert(JOB, job.to_owned());
    if let Some(deposit) = deposit {
        env.insert(TOKEN, deposit["token"].as_str().unwrap().to_owned());
        env.insert(AMOUNT, deposit["amount"].as_str().unwrap().to_owned());
    }
    env
}

/// Answers from one recorded job: the latest block as its proving block plus 10, then each URL's
/// recorded replies in order. A discovery body must be the recorded one.
struct Stub {
    proving_block: u64,
    recorded: BTreeMap<String, VecDeque<Value>>,
    /// `(url, body length)` of every request, in order.
    calls: Vec<(String, usize)>,
    slept: Vec<u64>,
}

impl Stub {
    fn new(job: &Value) -> Self {
        let mut recorded: BTreeMap<String, VecDeque<Value>> = BTreeMap::new();
        for request in job["requests"].as_array().unwrap() {
            let url = request["url"].as_str().unwrap().to_owned();
            recorded.entry(url).or_default().push_back(request.clone());
        }
        Self {
            proving_block: job["inputs"]["provingBlock"].as_u64().unwrap(),
            recorded,
            calls: Vec::new(),
            slept: Vec::new(),
        }
    }

    fn empty() -> Self {
        Self {
            proving_block: 0,
            recorded: BTreeMap::new(),
            calls: Vec::new(),
            slept: Vec::new(),
        }
    }

    fn unused(&self) -> usize {
        self.recorded.values().map(VecDeque::len).sum()
    }
}

impl Transport for Stub {
    fn post(&mut self, url: &str, body: &[u8], _: u64, _: u64) -> Option<(u16, Vec<u8>)> {
        self.calls.push((url.to_owned(), body.len()));
        let text = std::str::from_utf8(body).unwrap();
        let rpc: Option<Value> = serde_json::from_str(text)
            .ok()
            .filter(|value: &Value| value.get("jsonrpc").is_some());
        if let Some(rpc) = rpc
            .as_ref()
            .filter(|rpc| rpc["method"] == "starknet_blockNumber")
        {
            let reply = serde_json::json!({
                "jsonrpc": "2.0", "id": rpc["id"], "result": self.proving_block + 10,
            });
            return Some((200, reply.to_string().into_bytes()));
        }
        let recorded = self.recorded.get_mut(url)?.pop_front()?;
        let route = recorded["route"].as_str().unwrap();
        if matches!(route, "outgoing" | "incoming") {
            // Not `assert_eq!`: a failure must not print a body, which holds the viewing key.
            let same = text == recorded["body"].as_str().unwrap();
            assert!(same, "the {route} body is not the recorded one");
        }
        let reply = &recorded["reply"];
        let mut body = reply["body"].as_str().unwrap().to_owned();
        if let Some(rpc) = &rpc {
            // The job's JSON-RPC id, as the node would echo it.
            let mut value: Value = serde_json::from_str(&body).unwrap();
            value["id"] = rpc["id"].clone();
            body = value.to_string();
        }
        let status = u16::try_from(reply["status"].as_u64().unwrap()).unwrap();
        Some((status, body.into_bytes()))
    }

    fn sleep(&mut self, delay_ms: u64) {
        self.slept.push(delay_ms);
    }
}

/// Lowercase hex digits of a felt, no `0x`, no leading zeros.
fn digits(value: &str) -> String {
    felt::canonical(&felt::parse(value).unwrap())
        .trim_start_matches("0x")
        .to_owned()
}

/// Fails if any line holds the viewing key (which is also the `compile_actions` key slot), the
/// account key, or a word of the phrase.
fn assert_no_secret(lines: &[String], viewing_key: &str) {
    let account_key = account_key(PHRASE, None, Ladder::Account, 0).unwrap();
    let keys = [
        digits(viewing_key),
        digits(&felt::canonical(account_key.expose_secret())),
    ];
    let words: Vec<&str> = PHRASE.split(' ').collect();
    for line in lines {
        let lower = line.to_lowercase();
        for key in &keys {
            assert!(!lower.contains(key.as_str()), "a key in: {line}");
        }
        for word in lower.split(|c: char| !c.is_ascii_alphanumeric()) {
            assert!(!words.contains(&word), "a phrase word in: {line}");
        }
    }
}

#[test]
fn register_and_activate_shield_prove_through_the_harness() {
    for id in ["register", "activate-shield"] {
        let job = job(id);
        let env = env_for(&job);
        let lookup = |name: &str| env.get(name).cloned();
        let mut stub = Stub::new(&job);
        let smoke = run(&lookup, &mut stub, stream(&job)).unwrap_or_else(|r| panic!("{id}: {r}"));
        assert!(smoke.proved, "{id}: {:?}", smoke.lines);
        assert_eq!(stub.unused(), 0, "{id}: every recorded request is made");
        assert!(stub.slept.is_empty(), "{id}");
        // The block read, then the job's requests; each to the configured hosts only.
        assert_eq!(
            stub.calls.len(),
            job["requests"].as_array().unwrap().len() + 1
        );
        assert!(smoke.lines.iter().any(|line| line.contains(STAGE_B)));
        assert!(smoke
            .lines
            .iter()
            .any(|line| line == "CallSet digests signed: 1"));
        assert_no_secret(&smoke.lines, job["inputs"]["viewingKey"].as_str().unwrap());
    }
}

#[test]
fn every_refusal_comes_before_any_request() {
    let base = env_for(&job("register"));
    let without = |name: &str| {
        let mut env = base.clone();
        env.remove(name);
        env
    };
    let with = |name: &'static str, value: &str| {
        let mut env = base.clone();
        env.insert(name, value.to_owned());
        env
    };
    let cases = [
        (without(POOL), Refusal::Missing(POOL)),
        (without(MNEMONIC), Refusal::Missing(MNEMONIC)),
        (without(JOB), Refusal::Missing(JOB)),
        (
            with(DISCOVERY_URL, "http://d.example/discovery"),
            Refusal::Invalid(DISCOVERY_URL),
        ),
        (
            with(PROVER_URL, "https://p.example/prove?key=1"),
            Refusal::Invalid(PROVER_URL),
        ),
        (
            with(RPC_URL, "http://localhost:5050"),
            Refusal::Invalid(RPC_URL),
        ),
        (without(MAINNET_ACK), Refusal::MainnetNotAcknowledged),
        (with(MAINNET_ACK, "yes"), Refusal::MainnetNotAcknowledged),
        (with(ACCOUNT_INDEX, "1"), Refusal::AddressMismatch),
        (with(JOB, "deposit"), Refusal::Missing(AMOUNT)),
        (without(SEND_VIEWING_KEY), Refusal::NotConfirmed),
        (with(SEND_VIEWING_KEY, "yes"), Refusal::NotConfirmed),
    ];
    for (env, expected) in cases {
        let lookup = |name: &str| env.get(name).cloned();
        let mut stub = Stub::empty();
        let refused = run(&lookup, &mut stub, stream(&job("register")));
        assert_eq!(refused.err(), Some(expected));
        assert!(stub.calls.is_empty(), "{expected:?}: nothing sent");
    }
    // Sepolia needs no acknowledgement; a refusal never echoes a value.
    let sepolia = {
        let mut env = without(MAINNET_ACK);
        env.insert(NETWORK, "SN_SEPOLIA".to_owned());
        env
    };
    assert!(config(&|name: &str| sepolia.get(name).cloned()).is_ok());
    let wrong_phrase = with(MNEMONIC, "abandon abandon zebra");
    let refused = config(&|name: &str| wrong_phrase.get(name).cloned()).err();
    assert_eq!(refused, Some(Refusal::Invalid(MNEMONIC)));
    assert!(!refused.unwrap().to_string().contains("zebra"));
}

#[test]
fn curl_takes_the_body_on_stdin_only() {
    let url = "https://p.example/prove";
    let curl = live::curl_post(url, 4096, 30_000);
    let args: Vec<&str> = curl.get_args().map(|arg| arg.to_str().unwrap()).collect();
    assert_eq!(curl.get_program(), "curl");
    assert_eq!(
        args,
        [
            "-sS",
            "--proto",
            "=https",
            "--max-redirs",
            "0",
            "--max-time",
            "30.000",
            "--max-filesize",
            "4096",
            "-H",
            "Content-Type: application/json",
            "--data-binary",
            "@-",
            "-w",
            "\n%{http_code}",
            url,
        ]
    );
    let args = live::curl_post(url, 1, 1_500);
    assert!(args.get_args().any(|arg| arg == "1.500"));
}
