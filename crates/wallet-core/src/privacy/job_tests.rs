//! What the recorded SDK jobs cannot show (`strk20_job_parity` replays those): one whole job,
//! refused routes, closing, and the cases the deleted engine's tests held that still mean
//! something (refusal order, nothing signed before binding, limits clamped). The ways a job
//! fails mid-way are `job_failure_tests.rs`, over the helpers here.

use std::cell::{Cell, RefCell};

use super::*;
use crate::catalog::{NetworkInfo, TokenInfo};
use crate::privacy::compiler::CompileRefusal;
use crate::privacy::config::testing::limits;
use crate::privacy::config::testing::{mainnet, POOL};
use crate::privacy::plan::PrivacyReadPlan;
use crate::privacy::proof::APPLY_ACTIONS;
use crate::{sign_stark_hash, Error, NetworkId};

pub(super) const ACCOUNT: Felt =
    Felt::from_hex_unchecked("0x208361e3911b1d93a704400b88e7e53a89171fce64dbb48f8479e4f44b398d9");
pub(super) const VIEWING_KEY: Felt = Felt::from_hex_unchecked("0x5eed");
pub(super) const STRK: Felt =
    Felt::from_hex_unchecked("0x4718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d");
pub(super) const PROVER: &str = "https://p.example/prover";
pub(super) const NOW: u64 = 1_800_000_000_000;

pub(super) fn signer() -> SecretFelt {
    SecretFelt::new(Felt::from(0x1234567890abcdefu64))
}

pub(super) fn public_key() -> Felt {
    Felt::from_hex(&sign_stark_hash(&signer(), "0x1").unwrap().public_key).unwrap()
}

pub(super) fn pool() -> Felt {
    Felt::from_hex(POOL).unwrap()
}

/// The catalog record, chain id in the catalog's spelling (Bramble's `networks.json`).
pub(super) fn network() -> NetworkInfo {
    NetworkInfo {
        network: NetworkId::Mainnet,
        chain_id: "0x534e5f4d41494e".into(),
        explorer_base_url: "https://voyager.online".into(),
        name: "Mainnet".into(),
        rpc_urls: vec!["https://rpc.example".into()],
        tokens: vec![TokenInfo {
            address: felt::canonical(&STRK),
            decimals: 18,
            name: "Starknet Token".into(),
            symbol: "STRK".into(),
        }],
    }
}

pub(super) fn inputs(actions: Vec<ReviewedAction>) -> ProofInputs {
    ProofInputs {
        account: ACCOUNT,
        public_key: public_key(),
        actions,
        proving_block: 7000,
        route: mainnet(false),
        prover_url: PROVER.into(),
        limits: ProofLimits::CEILING,
        max_notes: 64,
        retry: ProverRetry {
            max_retries: 1,
            request_timeout_ms: 30_000,
            base_delay_ms: 1_000,
            max_delay_ms: 30_000,
        },
    }
}

pub(super) fn send() -> Vec<ReviewedAction> {
    vec![ReviewedAction::Transfer {
        token: STRK,
        recipient: Felt::from(0xb0bu64),
        amount: Felt::from(5u64),
    }]
}

pub(super) struct Zeros;

impl Entropy for Zeros {
    fn fill(&mut self, bytes: &mut [u8]) -> Result<(), Error> {
        bytes.fill(0);
        Ok(())
    }
}

/// A wallet that answers from switches and records what it was asked to sign.
pub(super) struct Host {
    pub(super) unlocked: Cell<bool>,
    pub(super) sign: fn(&Felt) -> Result<StarkSignature, SignRefusal>,
    pub(super) signed: RefCell<Vec<Felt>>,
}

pub(super) fn sign_with_account_key(hash: &Felt) -> Result<StarkSignature, SignRefusal> {
    Ok(sign_stark_hash(&signer(), &felt::canonical(hash)).unwrap())
}

impl Host {
    pub(super) fn new() -> Self {
        Self {
            unlocked: Cell::new(true),
            sign: sign_with_account_key,
            signed: RefCell::default(),
        }
    }
}

impl PrivacyHost for Host {
    fn viewing_key(&self, chain_id: &str, pool_address: &str) -> Option<SecretFelt> {
        assert_eq!(chain_id, "0x534e5f4d41494e");
        assert_eq!(pool_address, felt::canonical(&pool()));
        self.unlocked.get().then(|| SecretFelt::new(VIEWING_KEY))
    }
}

impl ProofHost for Host {
    fn sign(&self, hash: &Felt) -> Result<StarkSignature, SignRefusal> {
        self.signed.borrow_mut().push(*hash);
        (self.sign)(hash)
    }
}

pub(super) fn job(actions: Vec<ReviewedAction>) -> ProofJob<Zeros> {
    ProofJob::new(&network(), inputs(actions), Zeros).unwrap()
}

pub(super) fn http(id: u64, status: u16, body: &str) -> PrivacyReply {
    PrivacyReply::Http {
        id,
        status,
        body: body.as_bytes().to_vec(),
        now_ms: NOW,
    }
}

pub(super) fn id(step: &ProofStep) -> u64 {
    match step {
        ProofStep::Post { id, .. } | ProofStep::Rpc { id, .. } => *id,
        other => panic!("no request: {other:?}"),
    }
}

pub(super) fn refusal(step: ProofStep) -> ProofRefusal {
    match step {
        ProofStep::Failed { refusal } => refusal,
        other => panic!("not failed: {other:?}"),
    }
}

pub(super) const NO_CHANNELS: &str = r#"{"block_ref":7000,"channels":[],"cursor":{"channel_discovery_complete":true,"total_n_channels":0},"subchannels":[]}"#;
pub(super) const NO_NOTES: &str = r#"{"block_ref":7000,"channels":[],"subchannels":[],"notes":[],"cursor":{"channel_discovery_complete":true}}"#;

pub(super) fn proved(rpc_id: u64) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":{rpc_id},"result":{{"l2_to_l1_messages":[{{"from_address":"{POOL}","payload":["0x5ca1ab1e","0x0"],"to_address":"0x0"}}],"proof":"cHJvb2Y=","proof_facts":["0x1"]}}}}"#
    )
}

/// Runs a registration up to its first prove attempt; returns that step.
pub(super) fn to_prove(job: &mut ProofJob<Zeros>, host: &Host) -> ProofStep {
    let outgoing = job
        .advance(host, PrivacyReply::Start { now_ms: NOW })
        .unwrap();
    let nonce = job
        .advance(host, http(id(&outgoing), 200, NO_CHANNELS))
        .unwrap();
    let ProofStep::Rpc {
        id,
        ref body,
        purpose,
    } = nonce
    else {
        panic!("{nonce:?}");
    };
    assert_eq!(purpose, ProofPurpose::PoolNonce);
    assert!(body.contains("starknet_getNonce"));
    let reply = format!(r#"{{"jsonrpc":"2.0","id":{id},"result":"0x2a"}}"#);
    job.advance(
        host,
        PrivacyReply::Rpc {
            id,
            body: reply,
            now_ms: NOW,
        },
    )
    .unwrap()
}

#[test]
fn a_registration_runs_every_step_and_signs_once() {
    let (mut job, host) = (job(vec![ReviewedAction::Register]), Host::new());
    let outgoing = job
        .advance(&host, PrivacyReply::Start { now_ms: NOW })
        .unwrap();
    let ProofStep::Post {
        ref url, purpose, ..
    } = outgoing
    else {
        panic!("{outgoing:?}");
    };
    assert_eq!(url, "https://d.example/discovery/v1/sync/outgoing_state");
    assert_eq!(purpose, ProofPurpose::OutgoingState);
    // The step's `Debug` never prints the key it carries.
    assert!(!format!("{outgoing:?}").contains("5eed"));

    let mut job = self::job(vec![ReviewedAction::Register]);
    let prove = to_prove(&mut job, &host);
    let ProofStep::Post {
        id,
        ref url,
        ref body,
        purpose,
    } = prove
    else {
        panic!("{prove:?}");
    };
    assert_eq!((url.as_str(), purpose), (PROVER, ProofPurpose::Prove));
    assert!(std::str::from_utf8(body)
        .unwrap()
        .contains(r#""id":1800000000000,"#));
    assert_eq!(host.signed.borrow().len(), 1, "signed once, before proving");
    let finished = job.advance(&host, http(id, 200, &proved(NOW))).unwrap();
    let ProofStep::Finished { proof } = finished else {
        panic!("{finished:?}");
    };
    assert_eq!(proof.call.contract_address, pool());
    assert_eq!(proof.call.entrypoint, APPLY_ACTIONS);
    assert_eq!(proof.call.calldata, [Felt::ZERO, Felt::ONE]);
    // Closed after a terminal step.
    assert_eq!(
        job.advance(&host, PrivacyReply::Start { now_ms: NOW }),
        Err(PrivacyError::PlanClosed)
    );
}

#[test]
fn a_route_or_prover_url_is_refused_before_any_request() {
    let mut cases = Vec::new();
    let mut ohttp = inputs(vec![ReviewedAction::Register]);
    ohttp.route = mainnet(true);
    cases.push(ohttp);
    let mut other_network = inputs(vec![ReviewedAction::Register]);
    other_network.route.network = NetworkId::Sepolia;
    cases.push(other_network);
    let mut plain_http = inputs(vec![ReviewedAction::Register]);
    plain_http.route.discovery_url = "http://d.example/discovery".into();
    cases.push(plain_http);
    let mut prover = inputs(vec![ReviewedAction::Register]);
    prover.prover_url = "https://p.example/prover?x=1".into();
    cases.push(prover);
    let mut pool_spelling = inputs(vec![ReviewedAction::Register]);
    pool_spelling.route.pool_address = "0x1".into();
    cases.push(pool_spelling);
    for refused in cases {
        assert_eq!(
            ProofJob::new(&network(), refused, Zeros).err(),
            Some(ProofRefusal::InvalidRoute)
        );
    }
}

#[test]
fn wider_limits_are_clamped_to_the_ceiling() {
    let mut wide = inputs(vec![ReviewedAction::Register]);
    wide.limits.invoke_calldata_felts = usize::MAX;
    let job = ProofJob::new(&network(), wide, Zeros).unwrap();
    assert_eq!(job.inputs.limits, ProofLimits::CEILING);
}

#[test]
fn refusals_come_before_the_nonce_and_the_signature() {
    let host = Host::new();
    // The compiler's refusal after the walks: no nonce read, nothing signed.
    let mut job = job(send());
    let outgoing = job
        .advance(&host, PrivacyReply::Start { now_ms: NOW })
        .unwrap();
    let incoming = job
        .advance(&host, http(id(&outgoing), 200, NO_CHANNELS))
        .unwrap();
    let step = job
        .advance(&host, http(id(&incoming), 200, NO_NOTES))
        .unwrap();
    assert_eq!(
        refusal(step),
        ProofRefusal::Compile(CompileRefusal::InsufficientBalance)
    );

    // Actions the compiler refuses before any walk: nothing is requested.
    let mut job = self::job(vec![ReviewedAction::Deposit {
        token: STRK,
        amount: Felt::ZERO,
        recipient: None,
    }]);
    let step = job
        .advance(&host, PrivacyReply::Start { now_ms: NOW })
        .unwrap();
    assert_eq!(
        refusal(step),
        ProofRefusal::Compile(CompileRefusal::InvalidAmount)
    );

    // Calldata over the wallet's bound fails the binding: no nonce read, nothing signed.
    let mut tight = inputs(vec![ReviewedAction::Register]);
    tight.limits.invoke_calldata_felts = 3;
    let mut job = ProofJob::new(&network(), tight, Zeros).unwrap();
    let outgoing = job
        .advance(&host, PrivacyReply::Start { now_ms: NOW })
        .unwrap();
    let step = job
        .advance(&host, http(id(&outgoing), 200, NO_CHANNELS))
        .unwrap();
    assert_eq!(refusal(step), ProofRefusal::UnboundSignRequest);
    assert!(host.signed.borrow().is_empty());
}

#[test]
fn the_prover_reply_is_judged_against_the_pool() {
    let (mut job, host) = (job(vec![ReviewedAction::Register]), Host::new());
    let prove = to_prove(&mut job, &host);
    let other_pool = proved(NOW).replace(POOL, "0x7");
    let step = job
        .advance(&host, http(id(&prove), 200, &other_pool))
        .unwrap();
    assert_eq!(refusal(step), ProofRefusal::UnboundCallOrProof);
}

#[test]
fn a_catalog_whose_chain_id_is_another_chain_is_refused() {
    let mut sepolia_spelling = network();
    sepolia_spelling.chain_id = "0x534e5f5345504f4c4941".into();
    let mut unparsed = network();
    unparsed.chain_id = "SN_MAIN".into();
    for catalog in [sepolia_spelling, unparsed] {
        let refused = ProofJob::new(&catalog, inputs(vec![ReviewedAction::Register]), Zeros);
        assert_eq!(refused.err(), Some(ProofRefusal::InvalidRoute));
    }
}

/// Records the scope each `viewing_key` call names.
struct Scopes(RefCell<Vec<(String, String)>>);

impl PrivacyHost for Scopes {
    fn viewing_key(&self, chain_id: &str, pool_address: &str) -> Option<SecretFelt> {
        self.0
            .borrow_mut()
            .push((chain_id.to_owned(), pool_address.to_owned()));
        Some(SecretFelt::new(VIEWING_KEY))
    }
}

impl ProofHost for Scopes {
    fn sign(&self, _hash: &Felt) -> Result<StarkSignature, SignRefusal> {
        Err(SignRefusal::Failed)
    }
}

#[test]
fn the_job_scopes_the_viewing_key_exactly_as_the_balance_read() {
    // A catalog spelling with leading zeros: the job must not respell it, or it would prove with
    // another key than the one the balance read discovers with.
    let mut catalog = network();
    catalog.chain_id = "0x00534e5f4d41494e".into();

    let read = Scopes(RefCell::default());
    let mut plan =
        PrivacyReadPlan::new(&catalog, "id".into(), ACCOUNT, mainnet(false), limits()).unwrap();
    let chain = plan
        .advance(&read, PrivacyReply::Start { now_ms: NOW })
        .unwrap();
    let crate::privacy::types::PrivacyStep::Rpc { id, .. } = chain else {
        panic!("{chain:?}");
    };
    let answer = format!(r#"{{"jsonrpc":"2.0","id":{id},"result":"0x534e5f4d41494e"}}"#);
    let preflight = PrivacyReply::Rpc {
        id,
        body: answer,
        now_ms: NOW,
    };
    plan.advance(&read, preflight).unwrap();

    let proving = Scopes(RefCell::default());
    let mut job = ProofJob::new(&catalog, inputs(vec![ReviewedAction::Register]), Zeros).unwrap();
    job.advance(&proving, PrivacyReply::Start { now_ms: NOW })
        .unwrap();

    let expected = ("0x00534e5f4d41494e".to_owned(), felt::canonical(&pool()));
    assert_eq!(*read.0.borrow(), std::slice::from_ref(&expected));
    assert_eq!(*proving.0.borrow(), [expected]);
}
