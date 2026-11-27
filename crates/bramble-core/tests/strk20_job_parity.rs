//! The whole STRK20 proof job (#182, S5, `privacy::job::ProofJob`) against the SDK jobs the
//! extension recorded (`fixtures/extension-strk20-jobs.jsonl`). Each line drives one job with the
//! recorded viewing key, random bytes and replies, and the signer answers with the recorded
//! signature, so the job's own check (the signature verifies under the account's key over the
//! digest it computed) proves the digest is the SDK's.
//!
//! Every request the job makes must be the next one recorded on its route, byte for byte (the
//! nonce body as JSON, with the job's own JSON-RPC id), at the recorded URL; none may be invented
//! and none left over. The SDK ran the two walks concurrently, so each route is replayed in its
//! own order. What each line records decides what the job must end in:
//!
//! - `outcome.callAndProof`: finished with exactly that call and proof, after waiting the retry
//!   policy's delay for every busy prover reply;
//! - an empty `signing`: refused by the compiler with the refusal the SDK's error names, nothing
//!   signed;
//! - `sdk.createProofInvocation.warnings` (`USER_LINKAGE`): `PrivacyLeak` with nothing signed and
//!   no nonce read (the recorded nonce request is the one allowed leftover).
//!
//! The random reads are the SDK's, and every recorded byte is drawn.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};

use bramble_core::catalogs::multisig::argent_multisig_class;
use bramble_core::catalogs::networks;
use serde_json::Value;
use starknet_wallet_core::accounts::multisig::ArgentMultisigConfig;
use starknet_wallet_core::privacy::config::Route;
use starknet_wallet_core::privacy::entropy::Entropy;
use starknet_wallet_core::privacy::job::{
    ProofHost, ProofInputs, ProofJob, ProofPurpose, ProofStep,
};
use starknet_wallet_core::privacy::plan::PrivacyHost;
use starknet_wallet_core::privacy::proof::{ProofLimits, ProofRefusal, ProofSigner};
use starknet_wallet_core::privacy::prover::ProverRetry;
use starknet_wallet_core::privacy::types::PrivacyReply;
use starknet_wallet_core::send::types::SignRefusal;
use starknet_wallet_core::{felt, Error, Felt, NetworkId, SecretFelt, StarkSignature};

use crate::strk20_support::{compile_refusal, contracts, drive, proof_jobs, reviewed};

/// The generator's pinned clock, the SDK's `Date.now()`.
const NOW: u64 = 1_800_000_000_000;

/// Bramble's `STRK20_PROVER_REQUEST` with the SDK's backoff defaults.
pub const BRAMBLE_RETRY: ProverRetry = ProverRetry {
    max_retries: 3,
    request_timeout_ms: 30_000,
    base_delay_ms: 1_000,
    max_delay_ms: 30_000,
};

fn hex(value: &Value) -> Felt {
    felt::parse(value.as_str().unwrap()).unwrap()
}

fn hexes(value: &Value) -> Vec<Felt> {
    value.as_array().unwrap().iter().map(hex).collect()
}

pub fn network(job: &Value) -> NetworkId {
    match job["inputs"]["chainId"].as_str().unwrap() {
        "0x534e5f4d41494e" => NetworkId::Mainnet,
        "0x534e5f5345504f4c4941" => NetworkId::Sepolia,
        other => panic!("unknown chain {other}"),
    }
}

/// The account's signer key, or for a multisig line (`inputs.multisig`) its signers and
/// threshold under Bramble's pinned class.
fn signer(inputs: &Value) -> ProofSigner {
    let Some(multisig) = inputs.get("multisig") else {
        return ProofSigner::Key(hex(&inputs["accountPublicKey"]));
    };
    ProofSigner::Multisig {
        config: ArgentMultisigConfig {
            threshold: u32::try_from(multisig["threshold"].as_u64().unwrap()).unwrap(),
            signer_public_keys: serde_json::from_value(multisig["signerPublicKeys"].clone())
                .unwrap(),
        },
        class: argent_multisig_class().clone(),
    }
}

pub fn inputs(job: &Value) -> ProofInputs {
    let inputs = &job["inputs"];
    let network = network(job);
    assert_eq!(network.chain_id(), hex(&inputs["chainId"]));
    ProofInputs {
        account: hex(&inputs["account"]),
        signer: signer(inputs),
        actions: reviewed(inputs),
        proving_block: inputs["provingBlock"].as_u64().unwrap(),
        route: Route {
            network,
            ohttp: false,
            discovery_url: inputs["discoveryUrl"].as_str().unwrap().to_owned(),
            pool_address: inputs["pool"].as_str().unwrap().to_owned(),
            pool: hex(&inputs["pool"]),
        },
        prover_url: inputs["proverUrl"].as_str().unwrap().to_owned(),
        anonymizer: contracts(inputs).anonymizer,
        limits: ProofLimits::CEILING,
        // The bound the walk parity tests use, above any recorded job's notes.
        max_notes: 64,
        retry: BRAMBLE_RETRY,
    }
}

/// Hands out the job's bytes, recording each read's length; a read past the end fails.
pub struct Stream {
    pub bytes: Vec<u8>,
    pub at: usize,
    pub reads: Vec<usize>,
}

impl Entropy for Stream {
    fn fill(&mut self, out: &mut [u8]) -> Result<(), Error> {
        let end = self.at + out.len();
        let bytes = self
            .bytes
            .get(self.at..end)
            .ok_or_else(|| Error::storage("the recorded stream ran out"))?;
        out.copy_from_slice(bytes);
        self.at = end;
        self.reads.push(out.len());
        Ok(())
    }
}

pub fn stream(job: &Value) -> Stream {
    let text = job["random"]["bytes"].as_str().unwrap();
    Stream {
        bytes: (0..text.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap())
            .collect(),
        at: 0,
        reads: Vec::new(),
    }
}

/// The recorded wallet: its viewing key, and the SDK's signature over whatever it is asked.
struct Host {
    network: NetworkId,
    pool: Felt,
    key: Felt,
    public_key: String,
    signature: Option<(String, String)>,
    asked: RefCell<Vec<Felt>>,
}

impl PrivacyHost for Host {
    fn viewing_key(&self, chain_id: &str, pool_address: &str) -> Option<SecretFelt> {
        // Bramble's catalog spelling, as its balance read asks: the same scope, the same key.
        assert_eq!(chain_id, networks::info(self.network).chain_id);
        assert_eq!(pool_address, felt::canonical(&self.pool));
        Some(SecretFelt::new(self.key))
    }
}

impl ProofHost for Host {
    fn sign(&self, hash: &Felt) -> Result<StarkSignature, SignRefusal> {
        self.asked.borrow_mut().push(*hash);
        let (r, s) = self.signature.clone().ok_or(SignRefusal::Failed)?;
        Ok(StarkSignature {
            public_key: self.public_key.clone(),
            r,
            s,
        })
    }
}

fn host(job: &Value) -> Host {
    let inputs = &job["inputs"];
    let signature = job["signing"].as_array().unwrap().first().map(|signing| {
        let pair = signing["signature"].as_array().unwrap();
        assert_eq!(pair.len(), 2);
        (
            pair[0].as_str().unwrap().to_owned(),
            pair[1].as_str().unwrap().to_owned(),
        )
    });
    Host {
        network: network(job),
        pool: hex(&inputs["pool"]),
        key: hex(&inputs["viewingKey"]),
        public_key: inputs["accountPublicKey"].as_str().unwrap().to_owned(),
        signature,
        asked: RefCell::default(),
    }
}

/// What a run left behind: the end step, the waits, and the recorded requests not used.
pub struct Run {
    pub end: ProofStep,
    pub waits: Vec<u64>,
    pub leftover: BTreeMap<String, usize>,
}

fn route_of(purpose: ProofPurpose) -> &'static str {
    match purpose {
        ProofPurpose::OutgoingState => "outgoing",
        ProofPurpose::IncomingState => "incoming",
        ProofPurpose::PoolNonce => "nonce",
        ProofPurpose::BlockNumber => "blockNumber",
        ProofPurpose::ProofValidity => "call",
        ProofPurpose::Prove => "prover",
    }
}

/// A fresh job for the line, drawing the recorded bytes.
pub fn fresh<'a>(job: &Value, stream: &'a mut Stream) -> ProofJob<&'a mut Stream> {
    ProofJob::with_entropy(networks::info(network(job)), inputs(job), stream).unwrap()
}

fn run(job: &Value, host: &Host, stream: &mut Stream) -> Run {
    replay(job, host, fresh(job, stream))
}

/// Drives `proof_job` from `Start` against the line's recorded requests, each route in its
/// own order, until it ends.
pub fn replay<E: Entropy>(job: &Value, host: &dyn ProofHost, mut proof_job: ProofJob<E>) -> Run {
    let id = job["id"].as_str().unwrap();
    let mut queues: BTreeMap<String, VecDeque<&Value>> = BTreeMap::new();
    for request in job["requests"].as_array().unwrap() {
        let route = request["route"].as_str().unwrap().to_owned();
        queues.entry(route).or_default().push_back(request);
    }
    let mut take = |purpose: ProofPurpose| {
        queues
            .get_mut(route_of(purpose))
            .and_then(VecDeque::pop_front)
            .unwrap_or_else(|| panic!("{id}: an invented {purpose:?} request"))
    };
    let mut waits = Vec::new();
    let end = drive(&mut proof_job, host, NOW, |step| match step {
        ProofStep::Post {
            id: step_id,
            url,
            body,
            purpose,
        } => {
            let recorded = take(*purpose);
            assert_eq!(
                url,
                recorded["url"].as_str().unwrap(),
                "{id}: {purpose:?} URL"
            );
            let expected_url = match purpose {
                ProofPurpose::Prove => job["inputs"]["proverUrl"].as_str().unwrap().to_owned(),
                _ => format!(
                    "{}/v1/sync/{}_state",
                    job["inputs"]["discoveryUrl"].as_str().unwrap(),
                    route_of(*purpose)
                ),
            };
            assert_eq!(*url, expected_url, "{id}");
            assert_eq!(
                std::str::from_utf8(body).unwrap(),
                recorded["body"].as_str().unwrap(),
                "{id}: {purpose:?} body"
            );
            let reply = &recorded["reply"];
            PrivacyReply::Http {
                id: *step_id,
                status: u16::try_from(reply["status"].as_u64().unwrap()).unwrap(),
                body: reply["body"].as_str().unwrap().as_bytes().to_vec(),
                now_ms: NOW,
            }
        }
        ProofStep::Rpc {
            id: step_id,
            body,
            purpose,
        } => {
            let recorded = take(*purpose);
            assert_eq!(
                recorded["url"].as_str().unwrap(),
                job["inputs"]["rpcUrl"].as_str().unwrap(),
                "{id}"
            );
            // `rpc_bodies` sorts its keys, and the JSON-RPC id is the job's step id: equal as
            // JSON once the recorded id is the job's (the body is public).
            let ours: Value = serde_json::from_str(body).unwrap();
            let mut theirs: Value =
                serde_json::from_str(recorded["body"].as_str().unwrap()).unwrap();
            theirs["id"] = (*step_id).into();
            assert_eq!(ours, theirs, "{id}: {purpose:?} body");
            let mut reply: Value =
                serde_json::from_str(recorded["reply"]["body"].as_str().unwrap()).unwrap();
            reply["id"] = (*step_id).into();
            PrivacyReply::Rpc {
                id: *step_id,
                body: reply.to_string(),
                now_ms: NOW,
            }
        }
        ProofStep::Wait { delay_ms } => {
            waits.push(*delay_ms);
            PrivacyReply::WaitElapsed { now_ms: NOW }
        }
        _ => unreachable!("drive answers live steps only"),
    });
    let leftover = queues
        .into_iter()
        .filter(|(_, queue)| !queue.is_empty())
        .map(|(route, queue)| (route, queue.len()))
        .collect();
    Run {
        end,
        waits,
        leftover,
    }
}

#[test]
fn every_recorded_job_runs_as_the_sdk_ran_it() {
    let jobs = proof_jobs();
    let (mut proved, mut refused, mut linked) = (0, 0, 0);
    for job in &jobs {
        let id = job["id"].as_str().unwrap();
        let host = host(job);
        let mut stream = stream(job);
        let run = run(job, &host, &mut stream);

        let reads: Vec<usize> = serde_json::from_value(job["random"]["reads"].clone()).unwrap();
        assert_eq!(stream.reads, reads, "{id}: random reads");
        assert_eq!(stream.at, stream.bytes.len(), "{id}: every byte is drawn");

        let sdk = &job["sdk"]["createProofInvocation"];
        let warned = sdk["warnings"]
            .as_array()
            .is_some_and(|warnings| !warnings.is_empty());
        let asked = host.asked.borrow().clone();
        if let Some(expected) = job["outcome"].get("callAndProof") {
            let ProofStep::Finished { proof } = &run.end else {
                panic!("{id}: {:?}", run.end);
            };
            assert_eq!(asked, [hex(&job["signing"][0]["digest"])], "{id}: digest");
            let call = &expected["call"];
            assert_eq!(
                proof.call.contract_address,
                hex(&call["contract_address"]),
                "{id}"
            );
            assert_eq!(
                proof.call.entrypoint,
                call["entry_point"].as_str().unwrap(),
                "{id}"
            );
            assert_eq!(proof.call.calldata, hexes(&call["calldata"]), "{id}");
            let expected = &expected["proof"];
            assert_eq!(proof.proof.data, expected["data"].as_str().unwrap(), "{id}");
            assert_eq!(proof.proof.output, hexes(&expected["output"]), "{id}");
            assert_eq!(
                proof.proof.proof_facts,
                hexes(&expected["proof_facts"]),
                "{id}"
            );
            // One wait per busy reply: every recorded prove reply but the last.
            let attempts = job["requests"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|request| request["route"] == "prover")
                .count();
            let delays: Vec<u64> = (0..attempts - 1)
                .map(|attempt| {
                    BRAMBLE_RETRY
                        .delay_ms(u32::try_from(attempt).unwrap())
                        .unwrap()
                })
                .collect();
            assert_eq!(run.waits, delays, "{id}: backoff");
            assert!(run.leftover.is_empty(), "{id}: unused {:?}", run.leftover);
            proved += 1;
        } else if warned {
            assert_eq!(
                run.end,
                ProofStep::Failed {
                    refusal: ProofRefusal::PrivacyLeak
                },
                "{id}"
            );
            assert!(asked.is_empty(), "{id}: nothing signed");
            // The SDK read the nonce and signed before the extension refused; the job does
            // neither, so the recorded nonce request is the one left over.
            assert_eq!(
                run.leftover,
                BTreeMap::from([("nonce".to_owned(), 1)]),
                "{id}"
            );
            linked += 1;
        } else {
            assert!(job["signing"].as_array().unwrap().is_empty(), "{id}");
            let expected = compile_refusal(job).unwrap();
            assert_eq!(
                run.end,
                ProofStep::Failed {
                    refusal: ProofRefusal::Compile(expected)
                },
                "{id}"
            );
            assert!(asked.is_empty(), "{id}: nothing signed");
            assert!(run.leftover.is_empty(), "{id}: unused {:?}", run.leftover);
            refused += 1;
        }
    }
    // Tracks the fixture (42 single-signer proof jobs, 3 shadow-account reads and 6 multisig
    // lines today): update it when the fixture is regenerated. S6 added 6 proved invoke jobs and
    // 3 refusals; S7's fixture 6 proved single-signer jobs (the multisig lines are
    // `strk20_multisig_parity`'s).
    assert_eq!((proved, refused, linked), (34, 6, 2));
}

#[test]
fn the_reorg_job_restarts_once_and_the_busy_job_waits_twice() {
    let jobs = proof_jobs();
    let job = |id: &str| jobs.iter().find(|job| job["id"] == id).unwrap();
    let reorg = job("reorg-restart");
    let statuses: Vec<u64> = reorg["requests"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|request| request["route"] == "outgoing")
        .map(|request| request["reply"]["status"].as_u64().unwrap())
        .collect();
    assert_eq!(statuses, [409, 200]);
    let run = run(reorg, &host(reorg), &mut stream(reorg));
    assert!(matches!(run.end, ProofStep::Finished { .. }));
    let busy = job("prover-busy-retry");
    assert_eq!(run_waits(busy), [1_000, 2_000]);
}

fn run_waits(job: &Value) -> Vec<u64> {
    run(job, &host(job), &mut stream(job)).waits
}
