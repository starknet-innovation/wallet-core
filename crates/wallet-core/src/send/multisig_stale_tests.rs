use serde_json::json;

use super::*;
use crate::send::multisig_intent::ConfigSnapshot;
use crate::send::multisig_invoke::MultisigSignature;

fn call(amount: &str) -> MultisigCall {
    MultisigCall {
        calldata: vec!["0x1".into(), amount.into(), "0x0".into()],
        contract_address: "0x4718".into(),
        entrypoint: "transfer".into(),
    }
}

/// A fully signed (threshold 1) stale intent at `nonce`; only the fields these rules read matter.
fn stale(nonce: &str, hash: &str) -> SigningIntent {
    SigningIntent {
        account_id: "SN_SEPOLIA:0x1".into(),
        broadcast_transaction_hash: None,
        config_snapshot: ConfigSnapshot {
            class_hash: String::new(),
            signer_guids: Vec::new(),
            threshold: 1,
        },
        id: format!("SN_SEPOLIA:0x1:{hash}"),
        prepared: serde_json::from_value(json!({
            "accountDeploymentData": [], "calldata": [],
            "calls": [{"calldata": ["0x1", "0x5", "0x0"], "contractAddress": "0x4718",
                       "entrypoint": "transfer"}],
            "chainId": "0x1", "feeDataAvailabilityMode": "L1", "nonce": nonce,
            "nonceDataAvailabilityMode": "L1", "paymasterData": [], "resourceBounds": {
                "l1DataGas": {"maxAmount": "0x0", "maxPricePerUnit": "0x0"},
                "l1Gas": {"maxAmount": "0x0", "maxPricePerUnit": "0x0"},
                "l2Gas": {"maxAmount": "0x0", "maxPricePerUnit": "0x0"}},
            "senderAddress": "0x1", "tip": "0x0", "version": "0x3", "transactionHash": hash
        }))
        .unwrap(),
        signatures: vec![MultisigSignature {
            public_key: "0x2".into(),
            r: "0x3".into(),
            s: "0x4".into(),
        }],
        stale_outcome: None,
        stale_reason: Some(StaleReason::Nonce),
        state: IntentState::Stale,
        summary: "Send".into(),
    }
}

fn status(finality: &str, execution: &str) -> Option<Answer> {
    Some(Answer::Result(json!({
        "finality_status": finality, "execution_status": execution
    })))
}

const ACCEPTED: SignedHash = SignedHash::Found {
    status: TxStatus::Accepted,
    seen: true,
};

#[test]
fn a_status_reply_is_seen_only_for_a_real_finality_or_a_revert() {
    assert_eq!(signed_hash(status("ACCEPTED_ON_L2", "SUCCEEDED")), ACCEPTED);
    assert_eq!(
        signed_hash(status("RECEIVED", "")),
        SignedHash::Found {
            status: TxStatus::Pending,
            seen: true
        }
    );
    assert_eq!(
        signed_hash(status("PRE_CONFIRMED", "REVERTED")),
        SignedHash::Found {
            status: TxStatus::Reverted,
            seen: true
        }
    );
    assert_eq!(
        signed_hash(status("WHATEVER", "")),
        SignedHash::Found {
            status: TxStatus::Pending,
            seen: false
        }
    );
    let not_found = Answer::Error {
        code: Some(29),
        text: "Transaction hash not found".into(),
    };
    assert_eq!(signed_hash(Some(not_found)), SignedHash::Absent);
    assert_eq!(signed_hash(None), SignedHash::Unknown);
}

#[test]
fn only_a_sighting_moves_a_stale_intent_to_submitted() {
    let intent = stale("0x5", "0xabc");
    let found = recover_stale(&intent, ACCEPTED, None);
    assert_eq!(found.resolution, Resolution::Submitted);
    assert!(found.journal);
    let stored = found.store.unwrap();
    assert_eq!(stored.state, IntentState::Submitted);
    assert_eq!(stored.broadcast_transaction_hash.as_deref(), Some("0xabc"));
    let unseen = SignedHash::Found {
        status: TxStatus::Pending,
        seen: false,
    };
    for (chain, resolution) in [
        (unseen, Resolution::Unknown),
        (SignedHash::Unknown, Resolution::Unknown),
        (SignedHash::Absent, Resolution::Absent),
    ] {
        let recovered = recover_stale(&intent, chain, None);
        assert_eq!((recovered.resolution, recovered.store), (resolution, None));
    }
    // A version without its threshold is never looked up: co-signers may still complete it.
    let partial = SigningIntent {
        signatures: Vec::new(),
        ..intent.clone()
    };
    assert!(!needs_lookup(&partial));
    assert_eq!(
        recover_stale(&partial, ACCEPTED, None).resolution,
        Resolution::Absent
    );
}

#[test]
fn a_rejected_hash_records_whether_its_nonce_is_unused_and_never_relaxes() {
    let intent = stale("0x5", "0xabc");
    let rejected = SignedHash::Found {
        status: TxStatus::Rejected,
        seen: true,
    };
    let five = Felt::from(5u8);
    let six = Felt::from(6u8);
    let outcome = |intent: &SigningIntent, nonce| {
        recover_stale(intent, rejected, nonce)
            .store
            .and_then(|stored| stored.stale_outcome)
    };
    assert_eq!(
        outcome(&intent, Some(&five)),
        Some(StaleOutcome::RejectedNonceUnused)
    );
    assert_eq!(outcome(&intent, Some(&six)), Some(StaleOutcome::Rejected));
    assert_eq!(outcome(&intent, None), Some(StaleOutcome::Rejected));
    let recorded = SigningIntent {
        stale_outcome: Some(StaleOutcome::Rejected),
        ..intent
    };
    // Already `Rejected`: nothing to store, even though the nonce reads unused.
    assert_eq!(outcome(&recorded, Some(&five)), None);
}

#[test]
fn a_reverted_version_is_journaled_once_and_settled() {
    let intent = stale("0x5", "0xabc");
    let reverted = SignedHash::Found {
        status: TxStatus::Reverted,
        seen: true,
    };
    let recovered = recover_stale(&intent, reverted, None);
    assert_eq!(recovered.resolution, Resolution::Reverted);
    assert!(recovered.journal);
    let settled = recovered.store.unwrap();
    assert_eq!(settled.stale_outcome, Some(StaleOutcome::Reverted));
    assert!(!needs_lookup(&settled));
    let again = recover_stale(&settled, ACCEPTED, None);
    assert_eq!(
        (again.resolution, again.journal),
        (Resolution::Reverted, false)
    );
}

#[test]
fn the_resend_verdict_follows_the_stale_versions() {
    let at_five = stale("0x5", "0xabc");
    let calls = &at_five.prepared.calls;
    let intents = vec![at_five.clone()];
    assert_eq!(stale_versions(&intents, "SN_SEPOLIA:0x1", calls).count(), 1);
    assert_eq!(
        stale_versions(&intents, "SN_SEPOLIA:0x1", &[call("0x6")]).count(),
        0
    );
    assert_eq!(stale_versions(&intents, "SN_SEPOLIA:0x2", calls).count(), 0);
    let verdict = |resolution, nonce| resend([(&at_five, resolution)], nonce);
    assert_eq!(verdict(Resolution::Submitted, "0x5"), Some(Resend::Sent));
    assert_eq!(verdict(Resolution::Absent, "0x6"), Some(Resend::Maybe));
    assert_eq!(verdict(Resolution::Unknown, "0x6"), Some(Resend::Maybe));
    assert_eq!(verdict(Resolution::Rejected, "0x6"), Some(Resend::Maybe));
    assert_eq!(verdict(Resolution::Absent, "0x05"), None);
    assert_eq!(verdict(Resolution::Reverted, "0x6"), None);
}

#[test]
fn calls_compare_as_felts() {
    let padded = MultisigCall {
        calldata: vec!["0x01".into(), "0x5".into(), "0x0".into()],
        contract_address: "0x04718".into(),
        entrypoint: "transfer".into(),
    };
    assert!(same_calls(&[call("0x5")], &[padded]));
    assert!(!same_calls(&[call("0x5")], &[call("0x6")]));
    assert!(!same_calls(&[call("0x5")], &[call("0x5"), call("0x5")]));
}

#[test]
fn a_fee_refusal_is_fee_only_while_the_nonce_holds() {
    let five = Felt::from(5u8);
    assert_eq!(fee_refused_reason(Some(&five), "0x5"), StaleReason::Fee);
    assert_eq!(
        fee_refused_reason(Some(&Felt::from(6u8)), "0x5"),
        StaleReason::Nonce
    );
    assert_eq!(fee_refused_reason(None, "0x5"), StaleReason::Nonce);
    assert!(nonce_moved_past(&Felt::from(6u8), "0x5"));
    assert!(!nonce_moved_past(&five, "0x5"));
    assert!(!nonce_moved_past(&Felt::from(4u8), "0x5"));
}

#[test]
fn a_submitted_intent_settles_on_its_rows_terminal_status() {
    let submitted = SigningIntent {
        state: IntentState::Submitted,
        stale_reason: None,
        broadcast_transaction_hash: Some("0xabc".into()),
        ..stale("0x5", "0xabc")
    };
    let state = |status| settled(&submitted, status).map(|intent| intent.state);
    assert_eq!(state(TxStatus::Accepted), Some(IntentState::Accepted));
    assert_eq!(state(TxStatus::Reverted), Some(IntentState::Rejected));
    assert_eq!(state(TxStatus::Rejected), Some(IntentState::Rejected));
    assert_eq!(state(TxStatus::Pending), None);
    assert_eq!(settled(&stale("0x5", "0xabc"), TxStatus::Accepted), None);
}

#[test]
fn lookups_go_one_at_a_time_and_ignore_stray_replies() {
    let (one, two) = (Felt::from(0xabcu32), Felt::from(0xdefu32));
    let mut lookups = HashLookups::new(2_000, vec![one, two]);
    let (id, body) = lookups.next_request().unwrap();
    assert_eq!(id, 2_000);
    assert!(body.contains("starknet_getTransactionStatus"));
    assert!(!lookups.accept(2_001, None));
    assert!(lookups.accept(2_000, status("ACCEPTED_ON_L2", "SUCCEEDED")));
    assert_eq!(lookups.next_request().unwrap().0, 2_001);
    assert!(lookups.accept(2_001, None));
    assert!(lookups.done());
    assert!(lookups.next_request().is_none());
    assert_eq!(lookups.result("0x0abc"), ACCEPTED);
    assert_eq!(lookups.result("0xdef"), SignedHash::Unknown);
    assert_eq!(lookups.result("0x1"), SignedHash::Unknown);
}

#[test]
fn only_a_single_payment_names_its_payee() {
    let calls = crate::send::multisig_invoke::as_calls(&[call("0x5")]).unwrap();
    assert_eq!(paid_counterparty(&calls), Some(Felt::ONE));
    let batch = [calls.clone(), calls].concat();
    assert_eq!(paid_counterparty(&batch), None);
}
