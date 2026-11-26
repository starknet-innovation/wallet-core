use std::cell::RefCell;

use super::*;
use crate::felt;
use crate::privacy::call_set::{call_set_hash, sign_call_set, COMPILE_ACTIONS};
use crate::privacy::config::testing::proof_limits;
use crate::privacy::engine_judge::APPLY_ACTIONS;
use crate::{sign_stark_hash, stark_signature_is_valid};

const POOL: Felt = Felt::from_hex_unchecked("0x7");
const ACCOUNT: Felt = Felt::from_hex_unchecked("0x1234");
const VIEWING_KEY: Felt = Felt::from_hex_unchecked("0x5eed");

fn key() -> SecretFelt {
    SecretFelt::new(Felt::from(0x1234567890abcdefu64))
}

fn public_key() -> Felt {
    Felt::from_hex(&sign_stark_hash(&key(), "0x1").unwrap().public_key).unwrap()
}

fn wallet_sign(hash: &Felt) -> Result<StarkSignature, SignRefusal> {
    Ok(sign_stark_hash(&key(), &felt::canonical(hash)).unwrap())
}

fn job() -> EngineJob {
    EngineJob {
        id: 9,
        chain_id: Felt::from_bytes_be_slice(b"SN_MAIN"),
        pool: POOL,
        account: ACCOUNT,
        actions: vec![EngineAction::Deposit {
            token: Felt::from(0x57u64),
            amount: Felt::from(100u64),
            recipient: None,
        }],
        proving_block: 1_000,
        discovery_url: "https://d.example/discovery".into(),
        prover_url: "https://p.example/prover".into(),
        node_url: "https://rpc.example".into(),
        viewing_key: SecretFelt::new(VIEWING_KEY),
    }
}

fn compile_actions(pool: Felt, user: Felt) -> Call {
    Call {
        contract_address: pool,
        entrypoint: COMPILE_ACTIONS.into(),
        // One `Deposit` (variant 5) of the job's token and amount.
        calldata: vec![
            user,
            VIEWING_KEY,
            Felt::ONE,
            Felt::from(5u64),
            Felt::from(0x57u64),
            Felt::from(100u64),
        ],
    }
}

fn apply_actions(pool: Felt) -> EngineProof {
    EngineProof {
        call: Call {
            contract_address: pool,
            entrypoint: APPLY_ACTIONS.into(),
            calldata: vec![Felt::ONE],
        },
        proof: Proof {
            data: "cHJvb2Y=".into(),
            output: vec![Felt::TWO],
            proof_facts: vec![Felt::THREE],
        },
        warnings: vec![],
    }
}

/// An engine that answers from a script and records what it was given.
struct TestEngine {
    invocation: Result<SignRequest, EngineError>,
    proof: Result<EngineProof, EngineError>,
    jobs: RefCell<Vec<(JobId, Felt)>>,
    proved: RefCell<Vec<(JobId, Vec<Felt>)>>,
    cancelled: RefCell<Vec<JobId>>,
}

impl TestEngine {
    fn answering(calls: Vec<Call>, proof: Result<EngineProof, EngineError>) -> Self {
        Self {
            invocation: Ok(SignRequest {
                calls,
                warnings: vec![],
            }),
            proof,
            jobs: RefCell::default(),
            proved: RefCell::default(),
            cancelled: RefCell::default(),
        }
    }

    fn honest() -> Self {
        Self::answering(
            vec![compile_actions(POOL, ACCOUNT)],
            Ok(apply_actions(POOL)),
        )
    }
}

impl PrivacyEngine for TestEngine {
    fn invocation(&self, job: EngineJob) -> Result<SignRequest, EngineError> {
        self.jobs
            .borrow_mut()
            .push((job.id, *job.viewing_key.expose_secret()));
        self.invocation.clone()
    }

    fn prove(&self, id: JobId, signature: Vec<Felt>) -> Result<EngineProof, EngineError> {
        self.proved.borrow_mut().push((id, signature));
        self.proof.clone()
    }

    fn cancel(&self, id: JobId) {
        self.cancelled.borrow_mut().push(id);
    }
}

fn run(engine: &TestEngine) -> (Result<CallAndProof, ProofRefusal>, Vec<Felt>) {
    let mut signed = vec![];
    let result = prove(engine, job(), &public_key(), &proof_limits(), |hash| {
        signed.push(*hash);
        wallet_sign(hash)
    });
    (result, signed)
}

#[test]
fn signs_the_digest_it_computed_and_returns_the_judged_proof() {
    let engine = TestEngine::honest();
    let (result, signed) = run(&engine);
    let expected = apply_actions(POOL);
    assert_eq!(
        result,
        Ok(CallAndProof {
            call: expected.call,
            proof: expected.proof,
        })
    );
    // The key crossed once, inside the job; the digest is the core's, over the engine's calls.
    assert_eq!(*engine.jobs.borrow(), vec![(9, VIEWING_KEY)]);
    let hash = call_set_hash(&ACCOUNT, &[compile_actions(POOL, ACCOUNT)], &job().chain_id);
    assert_eq!(signed, vec![hash]);
    let proved = engine.proved.borrow();
    let [(9, signature)] = proved.as_slice() else {
        panic!("one prove for job 9: {proved:?}");
    };
    assert!(stark_signature_is_valid(
        &public_key(),
        &hash,
        &signature[0],
        &signature[1]
    ));
    assert!(engine.cancelled.borrow().is_empty());
}

#[test]
fn a_route_that_is_not_a_service_url_never_receives_the_key() {
    for (discovery, prover) in [
        ("http://d.example/discovery", "https://p.example/prover"),
        (
            "https://d.example/discovery",
            "https://p.example/prover?x=1",
        ),
    ] {
        let engine = TestEngine::honest();
        let mut bad = job();
        bad.discovery_url = discovery.into();
        bad.prover_url = prover.into();
        let result = prove(&engine, bad, &public_key(), &proof_limits(), wallet_sign);
        assert_eq!(result, Err(ProofRefusal::InvalidRoute));
        assert!(engine.jobs.borrow().is_empty());
    }
}

#[test]
fn an_unbound_sign_request_is_never_signed_and_cancels_the_job() {
    for calls in [
        vec![compile_actions(Felt::from(8u64), ACCOUNT)],
        vec![compile_actions(POOL, Felt::from(0x4321u64))],
        vec![apply_actions(POOL).call],
        vec![
            compile_actions(POOL, ACCOUNT),
            compile_actions(POOL, ACCOUNT),
        ],
    ] {
        let engine = TestEngine::answering(calls, Ok(apply_actions(POOL)));
        let (result, signed) = run(&engine);
        assert_eq!(result, Err(ProofRefusal::UnboundSignRequest));
        assert!(signed.is_empty());
        assert!(engine.proved.borrow().is_empty());
        assert_eq!(*engine.cancelled.borrow(), vec![9]);
    }
}

#[test]
fn a_warning_on_the_invocation_stops_before_signing() {
    let mut engine = TestEngine::honest();
    engine.invocation = Ok(SignRequest {
        calls: vec![compile_actions(POOL, ACCOUNT)],
        warnings: vec![EngineWarning {
            code: "USER_LINKAGE".into(),
            message: "linked".into(),
        }],
    });
    let (result, signed) = run(&engine);
    assert_eq!(result, Err(ProofRefusal::PrivacyLeak));
    assert!(signed.is_empty());
    assert_eq!(*engine.cancelled.borrow(), vec![9]);
}

#[test]
fn a_proof_for_another_call_is_refused_and_cancelled() {
    let engine = TestEngine::answering(
        vec![compile_actions(POOL, ACCOUNT)],
        Ok(apply_actions(Felt::from(8u64))),
    );
    assert_eq!(run(&engine).0, Err(ProofRefusal::UnboundCallOrProof));
    assert_eq!(*engine.cancelled.borrow(), vec![9]);
}

#[test]
fn engine_failures_and_a_locked_wallet_cancel_the_job() {
    let mut engine = TestEngine::honest();
    engine.invocation = Err(EngineError::TimedOut);
    assert_eq!(
        run(&engine).0,
        Err(ProofRefusal::Engine(EngineError::TimedOut))
    );
    assert_eq!(*engine.cancelled.borrow(), vec![9]);

    let engine = TestEngine::answering(
        vec![compile_actions(POOL, ACCOUNT)],
        Err(EngineError::Unavailable),
    );
    assert_eq!(
        run(&engine).0,
        Err(ProofRefusal::Engine(EngineError::Unavailable))
    );
    assert_eq!(*engine.cancelled.borrow(), vec![9]);

    let engine = TestEngine::honest();
    let locked = prove(&engine, job(), &public_key(), &proof_limits(), |_| {
        Err(SignRefusal::Locked)
    });
    assert_eq!(locked, Err(ProofRefusal::Locked));
    assert!(engine.proved.borrow().is_empty());
    assert_eq!(*engine.cancelled.borrow(), vec![9]);
}

#[test]
fn a_sign_request_moving_other_value_is_never_signed() {
    let mut withdraw = compile_actions(POOL, ACCOUNT);
    // `Withdraw(to, token, amount, random)` appended, the count raised to two.
    withdraw.calldata[2] = Felt::TWO;
    withdraw.calldata.extend([
        Felt::from(0xe5eu64),
        Felt::from(0x57u64),
        Felt::from(100u64),
        Felt::ONE,
    ]);
    let mut swapped_key = compile_actions(POOL, ACCOUNT);
    swapped_key.calldata[1] = Felt::from(0x777u64);
    for call in [withdraw, swapped_key] {
        let engine = TestEngine::answering(vec![call], Ok(apply_actions(POOL)));
        let (result, signed) = run(&engine);
        assert_eq!(result, Err(ProofRefusal::UnboundSignRequest));
        assert!(signed.is_empty());
        assert_eq!(*engine.cancelled.borrow(), vec![9]);
    }
}

#[test]
fn a_panic_after_handover_still_cancels_the_job() {
    let engine = TestEngine::honest();
    let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        prove(&engine, job(), &public_key(), &proof_limits(), |_| {
            panic!("signer panicked")
        })
    }));
    assert!(unwound.is_err());
    assert_eq!(*engine.cancelled.borrow(), vec![9]);
}

#[test]
fn wider_limits_are_clamped_to_the_ceiling() {
    let mut engine = TestEngine::honest();
    engine.proof = Ok(EngineProof {
        warnings: vec![
            EngineWarning {
                code: "USER_LINKAGE".into(),
                message: String::new(),
            };
            65
        ],
        ..apply_actions(POOL)
    });
    let wide = ProofLimits {
        sdk_warnings: usize::MAX,
        ..ProofLimits::CEILING
    };
    let result = prove(&engine, job(), &public_key(), &wide, wallet_sign);
    assert_eq!(result, Err(ProofRefusal::InvalidWarnings));
}

#[test]
fn the_job_debug_never_prints_the_viewing_key() {
    let printed = format!("{:?}", job());
    assert!(!printed.contains("5eed"), "{printed}");
}

// `call-set-signer.test.ts`, the signing half: exactly the digest, as a raw Stark pair.
#[test]
fn signs_exactly_the_digest_as_a_raw_stark_pair() {
    let hash = call_set_hash(&ACCOUNT, &[], &job().chain_id);
    let mut asked = None;
    let signature = sign_call_set(&hash, &public_key(), |digest| {
        asked = Some(*digest);
        wallet_sign(digest)
    })
    .unwrap();
    assert_eq!(asked, Some(hash));
    assert_eq!(signature.len(), 2);
    assert!(stark_signature_is_valid(
        &public_key(),
        &hash,
        &signature[0],
        &signature[1]
    ));
}

#[test]
fn refuses_a_signature_over_another_hash_or_key() {
    let hash = Felt::from(0xabcu64);
    let other_hash = |_: &Felt| Ok(sign_stark_hash(&key(), "0xabd").unwrap());
    assert_eq!(
        sign_call_set(&hash, &public_key(), other_hash),
        Err(ProofRefusal::SigningFailed)
    );
    let other_key =
        |_: &Felt| Ok(sign_stark_hash(&SecretFelt::new(Felt::from(0x77u64)), "0xabc").unwrap());
    assert_eq!(
        sign_call_set(&hash, &public_key(), other_key),
        Err(ProofRefusal::SigningFailed)
    );
    assert_eq!(
        sign_call_set(&hash, &public_key(), |_| Err(SignRefusal::Failed)),
        Err(ProofRefusal::SigningFailed)
    );
}
