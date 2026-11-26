//! How a proof job fails mid-way: reorgs past the one restart, spent retries, transport failures
//! at each step, replies out of order, a lock or a bad signature. Helpers are `job_tests.rs`'.

use super::tests::*;
use super::*;
use crate::privacy::types::PrivacyFailure;
use crate::sign_stark_hash;

#[test]
fn a_reorg_restarts_once_and_a_second_fails() {
    // Outgoing twice.
    let (mut job, host) = (job(vec![ReviewedAction::Register]), Host::new());
    let first = job
        .advance(&host, PrivacyReply::Start { now_ms: NOW })
        .unwrap();
    let again = job.advance(&host, http(id(&first), 409, "")).unwrap();
    assert!(matches!(
        again,
        ProofStep::Post {
            purpose: ProofPurpose::OutgoingState,
            ..
        }
    ));
    let failed = job.advance(&host, http(id(&again), 409, "")).unwrap();
    assert_eq!(
        refusal(failed),
        ProofRefusal::Discovery(PrivacyFailure::Reorg)
    );

    // A reorg on the incoming walk restarts from the outgoing one; the restart is spent.
    let mut job = self::job(send());
    let outgoing = job
        .advance(&host, PrivacyReply::Start { now_ms: NOW })
        .unwrap();
    let incoming = job
        .advance(&host, http(id(&outgoing), 200, NO_CHANNELS))
        .unwrap();
    assert!(matches!(
        incoming,
        ProofStep::Post {
            purpose: ProofPurpose::IncomingState,
            ..
        }
    ));
    let restarted = job.advance(&host, http(id(&incoming), 409, "")).unwrap();
    assert!(matches!(
        restarted,
        ProofStep::Post {
            purpose: ProofPurpose::OutgoingState,
            ..
        }
    ));
    let incoming = job
        .advance(&host, http(id(&restarted), 200, NO_CHANNELS))
        .unwrap();
    let failed = job.advance(&host, http(id(&incoming), 409, "")).unwrap();
    assert_eq!(
        refusal(failed),
        ProofRefusal::Discovery(PrivacyFailure::Reorg)
    );
    assert!(host.signed.borrow().is_empty());
}

#[test]
fn a_busy_prover_is_retried_under_the_policy_then_refused() {
    let (mut job, host) = (job(vec![ReviewedAction::Register]), Host::new());
    let prove = to_prove(&mut job, &host);
    let wait = job.advance(&host, http(id(&prove), 503, "busy")).unwrap();
    assert_eq!(wait, ProofStep::Wait { delay_ms: 1_000 });
    let later = NOW + 1_000;
    let retry = job
        .advance(&host, PrivacyReply::WaitElapsed { now_ms: later })
        .unwrap();
    let ProofStep::Post { id, ref body, .. } = retry else {
        panic!("{retry:?}");
    };
    // Each attempt is a new JSON-RPC call under the clock that started it.
    assert!(std::str::from_utf8(body)
        .unwrap()
        .contains(r#""id":1800000001000,"#));
    let busy =
        format!(r#"{{"jsonrpc":"2.0","id":{later},"error":{{"code":-32005,"message":"busy"}}}}"#);
    let spent = job.advance(&host, http(id, 200, &busy)).unwrap();
    assert_eq!(refusal(spent), ProofRefusal::ProverFailed);
    assert_eq!(host.signed.borrow().len(), 1, "a retry never signs again");
}

#[test]
fn a_transport_failure_at_each_step_ends_the_job() {
    let host = Host::new();
    let failure = |id| PrivacyReply::TransportFailure { id, now_ms: NOW };

    let mut job = job(send());
    let outgoing = job
        .advance(&host, PrivacyReply::Start { now_ms: NOW })
        .unwrap();
    let step = job.advance(&host, failure(id(&outgoing))).unwrap();
    let unavailable = ProofRefusal::Discovery(PrivacyFailure::ServiceUnavailable);
    assert_eq!(refusal(step), unavailable);

    let mut job = self::job(send());
    let outgoing = job
        .advance(&host, PrivacyReply::Start { now_ms: NOW })
        .unwrap();
    let incoming = job
        .advance(&host, http(id(&outgoing), 200, NO_CHANNELS))
        .unwrap();
    let step = job.advance(&host, failure(id(&incoming))).unwrap();
    assert_eq!(refusal(step), unavailable);

    let mut job = self::job(vec![ReviewedAction::Register]);
    let outgoing = job
        .advance(&host, PrivacyReply::Start { now_ms: NOW })
        .unwrap();
    let nonce = job
        .advance(&host, http(id(&outgoing), 200, NO_CHANNELS))
        .unwrap();
    let step = job.advance(&host, failure(id(&nonce))).unwrap();
    assert_eq!(refusal(step), ProofRefusal::NonceUnavailable);
    assert!(host.signed.borrow().is_empty());

    let mut job = self::job(vec![ReviewedAction::Register]);
    let prove = to_prove(&mut job, &host);
    let step = job.advance(&host, failure(id(&prove))).unwrap();
    assert_eq!(refusal(step), ProofRefusal::ProverUnavailable);
}

#[test]
fn replies_out_of_order_or_for_another_id_close_the_job() {
    let host = Host::new();
    let mut job = job(vec![ReviewedAction::Register]);
    let outgoing = job
        .advance(&host, PrivacyReply::Start { now_ms: NOW })
        .unwrap();
    assert_eq!(
        job.advance(&host, http(id(&outgoing) + 1, 200, NO_CHANNELS)),
        Err(PrivacyError::OutOfOrder)
    );
    assert_eq!(
        job.advance(&host, http(id(&outgoing), 200, NO_CHANNELS)),
        Err(PrivacyError::PlanClosed)
    );

    for wrong in [
        PrivacyReply::Start { now_ms: NOW },
        PrivacyReply::WaitElapsed { now_ms: NOW },
        PrivacyReply::Rpc {
            id: 1,
            body: String::new(),
            now_ms: NOW,
        },
        PrivacyReply::TransportFailure { id: 2, now_ms: NOW },
    ] {
        let mut job = self::job(vec![ReviewedAction::Register]);
        job.advance(&host, PrivacyReply::Start { now_ms: NOW })
            .unwrap();
        assert_eq!(job.advance(&host, wrong), Err(PrivacyError::OutOfOrder));
    }

    // An Http reply to the nonce read is out of order too.
    let mut job = self::job(vec![ReviewedAction::Register]);
    let outgoing = job
        .advance(&host, PrivacyReply::Start { now_ms: NOW })
        .unwrap();
    let nonce = job
        .advance(&host, http(id(&outgoing), 200, NO_CHANNELS))
        .unwrap();
    assert_eq!(
        job.advance(&host, http(id(&nonce), 200, "{}")),
        Err(PrivacyError::OutOfOrder)
    );
    assert!(host.signed.borrow().is_empty());
}

#[test]
fn a_lock_mid_job_ends_it_locked_and_unsigned() {
    // No key at the start: nothing is requested.
    let host = Host::new();
    host.unlocked.set(false);
    let mut job = job(vec![ReviewedAction::Register]);
    let step = job
        .advance(&host, PrivacyReply::Start { now_ms: NOW })
        .unwrap();
    assert_eq!(refusal(step), ProofRefusal::Locked);

    // Locked between the walk and the compile.
    let host = Host::new();
    let mut job = self::job(vec![ReviewedAction::Register]);
    let outgoing = job
        .advance(&host, PrivacyReply::Start { now_ms: NOW })
        .unwrap();
    host.unlocked.set(false);
    let step = job
        .advance(&host, http(id(&outgoing), 200, NO_CHANNELS))
        .unwrap();
    assert_eq!(refusal(step), ProofRefusal::Locked);

    // Locked at the signature.
    let mut host = Host::new();
    host.sign = |_| Err(SignRefusal::Locked);
    let mut job = self::job(vec![ReviewedAction::Register]);
    assert_eq!(refusal(to_prove(&mut job, &host)), ProofRefusal::Locked);
}

#[test]
fn a_signature_that_does_not_verify_is_never_proved() {
    let mut host = Host::new();
    host.sign = |hash| {
        let other = SecretFelt::new(Felt::from(0x77u64));
        Ok(sign_stark_hash(&other, &felt::canonical(hash)).unwrap())
    };
    let mut job = job(vec![ReviewedAction::Register]);
    assert_eq!(
        refusal(to_prove(&mut job, &host)),
        ProofRefusal::SigningFailed
    );
    host.sign = |_| Ok(sign_stark_hash(&signer(), "0xabc").unwrap());
    let mut job = self::job(vec![ReviewedAction::Register]);
    assert_eq!(
        refusal(to_prove(&mut job, &host)),
        ProofRefusal::SigningFailed
    );
}
