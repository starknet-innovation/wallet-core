//! A multisig's proof job (S7) beyond what the recorded lines show (`strk20_multisig_parity`
//! replays those): a held record tampered in each field, a lock on resume, the binding re-run,
//! the proof window's edges, and no randomness drawn on the resume path. Helpers are
//! `job_tests.rs`'.

use std::cell::{Cell, RefCell};

use super::tests::*;
use super::*;
use crate::accounts::multisig::{signer_guid, tests::class, ArgentMultisigConfig};
use crate::privacy::held::RedactedHeldProof;
use crate::privacy::job_resume::within_proof_window;
use crate::privacy::proof::AuthorizationRefusal;
use crate::{sign_stark_hash, Error};

const MARKER: Felt = Felt::from_hex_unchecked("0x7265646163746564");

fn keys() -> Vec<SecretFelt> {
    [0x111u64, 0x222, 0x333]
        .into_iter()
        .map(|key| SecretFelt::new(Felt::from(key)))
        .collect()
}

fn public(key: &SecretFelt) -> Felt {
    felt::parse(&sign_stark_hash(key, "0x1").unwrap().public_key).unwrap()
}

/// `signers` (indices into `keys()`) over `hash`, in signer-GUID order.
fn aggregate(hash: &Felt, signers: &[usize]) -> Vec<Felt> {
    let keys = keys();
    let mut signed: Vec<(Felt, [Felt; 3])> = signers
        .iter()
        .map(|at| {
            let signature = sign_stark_hash(&keys[*at], &felt::canonical(hash)).unwrap();
            let key = public(&keys[*at]);
            let r = felt::parse(&signature.r).unwrap();
            let s = felt::parse(&signature.s).unwrap();
            (signer_guid(&key).unwrap(), [key, r, s])
        })
        .collect();
    signed.sort_by_key(|(guid, _)| *guid);
    let mut out = vec![Felt::from(signed.len())];
    for (_, [key, r, s]) in signed {
        out.extend([Felt::ZERO, key, r, s]);
    }
    out
}

fn multisig_inputs(actions: Vec<ReviewedAction>) -> ProofInputs {
    ProofInputs {
        signer: ProofSigner::Multisig {
            config: ArgentMultisigConfig {
                threshold: 2,
                signer_public_keys: keys().iter().map(|k| felt::canonical(&public(k))).collect(),
            },
            class: class(),
        },
        ..inputs(actions)
    }
}

/// A multisig wallet: two co-owners have signed once `ready`, none before.
struct Owners {
    unlocked: Cell<bool>,
    ready: Cell<bool>,
    asked: RefCell<Vec<Felt>>,
}

impl Owners {
    fn new(ready: bool) -> Self {
        Self {
            unlocked: Cell::new(true),
            ready: Cell::new(ready),
            asked: RefCell::default(),
        }
    }
}

impl PrivacyHost for Owners {
    fn viewing_key(&self, _: &str, _: &str) -> Option<SecretFelt> {
        self.unlocked.get().then(|| SecretFelt::new(VIEWING_KEY))
    }
}

impl ProofHost for Owners {
    fn sign(&self, _: &Felt) -> Result<StarkSignature, SignRefusal> {
        panic!("a multisig is never asked for one signature")
    }

    fn authorization(&self, hash: &Felt) -> Result<Option<Vec<Felt>>, SignRefusal> {
        self.asked.borrow_mut().push(*hash);
        Ok(self.ready.get().then(|| aggregate(hash, &[0, 2])))
    }
}

/// Draws nothing: a resumed job must never compile.
struct Dry;

impl Entropy for Dry {
    fn fill(&mut self, _: &mut [u8]) -> Result<(), Error> {
        panic!("a resumed job drew randomness")
    }
}

fn rpc(id: u64, result: &str) -> PrivacyReply {
    PrivacyReply::Rpc {
        id,
        body: format!(r#"{{"jsonrpc":"2.0","id":{id},"result":{result}}}"#),
        now_ms: NOW,
    }
}

/// A registration up to its sign step: the end step.
fn compiled(owners: &Owners) -> ProofStep {
    let mut job = ProofJob::with_entropy(
        &network(),
        multisig_inputs(vec![ReviewedAction::Register]),
        Zeros,
    )
    .unwrap();
    let outgoing = job
        .advance(owners, PrivacyReply::Start { now_ms: NOW })
        .unwrap();
    let nonce = job
        .advance(owners, http(id(&outgoing), 200, NO_CHANNELS))
        .unwrap();
    job.advance(owners, rpc(id(&nonce), r#""0x2a""#)).unwrap()
}

fn held() -> RedactedHeldProof {
    let ProofStep::Held { held } = compiled(&Owners::new(false)) else {
        panic!("not held");
    };
    held.redact(&MARKER).unwrap()
}

fn resume(held: RedactedHeldProof, actions: Vec<ReviewedAction>) -> ProofJob<Dry> {
    ProofJob::with_entropy(&network(), multisig_inputs(actions), Dry)
        .unwrap()
        .resumed(held, MARKER)
}

#[test]
fn short_of_its_threshold_the_job_holds_and_a_resume_proves_it() {
    let owners = Owners::new(false);
    let ProofStep::Held { held } = compiled(&owners) else {
        panic!("not held");
    };
    assert_eq!(*owners.asked.borrow(), [held.digest]);
    assert_eq!(
        (held.nonce, held.proving_block),
        (Felt::from(0x2au64), 7000)
    );
    assert!(!format!("{held:?}").contains("5eed"), "redacted Debug");
    let redacted = held.redact(&MARKER).unwrap();
    assert_eq!(redacted.calldata[1], MARKER);

    owners.ready.set(true);
    let mut job = resume(redacted.clone(), vec![ReviewedAction::Register]);
    let block = job
        .advance(&owners, PrivacyReply::Start { now_ms: NOW })
        .unwrap();
    let ProofStep::Rpc {
        id,
        ref body,
        purpose,
    } = block
    else {
        panic!("{block:?}");
    };
    assert_eq!(purpose, ProofPurpose::BlockNumber);
    assert!(body.contains(r#""method":"starknet_blockNumber""#));
    let validity = job.advance(&owners, rpc(id, "7010")).unwrap();
    let ProofStep::Rpc {
        id,
        ref body,
        purpose,
    } = validity
    else {
        panic!("{validity:?}");
    };
    assert_eq!(purpose, ProofPurpose::ProofValidity);
    assert!(body.contains(&felt::canonical(&crate::selector(
        "get_proof_validity_blocks"
    ))));
    let prove = job.advance(&owners, rpc(id, r#"["0x64"]"#)).unwrap();
    let ProofStep::Post {
        id,
        ref body,
        purpose,
        ..
    } = prove
    else {
        panic!("{prove:?}");
    };
    assert_eq!(purpose, ProofPurpose::Prove);
    // The held envelope first, then the rebuilt calldata and the aggregate.
    let body = std::str::from_utf8(body).unwrap();
    assert!(body.contains(r#""block_number":7000"#));
    assert!(body.contains(r#""version":"0x3","calldata":["0x1","#));
    assert!(body.ends_with("]}}}"));
    assert_eq!(owners.asked.borrow().len(), 2);
    assert_eq!(owners.asked.borrow()[1], redacted.digest);
    let finished = job.advance(&owners, http(id, 200, &proved(NOW))).unwrap();
    assert!(
        matches!(finished, ProofStep::Finished { .. }),
        "{finished:?}"
    );
}

#[test]
fn a_complete_aggregate_proves_without_holding() {
    let prove = compiled(&Owners::new(true));
    assert!(matches!(
        prove,
        ProofStep::Post {
            purpose: ProofPurpose::Prove,
            ..
        }
    ));
}

#[test]
fn a_tampered_held_record_is_refused_before_anything_is_sent() {
    let good = held();
    let mut nonce = good.clone();
    nonce.nonce += Felt::ONE;
    let mut block = good.clone();
    block.proving_block -= 1;
    let mut calldata = good.clone();
    let last = calldata.calldata.len() - 1;
    calldata.calldata[last] += Felt::ONE;
    let mut digest = good.clone();
    digest.digest += Felt::ONE;
    let mut marker = good.clone();
    marker.calldata[0] = MARKER;
    for tampered in [nonce, block, calldata, digest, marker] {
        let owners = Owners::new(true);
        let mut job = resume(tampered.clone(), vec![ReviewedAction::Register]);
        let step = job
            .advance(&owners, PrivacyReply::Start { now_ms: NOW })
            .unwrap();
        assert_eq!(refusal(step), ProofRefusal::HeldMismatch, "{tampered:?}");
        assert!(owners.asked.borrow().is_empty(), "nothing asked or sent");
    }
}

#[test]
fn a_resume_rebinds_waits_for_the_threshold_and_needs_a_multisig() {
    let start = |job: &mut ProofJob<Dry>, owners: &Owners| {
        refusal(
            job.advance(owners, PrivacyReply::Start { now_ms: NOW })
                .unwrap(),
        )
    };
    // Other reviewed actions: the held calldata no longer moves what was reviewed.
    let owners = Owners::new(true);
    let mut job = resume(held(), send());
    assert_eq!(start(&mut job, &owners), ProofRefusal::HeldUnbound);
    assert!(owners.asked.borrow().is_empty());
    // Still short of the threshold.
    let owners = Owners::new(false);
    let mut job = resume(held(), vec![ReviewedAction::Register]);
    assert_eq!(
        start(&mut job, &owners),
        ProofRefusal::Authorization(AuthorizationRefusal::BelowThreshold)
    );
    // A single-signer job never holds, so it never resumes.
    let mut job = ProofJob::with_entropy(&network(), inputs(vec![ReviewedAction::Register]), Dry)
        .unwrap()
        .resumed(held(), MARKER);
    assert_eq!(
        start(&mut job, &Owners::new(true)),
        ProofRefusal::HeldMismatch
    );
}

#[test]
fn a_lock_ends_a_resume_at_the_start_or_before_proving() {
    let owners = Owners::new(true);
    owners.unlocked.set(false);
    let mut job = resume(held(), vec![ReviewedAction::Register]);
    let step = job
        .advance(&owners, PrivacyReply::Start { now_ms: NOW })
        .unwrap();
    assert_eq!(refusal(step), ProofRefusal::Locked);

    let owners = Owners::new(true);
    let mut job = resume(held(), vec![ReviewedAction::Register]);
    let block = job
        .advance(&owners, PrivacyReply::Start { now_ms: NOW })
        .unwrap();
    let validity = job.advance(&owners, rpc(id(&block), "7010")).unwrap();
    owners.unlocked.set(false);
    let step = job
        .advance(&owners, rpc(id(&validity), r#"["0x64"]"#))
        .unwrap();
    assert_eq!(refusal(step), ProofRefusal::Locked);
}

#[test]
fn an_aged_out_or_unreadable_window_ends_the_resume() {
    let to_window = |current: &str, validity: &str| {
        let owners = Owners::new(true);
        let mut job = resume(held(), vec![ReviewedAction::Register]);
        let block = job
            .advance(&owners, PrivacyReply::Start { now_ms: NOW })
            .unwrap();
        let validity_read = job.advance(&owners, rpc(id(&block), current)).unwrap();
        if !matches!(validity_read, ProofStep::Rpc { .. }) {
            return validity_read;
        }
        job.advance(&owners, rpc(id(&validity_read), validity))
            .unwrap()
    };
    assert_eq!(to_window("7101", r#"["0x64"]"#), ProofStep::Expired);
    assert_eq!(to_window("7000", r#"["0x64"]"#), ProofStep::Expired);
    for (current, validity) in [
        (r#""0x1b5a""#, r#"["0x64"]"#),
        ("-1", r#"["0x64"]"#),
        ("7010", r#"["0x64","0x1"]"#),
        ("7010", "[]"),
    ] {
        assert_eq!(
            refusal(to_window(current, validity)),
            ProofRefusal::WindowUnavailable,
            "{current} {validity}"
        );
    }
    let owners = Owners::new(true);
    let mut job = resume(held(), vec![ReviewedAction::Register]);
    let block = job
        .advance(&owners, PrivacyReply::Start { now_ms: NOW })
        .unwrap();
    let failed = job
        .advance(
            &owners,
            PrivacyReply::TransportFailure {
                id: id(&block),
                now_ms: NOW,
            },
        )
        .unwrap();
    assert_eq!(refusal(failed), ProofRefusal::WindowUnavailable);
}

#[test]
fn the_proof_window_is_the_pools() {
    let validity = Felt::from(100u64);
    assert!(!within_proof_window(7000, 7000, &validity), "same block");
    assert!(within_proof_window(7000, 7001, &validity));
    assert!(within_proof_window(7000, 7100, &validity), "last block");
    assert!(!within_proof_window(7000, 7101, &validity), "one past");
    assert!(!within_proof_window(7000, 6999, &validity), "before");
    assert!(!within_proof_window(7000, 7001, &Felt::ZERO), "no window");
    assert!(within_proof_window(0, u64::MAX, &Felt::MAX));
}

#[test]
fn a_started_job_cannot_be_turned_into_a_resume() {
    let owners = Owners::new(true);
    let mut job = ProofJob::with_entropy(
        &network(),
        multisig_inputs(vec![ReviewedAction::Register]),
        Zeros,
    )
    .unwrap();
    job.advance(&owners, PrivacyReply::Start { now_ms: NOW })
        .unwrap();
    let mut job = job.resumed(held(), MARKER);
    assert_eq!(
        job.advance(&owners, PrivacyReply::Start { now_ms: NOW }),
        Err(PrivacyError::PlanClosed)
    );
}
