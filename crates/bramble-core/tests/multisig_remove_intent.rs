//! Deleting one stored intent (`wallet.removeArgentMultisigIntent`): the intent goes, and a
//! relay serving its package again under a new event id cannot bring it back
//! (`background-argent-multisig-coordination.test.ts`, "an intent the user removed").
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

use crate::multisig_sync_import::{
    chain_at, cosigned, id_of, intent_id, nonce_of, proposal_package, ready_package, receipts,
    share, sync,
};
use crate::multisig_tx_support::*;

use bramble_core::{CoordinationSyncStep, CoreError};
use serde_json::json;

const NOW: u64 = NOW_MS / 1_000;

fn refusal(error: CoreError) -> String {
    match error {
        CoreError::InvalidInput { reason } => reason,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_removed_intent_stays_out_of_later_syncs() {
    let wallet = cosigned();
    let proposal = proposal_package(&wallet);
    let chain = chain_at(&wallet, &nonce_of(&proposal));
    let (outcome, _) = sync(&wallet, &[share(&wallet, &proposal)], &chain);
    assert_eq!(outcome.imported, 1);
    let id = intent_id(&proposal);

    wallet.core.remove_multisig_intent(id.clone()).unwrap();
    assert!(wallet
        .core
        .multisig_intents(wallet.multisig_id())
        .unwrap()
        .is_empty());

    // Shared again, as a new wrap: received and finished with, never imported or read for.
    let (outcome, reads) = sync(&wallet, &[share(&wallet, &proposal)], &chain);
    assert_eq!((outcome.received, outcome.imported, reads), (1, 0, 0));
    assert!(wallet
        .core
        .multisig_intents(wallet.multisig_id())
        .unwrap()
        .is_empty());

    // A second removal finds nothing.
    let error = wallet.core.remove_multisig_intent(id).unwrap_err();
    assert_eq!(refusal(error), "Argent multisig intent not found");
}

#[test]
fn removal_refuses_a_bad_id_a_locked_wallet_and_a_submission_under_way() {
    let wallet = cosigned();
    for id in [String::new(), "x".repeat(257)] {
        let error = wallet.core.remove_multisig_intent(id).unwrap_err();
        assert_eq!(
            refusal(error),
            "invalid Argent multisig intent removal request"
        );
    }
    let unknown = format!("{}:0x1", wallet.multisig_id());
    let error = wallet.core.remove_multisig_intent(unknown).unwrap_err();
    assert_eq!(refusal(error), "Argent multisig intent not found");

    let ready = wallet.ready();
    let submission = wallet
        .core
        .clone()
        .submit_multisig_intent(ready.id.clone(), wallet.current_state(&ready))
        .unwrap();
    let error = wallet
        .core
        .remove_multisig_intent(ready.id.clone())
        .unwrap_err();
    assert_eq!(refusal(error), "Argent multisig intent is being submitted");
    drop(submission);

    wallet.core.lock();
    let error = wallet
        .core
        .remove_multisig_intent(ready.id.clone())
        .unwrap_err();
    assert!(matches!(error, CoreError::Locked), "{error:?}");
    wallet.relock();
    wallet.core.remove_multisig_intent(ready.id).unwrap();
    assert!(wallet
        .core
        .multisig_intents(wallet.multisig_id())
        .unwrap()
        .is_empty());
}

#[test]
fn an_intent_removed_while_the_host_reads_is_not_imported_again() {
    let wallet = cosigned();
    let proposal = proposal_package(&wallet);
    let chain = chain_at(&wallet, &nonce_of(&proposal));
    sync(&wallet, &[share(&wallet, &proposal)], &chain);
    let id = intent_id(&proposal);

    // The co-signer's signatures arrive: an update, so the sync hands out a read.
    let event = share(&wallet, &ready_package(&wallet));
    let plan = wallet
        .core
        .coordination_sync_begin(wallet.multisig_id(), NOW)
        .unwrap();
    plan.accept_frame(0, json!(["EVENT", "mcw-1", event]).to_string());
    plan.accept_frame(0, json!(["EOSE", "mcw-1"]).to_string());
    plan.relay_ended(1);
    let step = wallet
        .core
        .coordination_sync_open(plan.clone(), wallet.tokens())
        .unwrap();
    let CoordinationSyncStep::Read { read } = step else {
        panic!("expected a read");
    };

    // Removed while the host reads.
    wallet.core.remove_multisig_intent(id).unwrap();
    chain(&read);
    let step = wallet.core.coordination_sync_import(plan, read).unwrap();
    let CoordinationSyncStep::Done { outcome } = step else {
        panic!("expected the sync to end");
    };
    assert_eq!((outcome.received, outcome.imported), (1, 0));
    assert!(wallet
        .core
        .multisig_intents(wallet.multisig_id())
        .unwrap()
        .is_empty());
    assert!(
        receipts(&wallet).contains(&id_of(&event)),
        "finished with for good"
    );
}
