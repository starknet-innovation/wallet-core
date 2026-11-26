//! The extension's protection against paying twice (Sherlock 177, mc-wallet 673), through
//! `WalletCore` on the Sepolia fixture multisig: a co-signer sends a ready intent elsewhere, this
//! wallet's submit finds the nonce moved, and proposing the same calls again is refused once the
//! stale version is found on chain, or needs `confirm_resend` while it is not; a stale version
//! found on chain stops signing and sending the calls too; a fee refusal reads the nonce again to
//! say why the intent is stale; one submission of an intent runs at a time; and removing a
//! multisig never leaves its intents behind.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

use crate::multisig_support;
use crate::multisig_tx_support;

use bramble_core::{
    CoreError, IntentState, MultisigIntent, MultisigTxFailure, MultisigTxStep, SendStep,
    StaleReason, StepReply, TxStatus,
};
use multisig_support::fixture;
use multisig_tx_support::*;
use serde_json::{json, Value};

const ALREADY_SENT: &str = "Argent multisig intent was already sent";
const NONCE_MOVED: &str = "Argent multisig intent nonce is no longer current";

fn sepolia() -> Wallet {
    wallet(&fixture()["networks"][0])
}

fn reason(error: CoreError) -> String {
    match error {
        CoreError::InvalidInput { reason } => reason,
        other => panic!("{other:?}"),
    }
}

fn stored(wallet: &Wallet, id: &str) -> MultisigIntent {
    wallet
        .core
        .multisig_intents(wallet.multisig_id())
        .unwrap()
        .into_iter()
        .find(|intent| intent.id == id)
        .unwrap()
}

/// The fixture's ready intent, submitted after a co-signer already sent it: the nonce read is
/// one past it and the node answers `status` for its hash.
fn submit_after_cosigner(wallet: &Wallet, ready: &MultisigIntent, status: Value) -> String {
    let moved = wallet.intent_state_with(
        &ready.id,
        wallet.threshold(),
        &wallet.guids(),
        "0x6",
        &|_| status.clone(),
    );
    reason(
        wallet
            .core
            .clone()
            .submit_multisig_intent(ready.id.clone(), moved)
            .err()
            .unwrap(),
    )
}

fn failed(failure: MultisigTxFailure) -> MultisigTxStep {
    MultisigTxStep::Failed { failure }
}

/// The paying-twice scenario: the co-signer's broadcast is not visible yet when this wallet's
/// submit finds the nonce moved, so the intent is left stale; by the time the same calls are
/// proposed again it is on chain, and the proposal is refused rather than paying twice.
#[test]
fn calls_a_cosigner_already_sent_are_not_proposed_again() {
    let wallet = sepolia();
    let ready = wallet.ready();
    assert_eq!(
        submit_after_cosigner(&wallet, &ready, not_found()),
        NONCE_MOVED
    );
    let stale = stored(&wallet, &ready.id);
    assert_eq!(
        (stale.state, stale.stale_reason),
        (IntentState::Stale, Some(StaleReason::Nonce))
    );

    let step = propose_at(
        &wallet,
        wallet.transfer(),
        "0x6",
        status("ACCEPTED_ON_L2", "SUCCEEDED"),
        false,
    );
    assert_eq!(step, failed(MultisigTxFailure::AlreadySent));
    // Found, the stale version is submitted under its signed hash and filed in activity.
    let sent = stored(&wallet, &ready.id);
    assert_eq!(sent.state, IntentState::Submitted);
    assert_eq!(
        sent.broadcast_transaction_hash,
        Some(ready.transaction_hash)
    );
    assert_eq!(
        wallet
            .core
            .multisig_intents(wallet.multisig_id())
            .unwrap()
            .len(),
        1,
        "nothing new stored"
    );
}

/// Not found yet, a stale version at another nonce may still have sent the calls through a
/// co-signer's own version: only the person can say, so the proposal waits for `confirm_resend`.
#[test]
fn calls_a_stale_version_may_have_sent_need_confirming() {
    let wallet = sepolia();
    let ready = wallet.ready();
    submit_after_cosigner(&wallet, &ready, not_found());
    let step = propose_at(&wallet, wallet.transfer(), "0x6", not_found(), false);
    assert_eq!(step, failed(MultisigTxFailure::ResendUnconfirmed));
    assert_eq!(stored(&wallet, &ready.id).state, IntentState::Stale);
    let step = propose_at(&wallet, wallet.transfer(), "0x6", not_found(), true);
    assert!(matches!(step, MultisigTxStep::Proposed { .. }), "{step:?}");
}

/// A submit that finds the nonce moved past the intent asks the chain for its signed hash; found,
/// it is submitted and refused as already sent (`nonceMismatch`).
#[test]
fn a_submit_after_the_nonce_moved_finds_the_cosigners_broadcast() {
    let wallet = sepolia();
    let ready = wallet.ready();
    let refused = submit_after_cosigner(&wallet, &ready, status("ACCEPTED_ON_L2", "SUCCEEDED"));
    assert_eq!(refused, ALREADY_SENT);
    let sent = stored(&wallet, &ready.id);
    assert_eq!(sent.state, IntentState::Submitted);
    assert_eq!(sent.stale_reason, Some(StaleReason::Nonce));
    // Nothing more to send: the intent's outcome is now the chain's.
    let again = wallet
        .core
        .clone()
        .submit_multisig_intent(ready.id.clone(), wallet.current_state(&ready))
        .err()
        .unwrap();
    assert_eq!(
        reason(again),
        "Argent multisig intent was already submitted"
    );
}

/// The fee-updated version of stale calls, confirmed and stored, is refused at signing and at
/// submission once the stale version turns up on chain (`refuseSigningResend`).
#[test]
fn a_stale_version_found_on_chain_stops_signing_and_sending_the_calls() {
    let wallet = sepolia();
    let stale = wallet.ready();
    submit_after_cosigner(&wallet, &stale, not_found());
    let MultisigTxStep::Proposed { intent_id } =
        propose_at(&wallet, wallet.transfer(), "0x6", not_found(), true)
    else {
        panic!("not proposed")
    };
    let next = stored(&wallet, &intent_id);
    let cosigners: Vec<u64> = wallet.net["proposal"]["cosignerAccountIndexes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|index| index.as_u64().unwrap())
        .collect();
    let landed = |hash: &str| {
        if hash == stale.transaction_hash {
            status("ACCEPTED_ON_L2", "SUCCEEDED")
        } else {
            not_found()
        }
    };
    let found = || {
        wallet.intent_state_with(
            &next.id,
            wallet.threshold(),
            &wallet.guids(),
            "0x6",
            &landed,
        )
    };
    // While the stale version is not found, the confirmed one is signed as usual.
    let mut signed = next.clone();
    for index in &cosigners {
        signed = wallet.sign(&signed, *index);
    }
    assert_eq!(signed.state, IntentState::Ready);
    let refused = wallet
        .core
        .clone()
        .submit_multisig_intent(next.id.clone(), found())
        .err()
        .unwrap();
    assert_eq!(reason(refused), ALREADY_SENT);
    assert_eq!(stored(&wallet, &stale.id).state, IntentState::Submitted);
}

#[test]
fn signing_stops_at_a_stale_version_found_on_chain() {
    let wallet = sepolia();
    let stale = wallet.ready();
    submit_after_cosigner(&wallet, &stale, not_found());
    let MultisigTxStep::Proposed { intent_id } =
        propose_at(&wallet, wallet.transfer(), "0x6", not_found(), true)
    else {
        panic!("not proposed")
    };
    let landed = status("ACCEPTED_ON_L2", "SUCCEEDED");
    let found = wallet.intent_state_with(
        &intent_id,
        wallet.threshold(),
        &wallet.guids(),
        "0x6",
        &|_| landed.clone(),
    );
    let refused = wallet
        .core
        .sign_multisig_intent(intent_id.clone(), wallet.signer_id(0), found)
        .err()
        .unwrap();
    assert_eq!(reason(refused), ALREADY_SENT);
    assert!(stored(&wallet, &intent_id).signed_public_keys.is_empty());
}

/// A sign or submit needs the read made for its intent: a plain read looked nothing up.
#[test]
fn a_plain_read_signs_and_sends_nothing() {
    let wallet = sepolia();
    let intent = wallet.propose();
    let plain = wallet.state(wallet.threshold(), &wallet.guids(), &intent.nonce);
    let refused = wallet
        .core
        .sign_multisig_intent(intent.id.clone(), wallet.signer_id(0), plain)
        .err()
        .unwrap();
    assert_eq!(
        reason(refused),
        "Argent multisig state read is not for this intent"
    );
}

/// `markFeeRefusedStale`: the fee refusal leaves the intent stale for its fee only while the
/// nonce, read again, is still its own; unreadable is `Nonce`, and a later `Fee` never relaxes it.
#[test]
fn a_fee_refusal_on_submit_marks_the_intent_stale_for_the_right_reason() {
    let wallet = sepolia();
    let ready = wallet.ready();
    let step = fee_refused(&wallet, &ready, Some(&ready.nonce));
    assert_eq!(step, failed(MultisigTxFailure::FeeExpired));
    let stale = stored(&wallet, &ready.id);
    assert_eq!(
        (stale.state, stale.stale_reason),
        (IntentState::Stale, Some(StaleReason::Fee))
    );

    let wallet = sepolia();
    let ready = wallet.ready();
    fee_refused(&wallet, &ready, None);
    assert_eq!(
        stored(&wallet, &ready.id).stale_reason,
        Some(StaleReason::Nonce)
    );
    // Submitted again (a stale intent at its nonce may be), refused again with the nonce
    // unchanged: still `Nonce`.
    fee_refused(&wallet, &ready, Some(&ready.nonce));
    assert_eq!(
        stored(&wallet, &ready.id).stale_reason,
        Some(StaleReason::Nonce)
    );
}

/// One submission of an intent at a time; cancelling one releases the intent.
#[test]
fn two_submissions_of_one_intent_cannot_both_run() {
    let wallet = sepolia();
    let ready = wallet.ready();
    let first = wallet
        .core
        .clone()
        .submit_multisig_intent(ready.id.clone(), wallet.current_state(&ready))
        .unwrap();
    let second = wallet
        .core
        .clone()
        .submit_multisig_intent(ready.id.clone(), wallet.current_state(&ready))
        .err()
        .unwrap();
    assert_eq!(reason(second), "Argent multisig submission claim exists");
    first.close();
    assert!(wallet
        .core
        .clone()
        .submit_multisig_intent(ready.id.clone(), wallet.current_state(&ready))
        .is_ok());
}

/// A submission files its broadcast: the multisig's activity gains a row under the hash.
#[test]
fn a_submission_files_its_activity_row() {
    let wallet = sepolia();
    let ready = wallet.ready();
    let submission = wallet
        .core
        .clone()
        .submit_multisig_intent(ready.id.clone(), wallet.current_state(&ready))
        .unwrap();
    let step = submission.advance(StepReply::Start).unwrap();
    let step = drive(step, &phase(&wallet.net["proposal"]["submit"]), |reply| {
        submission.advance(reply).unwrap()
    });
    assert!(matches!(step, MultisigTxStep::Submitted { .. }));
    let file = wallet.dir.path().join("activity.v1.json");
    let text = std::fs::read_to_string(file).unwrap();
    assert!(text.contains(&ready.transaction_hash), "{text}");
}

/// A submitted intent settles once its row does: `refresh_activity` takes the multisig's id, and
/// the accepted row marks the intent accepted, so it may be evicted to make room.
#[test]
fn a_submitted_intent_settles_through_a_refresh_by_multisig_id() {
    let wallet = sepolia();
    let ready = wallet.ready();
    let submission = wallet
        .core
        .clone()
        .submit_multisig_intent(ready.id.clone(), wallet.current_state(&ready))
        .unwrap();
    let step = submission.advance(StepReply::Start).unwrap();
    let step = drive(step, &phase(&wallet.net["proposal"]["submit"]), |reply| {
        submission.advance(reply).unwrap()
    });
    let MultisigTxStep::Submitted { hash } = step else {
        panic!("{step:?}");
    };
    assert_eq!(stored(&wallet, &ready.id).state, IntentState::Submitted);

    let refresh = wallet
        .core
        .clone()
        .refresh_activity(wallet.multisig_id(), hash)
        .unwrap();
    let SendStep::Rpc { id, body, .. } = refresh.advance(StepReply::Start).unwrap() else {
        panic!("expected the status read");
    };
    assert!(body.contains("starknet_getTransactionStatus"), "{body}");
    let accepted = json!({"jsonrpc": "2.0", "id": id, "result":
        {"finality_status": "ACCEPTED_ON_L2", "execution_status": "SUCCEEDED"}});
    let reply = StepReply::Rpc {
        id,
        body: accepted.to_string(),
        now_ms: 0,
    };
    // The settled row reads its receipt fee once; a failed read records none and settles the same.
    let SendStep::Rpc { id, .. } = refresh.advance(reply).unwrap() else {
        panic!("expected the receipt read");
    };
    let failed = json!({"jsonrpc": "2.0", "id": id, "error": {"code": 29, "message": "x"}});
    let reply = StepReply::Rpc {
        id,
        body: failed.to_string(),
        now_ms: 0,
    };
    let step = refresh.advance(reply).unwrap();
    assert!(
        matches!(
            step,
            SendStep::Finished {
                status: TxStatus::Accepted,
                ..
            }
        ),
        "{step:?}"
    );
    assert_eq!(refresh.signatures_made(), 0);
    assert_eq!(stored(&wallet, &ready.id).state, IntentState::Accepted);
}

/// Removing a multisig drops its intents first: when that fails, the record stays, so removal
/// can be retried and no intent is left without its multisig.
#[test]
fn a_failed_removal_leaves_the_multisig_and_its_intents_together() {
    let wallet = sepolia();
    wallet.propose();
    let intents = wallet.dir.path().join("multisig-intents.json");
    assert!(intents.exists());
    std::fs::write(&intents, "not json").unwrap();
    assert!(wallet.core.remove_multisig(wallet.multisig_id()).is_err());
    assert_eq!(wallet.core.multisigs(wallet.network).unwrap().len(), 1);
}
