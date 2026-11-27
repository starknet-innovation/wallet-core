//! The STRK20 live smoke harness (#182, "Live smoke before S5 merges"): one `ProofJob` against a
//! real discovery service, prover and node, configured entirely from the environment. Never runs
//! by default (`#[ignore]`), refuses to run under CI, and is run by hand by whoever owns the dev
//! wallet. How to run it and every variable: `crates/bramble-core/README.md`, "Live STRK20 smoke".
//!
//! Stage A (the only stage) proves and stops: it reads the latest block, runs the job to its end,
//! and prints a report with no secret in it. It SENDS THE DEV WALLET'S VIEWING KEY to the
//! configured discovery service and prover (that is the protocol) and signs one `CallSet` digest;
//! it submits nothing on chain. Stage B (submitting `apply_actions` with its proof) is not run
//! here: [`STAGE_B`] says why.
//!
//! Named simplification, the proving block: the extension (`official-sdk-maturity.ts`) proves at
//! `latest - PROVING_BLOCK_DEPTH` (10) only once that block is past the accepted block of the
//! scope's previous privacy write, and (`official-sdk-readiness.ts`) once the account's class and,
//! for a deposit, the deposited balances are visible at it. This harness takes `latest - 10` with
//! neither check: the operator must leave at least 10 blocks after deploying or funding the
//! account and after any earlier privacy write by it.
//!
//! Transport: the system `curl` (`common::live::post_json`), the body on stdin, HTTPS only, no
//! redirects, the response bounds and the prove timeout the job's steps require.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

use std::cell::Cell;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bramble_core::catalogs::networks;
use starknet_wallet_core::js::felt::address_label;
use starknet_wallet_core::privacy::entropy::{Entropy, OsEntropy};
use starknet_wallet_core::privacy::job::{
    ProofHost, ProofInputs, ProofJob, ProofPurpose, ProofStep,
};
use starknet_wallet_core::privacy::plan::PrivacyHost;
use starknet_wallet_core::privacy::proof::{ProofLimits, ProofSigner, ReviewedAction};
use starknet_wallet_core::privacy::prover::MAX_PROVE_RESPONSE_BYTES;
use starknet_wallet_core::privacy::types::PrivacyReply;
use starknet_wallet_core::send::types::SignRefusal;
use starknet_wallet_core::send::{judge, rpc_bodies};
use starknet_wallet_core::{
    derive_strk20_viewing_key, felt, privacy, rpc, sign_stark_hash, Felt, SecretFelt,
    StarkSignature,
};

use crate::common::live;
use crate::strk20_job_parity::BRAMBLE_RETRY;
pub use crate::strk20_live_smoke_config::*;
use crate::strk20_support::drive;

/// The extension's `PROVING_BLOCK_DEPTH` (`official-sdk-maturity-resolution.ts`).
const PROVING_BLOCK_DEPTH: u64 = 10;
/// The extension's discovery wait budget (`generated/privacy.json`, `discoveryMs`).
const DISCOVERY_TIMEOUT_MS: u64 = 60_000;
const RPC_TIMEOUT_MS: u64 = 30_000;
/// The bound the walk parity tests use; a dev wallet holds far fewer notes.
const MAX_NOTES: usize = 64;

pub const STAGE_B: &str = "stage B (submission) is not run by this harness: the core builds a \
     proof-bearing INVOKE (SNIP-36, send::invoke::Invoke::with_proof through send::plan::Plan), \
     but submitting one on chain waits for its own step with the dev wallet's owner. Nothing was \
     submitted.";

/// Moves one request. The live harness's is `Curl`; the offline tests' is a stub.
pub trait Transport {
    /// POSTs JSON `body` to `url`: `(status, reply)`, or `None` on a transport failure, a
    /// timeout after `timeout_ms`, or a reply over `limit` bytes. Never logs the body.
    fn post(
        &mut self,
        url: &str,
        body: &[u8],
        limit: u64,
        timeout_ms: u64,
    ) -> Option<(u16, Vec<u8>)>;
    fn sleep(&mut self, delay_ms: u64);
}

pub struct Curl;

impl Transport for Curl {
    fn post(
        &mut self,
        url: &str,
        body: &[u8],
        limit: u64,
        timeout_ms: u64,
    ) -> Option<(u16, Vec<u8>)> {
        live::post_json(url, body, limit, timeout_ms)
    }

    fn sleep(&mut self, delay_ms: u64) {
        std::thread::sleep(Duration::from_millis(delay_ms));
    }
}

/// The dev wallet as the job's host: the scoped viewing key and the `CallSet` signature, both
/// from the account key, as `WalletCore::plan_viewing_key` and `sign_hash` derive them.
struct DevWallet<'a> {
    key: &'a SecretFelt,
    signed: Cell<u32>,
}

impl PrivacyHost for DevWallet<'_> {
    fn viewing_key(&self, chain_id: &str, pool_address: &str) -> Option<SecretFelt> {
        derive_strk20_viewing_key(self.key, chain_id, pool_address).ok()
    }
}

impl ProofHost for DevWallet<'_> {
    fn sign(&self, hash: &Felt) -> Result<StarkSignature, SignRefusal> {
        self.signed.set(self.signed.get() + 1);
        sign_stark_hash(self.key, &felt::canonical(hash)).map_err(|_| SignRefusal::Failed)
    }
}

/// What a run printed, and whether it ended in a proof.
pub struct Smoke {
    pub lines: Vec<String>,
    pub proved: bool,
}

#[derive(Default)]
struct Report(Vec<String>);

impl Report {
    fn note(&mut self, line: String) {
        println!("{line}");
        self.0.push(line);
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// `host/path` of a service URL (no query or credentials by `is_service_url`).
fn host_path(url: &str) -> &str {
    url.trim_start_matches("https://")
}

/// The host alone: an RPC URL's path or query may carry an API key.
fn host(url: &str) -> &str {
    host_path(url).split(['/', '?']).next().unwrap_or_default()
}

fn describe(action: &ReviewedAction) -> String {
    match action {
        ReviewedAction::Register => "register".into(),
        ReviewedAction::Deposit { token, amount, .. } => format!(
            "deposit {amount} base units of {}",
            address_label(&felt::canonical(token))
        ),
        other => format!("{other:?}"),
    }
}

fn reply_text(got: &Option<(u16, Vec<u8>)>) -> String {
    match got {
        Some((status, body)) => format!("HTTP {status}, {} bytes", body.len()),
        None => "transport failure".into(),
    }
}

/// One JSON-RPC read to the node: the reply text on HTTP 200, else `None`.
fn rpc_read(
    config: &Config,
    transport: &mut dyn Transport,
    report: &mut Report,
    what: &str,
    body: &str,
) -> Option<String> {
    let started = Instant::now();
    let got = transport.post(
        &config.rpc_url,
        body.as_bytes(),
        rpc::MAX_RESPONSE_BYTES as u64,
        RPC_TIMEOUT_MS,
    );
    report.note(format!(
        "{what}: RPC {} -> {} ({} ms)",
        host(&config.rpc_url),
        reply_text(&got),
        started.elapsed().as_millis()
    ));
    got.filter(|(status, _)| *status == 200)
        .and_then(|(_, body)| String::from_utf8(body).ok())
}

/// The host's side of one live step.
fn answer(
    step: &ProofStep,
    config: &Config,
    transport: &mut dyn Transport,
    report: &mut Report,
) -> PrivacyReply {
    match step {
        ProofStep::Post {
            id,
            url,
            body,
            purpose,
        } => {
            let (limit, timeout_ms) = match purpose {
                ProofPurpose::Prove => (MAX_PROVE_RESPONSE_BYTES, BRAMBLE_RETRY.request_timeout_ms),
                _ => (privacy::MAX_RESPONSE_BYTES, DISCOVERY_TIMEOUT_MS),
            };
            let started = Instant::now();
            let got = transport.post(url, body, limit as u64, timeout_ms);
            report.note(format!(
                "{purpose:?}: POST {} ({} bytes sent) -> {} ({} ms)",
                host_path(url),
                body.len(),
                reply_text(&got),
                started.elapsed().as_millis()
            ));
            match got {
                Some((status, body)) => PrivacyReply::Http {
                    id: *id,
                    status,
                    body,
                    now_ms: now_ms(),
                },
                None => PrivacyReply::TransportFailure {
                    id: *id,
                    now_ms: now_ms(),
                },
            }
        }
        ProofStep::Rpc { id, body, purpose } => {
            match rpc_read(config, transport, report, &format!("{purpose:?}"), body) {
                Some(body) => PrivacyReply::Rpc {
                    id: *id,
                    body,
                    now_ms: now_ms(),
                },
                None => PrivacyReply::TransportFailure {
                    id: *id,
                    now_ms: now_ms(),
                },
            }
        }
        ProofStep::Wait { delay_ms } => {
            report.note(format!("prover busy: waiting {delay_ms} ms"));
            transport.sleep(*delay_ms);
            PrivacyReply::WaitElapsed { now_ms: now_ms() }
        }
        _ => unreachable!("drive answers live steps only"),
    }
}

/// Stage A: checks the environment, prints where the viewing key will go, and with the send
/// confirmation reads the proving block and runs the job to its end. Nothing is sent before
/// every check has passed.
pub fn run<E: Entropy>(
    env: Env,
    transport: &mut dyn Transport,
    entropy: E,
) -> Result<Smoke, Refusal> {
    let config = config(env)?;
    let mut report = Report::default();
    report.note(format!(
        "network {}; account {}; actions: {}",
        config.network.catalog_id(),
        address_label(&felt::canonical(&config.account)),
        config
            .actions
            .iter()
            .map(describe)
            .collect::<Vec<_>>()
            .join(", ")
    ));
    report.note(format!(
        "the viewing key will be sent to: {} (discovery) and {} (prover); public reads go to {}",
        host_path(&config.route.discovery_url),
        host_path(&config.prover_url),
        host(&config.rpc_url)
    ));
    if env(SEND_VIEWING_KEY).as_deref() != Some(SEND_TEXT) {
        return Err(Refusal::NotConfirmed);
    }

    let latest = rpc_read(
        &config,
        transport,
        &mut report,
        "latest block",
        &rpc_bodies::block_number(0),
    )
    .and_then(|body| judge::block_number(judge::answer(&body, 0)))
    .ok_or(Refusal::ProvingBlockUnavailable)?;
    let proving_block = latest
        .checked_sub(PROVING_BLOCK_DEPTH)
        .ok_or(Refusal::ProvingBlockUnavailable)?;
    report.note(format!(
        "proving block {proving_block} (latest {latest} - {PROVING_BLOCK_DEPTH})"
    ));

    let inputs = ProofInputs {
        account: config.account,
        signer: ProofSigner::Key(config.public_key),
        actions: config.actions.clone(),
        proving_block,
        route: config.route.clone(),
        prover_url: config.prover_url.clone(),
        anonymizer: None,
        limits: ProofLimits::CEILING,
        max_notes: MAX_NOTES,
        retry: BRAMBLE_RETRY,
    };
    let mut job = ProofJob::with_entropy(networks::info(config.network), inputs, entropy)
        .map_err(|_| Refusal::RouteRefused)?;
    let host = DevWallet {
        key: &config.key,
        signed: Cell::new(0),
    };
    let end = drive(&mut job, &host, now_ms(), |step| {
        answer(step, &config, transport, &mut report)
    });
    let proved = matches!(end, ProofStep::Finished { .. });
    report.note(match &end {
        ProofStep::Finished { proof } => format!(
            "outcome: proof ready: apply_actions calldata {} felts; proof data {} characters, \
             output {} felts, proof_facts {} felts",
            proof.call.calldata.len(),
            proof.proof.data.len(),
            proof.proof.output.len(),
            proof.proof.proof_facts.len()
        ),
        ProofStep::Failed { refusal } => format!("outcome: refused: {refusal:?}"),
        ProofStep::Held { .. } => "outcome: held (unexpected for a single signer)".into(),
        _ => "outcome: expired (unexpected for a fresh job)".into(),
    });
    report.note(format!("CallSet digests signed: {}", host.signed.get()));
    report.note(STAGE_B.into());
    Ok(Smoke {
        lines: report.0,
        proved,
    })
}

#[test]
#[ignore = "live: sends the dev wallet's viewing key to the configured STRK20 services"]
fn stage_a_proves_against_the_configured_services() {
    live::refuse_ci();
    let env = |name: &str| std::env::var(name).ok();
    match run(&env, &mut Curl, OsEntropy) {
        Ok(smoke) => assert!(
            smoke.proved,
            "the job ended without a proof (see the outcome)"
        ),
        Err(refusal) => panic!("{refusal}"),
    }
}
