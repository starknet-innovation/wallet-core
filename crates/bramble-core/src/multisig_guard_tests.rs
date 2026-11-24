use wallet_core::send::judge::{Observation, TxStatus};
use wallet_core::send::multisig_invoke::MultisigCall;

use super::*;

/// The Sepolia proposal's ready intent (G5) and the multisig it belongs to.
fn ready() -> (SigningIntent, Record) {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../tests/fixtures/extension-multisig-transactions.json"
    ))
    .unwrap();
    let net = &fixture["networks"][0];
    let stage = &net["proposal"]["stages"][2]["intent"];
    let intent: SigningIntent = serde_json::from_str(stage.as_str().unwrap()).unwrap();
    let record: Record = serde_json::from_value(net["account"].clone()).unwrap();
    assert_eq!(record.id, intent.account_id);
    (intent, record)
}

fn submitted(intent: &SigningIntent) -> SigningIntent {
    SigningIntent {
        broadcast_transaction_hash: Some(intent.prepared.transaction_hash.clone()),
        state: IntentState::Submitted,
        ..intent.clone()
    }
}

#[test]
fn a_broadcast_files_its_approvals_and_then_its_activity_row() {
    let dir = tempfile::tempdir().unwrap();
    let (intent, record) = ready();
    let mut approving = intent.clone();
    approving.prepared.calls = vec![MultisigCall {
        calldata: vec!["0x123".into(), "0x10".into(), "0x0".into()],
        contract_address: approving.prepared.calls[0].contract_address.clone(),
        entrypoint: "approve".into(),
    }];
    journal(dir.path(), &approving, &record, "0xabc", 7);
    let grants = with_allowance_log(dir.path(), |log| log.list()).unwrap();
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0].spender, "0x123");
    assert_eq!(grants[0].account_id, record.id);

    journal(
        dir.path(),
        &intent,
        &record,
        &intent.prepared.transaction_hash,
        7,
    );
    let rows = with_log(dir.path(), |log| log.list(&record.id));
    let row = rows
        .iter()
        .find(|row| row.hash == intent.prepared.transaction_hash)
        .unwrap();
    assert_eq!(
        (row.status.as_str(), row.kind.as_str(), row.submitted_at),
        ("pending", "invoke", 7)
    );
    assert_eq!(row.origin.as_deref(), Some("wallet"));
    assert_eq!(row.summary, intent.summary);
    // A single transfer names its payee.
    assert_eq!(
        row.counterparty.as_deref(),
        Some(
            felt::canonical(&felt::parse(&intent.prepared.calls[0].calldata[0]).unwrap()).as_str()
        )
    );
}

#[test]
fn a_submitted_intent_settles_from_its_activity_row() {
    let dir = tempfile::tempdir().unwrap();
    let (intent, record) = ready();
    multisig_intents::upsert(dir.path(), &intent).unwrap();
    let sent = submitted(&intent);
    multisig_intents::upsert(dir.path(), &sent).unwrap();
    journal(
        dir.path(),
        &sent,
        &record,
        &intent.prepared.transaction_hash,
        7,
    );
    settle(dir.path(), &record.id).unwrap();
    let state = || multisig_intents::load(dir.path()).unwrap()[0].state;
    assert_eq!(state(), IntentState::Submitted, "still pending");
    let hash = felt::parse(&intent.prepared.transaction_hash).unwrap();
    with_log(dir.path(), |log| {
        log.observe(
            &record.id,
            &hash,
            Observation::Found {
                status: TxStatus::Accepted,
                accepted_finality: true,
                finalized_on_l1: false,
            },
        )
    });
    settle(dir.path(), &record.id).unwrap();
    assert_eq!(state(), IntentState::Accepted);
}

#[test]
fn a_submit_claim_is_held_until_dropped() {
    let (one, two) = (Path::new("/wallet-one"), Path::new("/wallet-two"));
    let claim = SubmitClaim::take(one, "claim-test").unwrap();
    assert!(SubmitClaim::take(one, "claim-test").is_none());
    assert!(SubmitClaim::take(one, "claim-other").is_some());
    assert!(
        SubmitClaim::take(two, "claim-test").is_some(),
        "another wallet's"
    );
    drop(claim);
    assert!(SubmitClaim::take(one, "claim-test").is_some());
}
