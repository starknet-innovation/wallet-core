//! Importing a co-signer's intent package as the extension's `importIntent` does:
//! `readImportIntentInput` (`IntentPackage::parse`), `locallyDerivedImportedIntent`
//! (`SigningIntent::locally_derived_imported`), then the merge, validator and replacement rule.
//! Built from `extension-multisig-transactions-refusals.json`'s ready intent; the extension
//! recorded no import cases, so these hold the Rust to the TypeScript rules directly.

mod multisig_support;

use multisig_support::*;
use serde_json::Value;
use starknet_wallet_core::accounts::multisig::signer_guid;
use starknet_wallet_core::accounts::multisig_records::Record;
use starknet_wallet_core::send::multisig_intent::{
    IntentPackage, IntentState, SigningIntent, StaleReason,
};
use starknet_wallet_core::send::multisig_invoke::MultisigSignature;
use starknet_wallet_core::{felt, sign_stark_hash};

fn refusals() -> Value {
    let path = format!(
        "{}/tests/fixtures/extension-multisig-transactions-refusals.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn ready() -> SigningIntent {
    serde_json::from_str(refusals()["readyIntent"].as_str().unwrap()).unwrap()
}

fn parse(text: &str) -> Option<IntentPackage> {
    IntentPackage::parse(text, &class(), summary_length(), &TestPrimitives::new())
}

/// What a co-signer sends: `intent` in a package, every field it holds kept.
fn sent(intent: &SigningIntent) -> String {
    IntentPackage {
        intent: intent.clone(),
        kind: "bramble.argent-multisig-intent".into(),
        version: 1,
    }
    .to_json()
}

/// The local collecting intent holding only the second signature.
fn local(ready: &SigningIntent) -> SigningIntent {
    SigningIntent {
        signatures: vec![ready.signatures[1].clone()],
        state: IntentState::Collecting,
        ..ready.clone()
    }
}

/// `importIntent` over `local`: parse, derive locally, merge, then the store's two checks.
fn import(local: &SigningIntent, text: &str) -> SigningIntent {
    let incoming = parse(text).unwrap().intent.locally_derived_imported();
    let merged = local.merge_imported(&incoming).unwrap();
    let primitives = TestPrimitives::new();
    assert!(merged.is_valid(&class(), summary_length(), &primitives));
    assert!(local.can_replace(&merged, &class(), summary_length(), &primitives));
    merged
}

#[test]
fn an_explicit_null_is_not_an_absent_key() {
    let intent: Value = serde_json::from_str(refusals()["readyIntent"].as_str().unwrap()).unwrap();
    assert!(serde_json::from_value::<SigningIntent>(intent.clone()).is_ok());
    for key in ["broadcastTransactionHash", "staleOutcome", "staleReason"] {
        let mut nulled = intent.clone();
        nulled[key] = Value::Null;
        assert!(
            serde_json::from_value::<SigningIntent>(nulled).is_err(),
            "{key}"
        );
    }
    // `isArgentMultisigRecord` guards its optional keys with `!== undefined` too.
    let account = refusals()["account"].clone();
    assert!(serde_json::from_value::<Record>(account.clone()).is_ok());
    for key in ["currentConfig", "deployment", "name"] {
        let mut nulled = account.clone();
        nulled[key] = Value::Null;
        assert!(serde_json::from_value::<Record>(nulled).is_err(), "{key}");
    }
}

#[test]
fn only_this_kind_and_version_one_are_read() {
    let ready = ready();
    let text = ready.package().to_json();
    assert_eq!(parse(&text), Some(ready.package()));
    let package: Value = serde_json::from_str(&text).unwrap();
    for (key, value) in [
        ("kind", Value::from("bramble.argent-multisig-intent-v2")),
        ("version", Value::from(2)),
        ("version", Value::from("1")),
        ("extra", Value::from(true)),
    ] {
        let mut changed = package.clone();
        changed[key] = value;
        assert_eq!(parse(&changed.to_string()), None, "{key}");
    }
    // A package whose intent the validator refuses is not read either.
    let mut tampered = ready.clone();
    tampered.prepared.nonce = "0x6".into();
    assert_eq!(parse(&sent(&tampered)), None);
}

#[test]
fn a_submitted_export_imports_as_ready() {
    let ready = ready();
    let submitted = SigningIntent {
        broadcast_transaction_hash: Some(ready.prepared.transaction_hash.clone()),
        state: IntentState::Submitted,
        ..ready.clone()
    };
    let merged = import(&local(&ready), &submitted.package().to_json());
    assert_eq!(merged.state, IntentState::Ready);
    assert_eq!(merged.broadcast_transaction_hash, None);
    assert_eq!(merged.signatures, ready.signatures);
    // Merged without the local derivation, the remote's broadcast hash would ride along on a
    // ready intent, which the validator refuses.
    let underived = local(&ready).merge_imported(&submitted).unwrap();
    assert!(!underived.is_valid(&class(), summary_length(), &TestPrimitives::new()));
}

#[test]
fn a_remote_stale_reason_does_not_survive() {
    let ready = ready();
    let stale = SigningIntent {
        state: IntentState::Stale,
        stale_reason: Some(StaleReason::Fee),
        ..ready.clone()
    };
    let merged = import(&local(&ready), &sent(&stale));
    assert_eq!(merged.state, IntentState::Ready);
    assert_eq!((merged.stale_reason, merged.stale_outcome), (None, None));
    assert_eq!(
        local(&ready).merge_imported(&stale).unwrap().stale_reason,
        Some(StaleReason::Fee),
        "the merge alone carries it in; the derivation is what drops it"
    );
}

#[test]
fn a_merge_stops_at_the_threshold() {
    // 2-of-3: the local intent holds one signer; a co-signer's package holds the other two.
    let ready = ready();
    let config = current_config(&serde_json::from_value(refusals()["account"].clone()).unwrap());
    let held: Vec<&str> = ready
        .signatures
        .iter()
        .map(|s| s.public_key.as_str())
        .collect();
    let third = config
        .signer_public_keys
        .iter()
        .find(|key| !held.contains(&key.as_str()))
        .unwrap();
    let index = (0..5).find(|&i| &signer(i).1 == third).unwrap();
    let (key, _) = signer(index);
    let extra =
        MultisigSignature::from(sign_stark_hash(&key, &ready.prepared.transaction_hash).unwrap());
    let mut others = vec![ready.signatures[0].clone(), extra];
    others.sort_by_key(|s| signer_guid(&felt::parse(&s.public_key).unwrap()).unwrap());
    let incoming = SigningIntent {
        signatures: others,
        ..ready.clone()
    };
    let merged = local(&ready).merge_imported(&incoming).unwrap();
    let primitives = TestPrimitives::new();
    assert_eq!(merged.state, IntentState::Ready);
    assert_eq!(merged.signatures.len(), 2);
    assert!(
        merged.signatures.contains(&ready.signatures[1]),
        "the local signer stays"
    );
    assert!(merged.is_valid(&class(), summary_length(), &primitives));
    assert!(local(&ready).can_replace(&merged, &class(), summary_length(), &primitives));
}

#[test]
fn a_merge_refuses_another_intent() {
    let ready = ready();
    let mut other_hash = ready.clone();
    other_hash.prepared.nonce = "0x6".into();
    let mut other_id = ready.clone();
    other_id.id = format!("{}0", ready.id);
    let mut other_snapshot = ready.clone();
    other_snapshot.config_snapshot.threshold = 3;
    for incoming in [other_hash, other_id, other_snapshot] {
        assert_eq!(local(&ready).merge_imported(&incoming), None);
    }
}
