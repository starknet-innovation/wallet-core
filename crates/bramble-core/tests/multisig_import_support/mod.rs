//! Shared by the import suites: `extension-multisig-transactions-refusals.json`'s ready intent
//! and account, the package round trip, and `import_intent` over an authority read driven to its
//! end. Over `multisig_support`, which each suite declares beside this module.
#![allow(dead_code)] // Each import suite reads a different subset.

use super::multisig_support::*;
use serde_json::{json, Value};
use starknet_wallet_core::accounts::multisig_read::{
    MultisigAuthorityRead, MultisigAuthorityStatus,
};
use starknet_wallet_core::accounts::multisig_records::Record;
use starknet_wallet_core::send::call_decode::Known;
use starknet_wallet_core::send::multisig_import::{import_intent, ImportContext, ImportError};
use starknet_wallet_core::send::multisig_intent::{IntentPackage, IntentState, SigningIntent};
use starknet_wallet_core::{felt, Felt, NetworkId};

pub fn refusals() -> Value {
    let path = format!(
        "{}/tests/fixtures/extension-multisig-transactions-refusals.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

pub fn ready() -> SigningIntent {
    serde_json::from_str(refusals()["readyIntent"].as_str().unwrap()).unwrap()
}

pub fn parse(text: &str) -> Option<IntentPackage> {
    IntentPackage::parse(text, &class(), summary_length())
}

/// What a co-signer sends: `intent` in a package, every field it holds kept.
pub fn sent(intent: &SigningIntent) -> String {
    IntentPackage {
        intent: intent.clone(),
        kind: "bramble.argent-multisig-intent".into(),
        version: 1,
    }
    .to_json()
}

/// The local collecting intent holding only the second signature.
pub fn local(ready: &SigningIntent) -> SigningIntent {
    SigningIntent {
        signatures: vec![ready.signatures[1].clone()],
        state: IntentState::Collecting,
        ..ready.clone()
    }
}

/// `importIntent` over `local`: parse, derive locally, merge, then the store's two checks.
pub fn import(local: &SigningIntent, text: &str) -> SigningIntent {
    let incoming = parse(text).unwrap().intent.locally_derived_imported();
    let merged = local.merge_imported(&incoming).unwrap();
    assert!(merged.is_valid(&class(), summary_length()));
    assert!(local.can_replace(&merged, &class(), summary_length()));
    merged
}

pub fn account() -> Record {
    serde_json::from_value(refusals()["account"].clone()).unwrap()
}

pub fn respond(read: &mut MultisigAuthorityRead, result: Value) {
    let request = read.next_request().unwrap();
    let body = json!({"jsonrpc": "2.0", "id": request.id, "result": result}).to_string();
    read.accept_response(request.id, &body).unwrap();
}

/// An authority read of `address` on Sepolia driven to `Available` with `threshold` and
/// `guids`, as `multisig_parity.rs` drives one (the node returns the GUIDs in any order).
pub fn read_of(address: &str, threshold: u32, guids: &[Felt]) -> MultisigAuthorityRead {
    let mut read = MultisigAuthorityRead::new(NetworkId::Sepolia, address, &class()).unwrap();
    respond(&mut read, json!("0x534e5f5345504f4c4941"));
    respond(&mut read, json!({"block_hash": "0x1", "block_number": 1}));
    respond(&mut read, json!(class().class_hash()));
    respond(&mut read, json!([format!("0x{threshold:x}")]));
    let mut reply = vec![json!(format!("0x{:x}", guids.len()))];
    reply.extend(guids.iter().rev().map(|guid| json!(felt::canonical(guid))));
    respond(&mut read, Value::Array(reply));
    assert_eq!(read.status(), Some(MultisigAuthorityStatus::Available));
    read
}

/// What the chain holds for `intent`'s account: its snapshot's authority and its nonce.
pub fn chain(intent: &SigningIntent) -> (u32, Vec<Felt>, Felt) {
    let snapshot = &intent.config_snapshot;
    let guids = snapshot
        .signer_guids
        .iter()
        .map(|guid| felt::parse(guid).unwrap())
        .collect();
    let nonce = felt::parse(&intent.prepared.nonce).unwrap();
    (snapshot.threshold, guids, nonce)
}

/// `import_intent` with a fresh wallet's review arguments: no known tokens, nothing trusted.
pub fn import_with(
    package: &IntentPackage,
    record: &Record,
    stored: Option<&SigningIntent>,
    (threshold, guids, nonce): (u32, Vec<Felt>, Felt),
) -> Result<SigningIntent, ImportError> {
    let read = read_of(&account().address, threshold, &guids);
    import_stored(package, record, stored, &read, &nonce)
}

/// `import_intent` over a read the caller drove.
pub fn import_stored(
    package: &IntentPackage,
    record: &Record,
    stored: Option<&SigningIntent>,
    read: &MultisigAuthorityRead,
    nonce: &Felt,
) -> Result<SigningIntent, ImportError> {
    let known = Known {
        tokens: &[],
        nft_contracts: &[],
    };
    let wallet = ImportContext {
        record,
        stored,
        class: &class(),
        known: &known,
        trusted: &[],
        summary_length: summary_length(),
    };
    import_intent(package, read, nonce, &wallet)
}

/// The ready intent as a co-signer sends it, with a summary that lies about the calls.
pub fn lying(ready: &SigningIntent) -> IntentPackage {
    let lie = SigningIntent {
        summary: "Receive 1000 STRK".into(),
        ..ready.clone()
    };
    parse(&sent(&lie)).unwrap()
}
