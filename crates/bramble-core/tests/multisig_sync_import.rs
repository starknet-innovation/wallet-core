//! A coordination sync imports every trusted package it opens, as the extension's sync does
//! (`importSharedIntent`). Ported from the extension's own tests of it,
//! `background-argent-multisig-coordination.test.ts` and
//! `background-argent-multisig-sync-receipts.test.ts`, over G5's fixture multisig: this wallet
//! holds it, and a co-signer (the fixture phrase's Nostr address 1, saved as a contact with a
//! signer's key) shares the extension's own packages. Each import is checked against the fresh
//! read the sync hands out; a wrap is receipted only once the sync is finished with it.
//!
//! Not ported: "an intent the user removed" (mobile cannot remove one intent) and the
//! per-multisig flood (`MAX_INTENTS_PER_MULTISIG` packages; the store's own suite holds the cap).
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

use crate::multisig_support;
use crate::multisig_tx_support;

use std::sync::Arc;

use bramble_core::testing::{nostr_public_key_at, wrap_gift};
use bramble_core::{
    CoordinationSyncOutcome, CoordinationSyncStep, IntentState, MultisigStateRead, NetworkId,
};
use multisig_support::fixture;
use multisig_tx_support::*;
use serde_json::{json, Value};
use starknet_wallet_core::felt::{self, canonical};
use starknet_wallet_core::send::calls::{execute_calldata, Call};
use starknet_wallet_core::send::multisig_intent::SigningIntent;
use starknet_wallet_core::send::sign::invoke_hash;

const NOW: u64 = NOW_MS / 1_000;

fn phrase() -> String {
    fixture()["mnemonic"].as_str().unwrap().to_string()
}

/// The fixture wallet on Sepolia, with the co-signer saved: a contact whose Nostr key is the
/// phrase's address 1 and whose Stark key is the fixture's first co-signer.
fn cosigned() -> Wallet {
    let net = fixture()["networks"][0].clone();
    let wallet = wallet(&net);
    assert_eq!(wallet.network, NetworkId::Sepolia);
    let index = net["proposal"]["cosignerAccountIndexes"][0].clone();
    let signer = net["signers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|signer| signer["accountIndex"] == index)
        .unwrap();
    wallet
        .core
        .save_contact(
            wallet.network,
            "Co-signer".into(),
            format!("0x{}", "5".repeat(63)),
            Some(signer["publicKey"].as_str().unwrap().into()),
            Some(nostr_public_key_at(&phrase(), 1).unwrap()),
        )
        .unwrap();
    wallet
}

/// A package as the co-signer shares it, gift-wrapped for this wallet.
fn share(wallet: &Wallet, package: &str) -> Value {
    let to = wallet.core.nostr_public_key().unwrap();
    let wrapped = wrap_gift(&phrase(), 1, &to, "bramble.multisig", package, NOW).unwrap();
    serde_json::from_str(&wrapped).unwrap()
}

fn proposal_package(wallet: &Wallet) -> String {
    wallet.net["proposal"]["proposalPackage"]
        .as_str()
        .unwrap()
        .into()
}

fn ready_package(wallet: &Wallet) -> String {
    wallet.net["proposal"]["readyPackage"]
        .as_str()
        .unwrap()
        .into()
}

fn nonce_of(package: &str) -> String {
    let package: Value = serde_json::from_str(package).unwrap();
    package["intent"]["prepared"]["nonce"]
        .as_str()
        .unwrap()
        .into()
}

/// A chain holding the fixture's signers at `nonce`.
fn chain_at<'a>(wallet: &'a Wallet, nonce: &str) -> impl Fn(&MultisigStateRead) + 'a {
    let nonce = nonce.to_string();
    move |read| {
        let mut replies = wallet.authority_replies(wallet.threshold(), &wallet.guids());
        replies.push(json!(nonce));
        for result in replies {
            let request = read.next_request().unwrap();
            read.accept_response(request.id, body(request.id, result))
                .unwrap();
        }
        assert!(read.next_request().is_none());
    }
}

/// A node that cannot be reached.
fn node_down(read: &MultisigStateRead) {
    let request = read.next_request().unwrap();
    read.transport_failed(request.id);
}

/// One sync at `NOW`: the relays return `events`, and every read the sync hands out is driven
/// by `chain`. Returns the outcome and how many reads were asked for.
fn sync(
    wallet: &Wallet,
    events: &[Value],
    chain: &dyn Fn(&MultisigStateRead),
) -> (CoordinationSyncOutcome, usize) {
    let plan = wallet
        .core
        .coordination_sync_begin(wallet.multisig_id(), NOW)
        .unwrap();
    for event in events {
        plan.accept_frame(0, json!(["EVENT", "mcw-1", event]).to_string());
    }
    plan.accept_frame(0, json!(["EOSE", "mcw-1"]).to_string());
    plan.relay_ended(1);
    let mut reads = 0;
    let mut step = wallet
        .core
        .coordination_sync_open(plan.clone(), wallet.tokens())
        .unwrap();
    loop {
        match step {
            CoordinationSyncStep::Read { read } => {
                reads += 1;
                chain(&read);
                step = wallet
                    .core
                    .coordination_sync_import(plan.clone(), read)
                    .unwrap();
            }
            CoordinationSyncStep::Done { outcome } => return (outcome, reads),
        }
    }
}

/// The wraps later syncs skip.
fn receipts(wallet: &Wallet) -> Vec<String> {
    let text = std::fs::read_to_string(wallet.dir.path().join("coordination.json")).unwrap();
    let file: Value = serde_json::from_str(&text).unwrap();
    serde_json::from_value(file["checkpoints"][0]["eventIds"].clone()).unwrap()
}

fn id_of(event: &Value) -> String {
    event["id"].as_str().unwrap().into()
}

/// `withCalls`: the same frozen transaction with other calls, re-hashed so it passes every
/// package validator: what a co-signer's own wallet could publish.
fn with_calls(package: &str, calls: &[(&str, &str, &[&str])]) -> String {
    let mut package: Value = serde_json::from_str(package).unwrap();
    let mut intent: SigningIntent = serde_json::from_value(package["intent"].clone()).unwrap();
    let parse = |text: &str| felt::parse(text).unwrap();
    let typed: Vec<Call> = calls
        .iter()
        .map(|(contract, entrypoint, calldata)| Call {
            contract_address: parse(contract),
            entrypoint: (*entrypoint).into(),
            calldata: calldata.iter().map(|value| parse(value)).collect(),
        })
        .collect();
    let calldata = execute_calldata(&typed);
    let prepared = &mut intent.prepared;
    let bounds = prepared.resource_bounds.to_bounds().unwrap();
    let hash = invoke_hash(
        starknet_wallet_core::kms::NetworkId::Sepolia,
        &parse(&prepared.sender_address),
        &calldata,
        &parse(&prepared.nonce),
        &bounds,
    )
    .unwrap();
    prepared.calls = serde_json::from_value(json!(calls
        .iter()
        .map(|(contract, entrypoint, data)| json!({
            "calldata": data.iter().map(|value| canonical(&parse(value))).collect::<Vec<_>>(),
            "contractAddress": canonical(&parse(contract)),
            "entrypoint": entrypoint,
        }))
        .collect::<Vec<_>>()))
    .unwrap();
    prepared.calldata = calldata.iter().map(canonical).collect();
    prepared.transaction_hash = canonical(&hash);
    intent.id = format!("{}:{}", intent.account_id, prepared.transaction_hash);
    intent.signatures.clear();
    intent.state = starknet_wallet_core::send::multisig_intent::IntentState::Collecting;
    package["intent"] = serde_json::to_value(&intent).unwrap();
    package.to_string()
}

fn intent_id(package: &str) -> String {
    let package: Value = serde_json::from_str(package).unwrap();
    package["intent"]["id"].as_str().unwrap().into()
}

#[test]
fn a_shared_proposal_is_imported_once_and_its_signatures_merge_later() {
    let wallet = cosigned();
    let proposal = proposal_package(&wallet);
    let chain = chain_at(&wallet, &nonce_of(&proposal));
    // Shared twice: the second wrap brings nothing new, and is finished with too.
    let events = [share(&wallet, &proposal), share(&wallet, &proposal)];
    let (outcome, reads) = sync(&wallet, &events, &chain);
    let id = intent_id(&proposal);
    assert_eq!(outcome.intent_ids, std::slice::from_ref(&id));
    assert_eq!((outcome.received, outcome.imported, reads), (2, 1, 1));
    let stored = wallet.core.multisig_intents(wallet.multisig_id()).unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].package, proposal);
    assert_eq!(stored[0].state, IntentState::Collecting);

    let (again, reads) = sync(&wallet, &events, &chain);
    assert_eq!((again.received, again.imported, reads), (0, 0, 0));
    assert!(again.intent_ids.is_empty());

    // The co-signers' signatures arrive in the ready package and merge into the stored intent.
    let ready = ready_package(&wallet);
    let (merged, _) = sync(&wallet, &[share(&wallet, &ready)], &chain);
    assert_eq!((merged.received, merged.intent_ids), (1, vec![id]));
    let stored = wallet.core.multisig_intents(wallet.multisig_id()).unwrap();
    assert_eq!(stored[0].package, ready);
    assert_eq!(stored[0].state, IntentState::Ready);
}

#[test]
fn a_wrap_whose_read_failed_is_retried_by_the_next_sync() {
    let wallet = cosigned();
    let proposal = proposal_package(&wallet);
    let event = share(&wallet, &proposal);
    let (outcome, reads) = sync(&wallet, std::slice::from_ref(&event), &node_down);
    assert_eq!((outcome.received, outcome.imported, reads), (0, 0, 1));
    assert!(receipts(&wallet).is_empty());
    assert!(wallet
        .core
        .multisig_intents(wallet.multisig_id())
        .unwrap()
        .is_empty());

    let (outcome, _) = sync(&wallet, &[event], &chain_at(&wallet, &nonce_of(&proposal)));
    assert_eq!(outcome.intent_ids, [intent_id(&proposal)]);
    assert_eq!(outcome.imported, 1);
}

#[test]
fn a_wrap_imported_is_not_opened_again() {
    let wallet = cosigned();
    let proposal = proposal_package(&wallet);
    let chain = chain_at(&wallet, &nonce_of(&proposal));
    let event = share(&wallet, &proposal);
    let (outcome, _) = sync(&wallet, std::slice::from_ref(&event), &chain);
    assert_eq!((outcome.received, outcome.imported), (1, 1));
    let (outcome, reads) = sync(&wallet, std::slice::from_ref(&event), &chain);
    assert_eq!((outcome.received, outcome.imported, reads), (0, 0, 0));
    assert_eq!(receipts(&wallet), [id_of(&event)]);
}

#[test]
fn a_wrap_refused_for_good_is_not_opened_again() {
    let wallet = cosigned();
    let proposal = proposal_package(&wallet);
    let event = share(&wallet, &proposal);
    // The multisig moved past the proposal's nonce before this signer synced.
    let (outcome, _) = sync(
        &wallet,
        std::slice::from_ref(&event),
        &chain_at(&wallet, "0x99"),
    );
    assert_eq!((outcome.received, outcome.imported), (1, 0));
    assert_eq!(receipts(&wallet), [id_of(&event)]);
    let (outcome, reads) = sync(&wallet, &[event], &chain_at(&wallet, "0x99"));
    assert_eq!((outcome.received, reads), (0, 0));
}

#[test]
fn a_call_set_the_safety_checks_refuse_never_stops_the_sync() {
    let wallet = cosigned();
    let honest = proposal_package(&wallet);
    let address = wallet.address();
    let token = wallet.net["proposal"]["calls"][0]["contractAddress"]
        .as_str()
        .unwrap()
        .to_string();
    // A multi-call that includes an account self-call, which the call-safety check refuses.
    let poisoned = with_calls(
        &honest,
        &[
            (&token, "approve", &[]),
            (&address, "change_threshold", &["0x1"]),
        ],
    );
    let events = [share(&wallet, &poisoned), share(&wallet, &honest)];
    let (outcome, reads) = sync(&wallet, &events, &chain_at(&wallet, &nonce_of(&honest)));
    assert_eq!(outcome.intent_ids, [intent_id(&honest)]);
    assert_eq!((outcome.received, outcome.imported, reads), (2, 1, 2));
    assert_eq!(receipts(&wallet).len(), 2);
}

#[test]
fn a_package_from_a_sender_outside_the_signer_set_is_finished_without_a_read() {
    let wallet = cosigned();
    let proposal = proposal_package(&wallet);
    // The same package from address 2, which no saved contact names.
    let to = wallet.core.nostr_public_key().unwrap();
    let stranger = wrap_gift(&phrase(), 2, &to, "bramble.multisig", &proposal, NOW).unwrap();
    let stranger: Value = serde_json::from_str(&stranger).unwrap();
    let (outcome, reads) = sync(
        &wallet,
        std::slice::from_ref(&stranger),
        &chain_at(&wallet, &nonce_of(&proposal)),
    );
    assert_eq!((outcome.received, outcome.imported, reads), (1, 0, 0));
}

#[test]
fn only_the_read_the_sync_handed_out_is_taken() {
    let wallet = cosigned();
    let proposal = proposal_package(&wallet);
    let plan = wallet
        .core
        .coordination_sync_begin(wallet.multisig_id(), NOW)
        .unwrap();
    plan.accept_frame(
        0,
        json!(["EVENT", "mcw-1", share(&wallet, &proposal)]).to_string(),
    );
    plan.accept_frame(0, json!(["EOSE", "mcw-1"]).to_string());
    plan.relay_ended(1);
    let CoordinationSyncStep::Read { read } = wallet
        .core
        .coordination_sync_open(plan.clone(), wallet.tokens())
        .unwrap()
    else {
        panic!("a read first");
    };
    assert!(wallet
        .core
        .coordination_sync_open(plan.clone(), wallet.tokens())
        .is_err());
    let nonce = nonce_of(&proposal);
    let other = wallet.state(wallet.threshold(), &wallet.guids(), &nonce);
    assert!(wallet
        .core
        .coordination_sync_import(plan.clone(), other)
        .is_err());
    chain_at(&wallet, &nonce)(&read);
    let step = wallet
        .core
        .coordination_sync_import(plan.clone(), Arc::clone(&read))
        .unwrap();
    assert!(matches!(step, CoordinationSyncStep::Done { outcome } if outcome.imported == 1));
    assert!(wallet.core.coordination_sync_import(plan, read).is_err());
}

#[test]
fn a_lock_mid_sync_stops_it_and_records_nothing() {
    let wallet = cosigned();
    let proposal = proposal_package(&wallet);
    let plan = wallet
        .core
        .coordination_sync_begin(wallet.multisig_id(), NOW)
        .unwrap();
    plan.accept_frame(
        0,
        json!(["EVENT", "mcw-1", share(&wallet, &proposal)]).to_string(),
    );
    plan.accept_frame(0, json!(["EOSE", "mcw-1"]).to_string());
    plan.relay_ended(1);
    let CoordinationSyncStep::Read { read } = wallet
        .core
        .coordination_sync_open(plan.clone(), wallet.tokens())
        .unwrap()
    else {
        panic!("a read first");
    };
    chain_at(&wallet, &nonce_of(&proposal))(&read);
    // A new session is not the one the sync was opened in.
    wallet.relock();
    assert!(matches!(
        wallet.core.coordination_sync_import(plan, read),
        Err(bramble_core::CoreError::Locked)
    ));
    assert!(wallet
        .core
        .multisig_intents(wallet.multisig_id())
        .unwrap()
        .is_empty());
    assert!(!wallet.dir.path().join("coordination.json").exists());
}

#[test]
fn a_saved_signer_forging_the_snapshot_is_refused_for_good_by_the_chain() {
    let wallet = cosigned();
    let proposal = proposal_package(&wallet);
    // The co-signer's package claims a 3-of-3 the chain does not hold (still a valid package:
    // the snapshot is not in the transaction hash, and the sender's key is in its set).
    let mut forged: Value = serde_json::from_str(&proposal).unwrap();
    assert_eq!(forged["intent"]["configSnapshot"]["threshold"], json!(2));
    forged["intent"]["configSnapshot"]["threshold"] = json!(3);
    let event = share(&wallet, &forged.to_string());
    let (outcome, reads) = sync(
        &wallet,
        std::slice::from_ref(&event),
        &chain_at(&wallet, &nonce_of(&proposal)),
    );
    // Trusted, so read; `ConfigChanged` is final.
    assert_eq!((outcome.received, outcome.imported, reads), (1, 0, 1));
    assert_eq!(receipts(&wallet), [id_of(&event)]);
    assert!(wallet
        .core
        .multisig_intents(wallet.multisig_id())
        .unwrap()
        .is_empty());
}

#[test]
fn an_unreadable_multisig_store_mid_sync_is_retried_not_receipted() {
    let wallet = cosigned();
    let proposal = proposal_package(&wallet);
    let event = share(&wallet, &proposal);
    let store = wallet.dir.path().join("multisigs.json");
    let saved = std::fs::read(&store).unwrap();
    let chain = chain_at(&wallet, &nonce_of(&proposal));
    let (outcome, reads) = sync(&wallet, std::slice::from_ref(&event), &|read| {
        chain(read);
        // The store goes unreadable after the sync opened, before the import reads it.
        std::fs::write(&store, b"{").unwrap();
    });
    assert_eq!((outcome.received, outcome.imported, reads), (0, 0, 1));
    assert!(receipts(&wallet).is_empty());

    std::fs::write(&store, saved).unwrap();
    let (outcome, _) = sync(&wallet, &[event], &chain);
    assert_eq!(outcome.intent_ids, [intent_id(&proposal)]);
}
