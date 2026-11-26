//! `import_multisig_signature` (the extension's `importSignature`), through `WalletCore` over
//! G5's `extension-multisig-transactions.json`: each co-signer's signature pasted onto the
//! proposal yields the extension's stored intent at that stage, and the ready intent submits.
//! Refused: an empty id or a felt not canonical, a signature that does not verify over the
//! intent's hash, a key outside the snapshot, a signer twice, an intent not collecting, a moved
//! chain and a locked wallet. A refused import stores nothing.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

use crate::multisig_support;
use crate::multisig_tx_support;

use bramble_core::{CoreError, IntentState, MultisigIntent, MultisigTxStep, StepReply};
use multisig_support::fixture;
use multisig_tx_support::*;
use serde_json::Value;

fn reason(error: CoreError) -> String {
    match error {
        CoreError::InvalidInput { reason } => reason,
        other => panic!("{other:?}"),
    }
}

/// The stored intent at proposal stage `index`, as the extension wrote it.
fn stage(net: &Value, index: usize) -> Value {
    let text = net["proposal"]["stages"][index]["intent"].as_str().unwrap();
    serde_json::from_str(text).unwrap()
}

/// The signature stage `index` added to stage `index - 1`: `(public key, r, s)`.
fn added(net: &Value, index: usize) -> (String, String, String) {
    let before = stage(net, index - 1)["signatures"].clone();
    let after = stage(net, index)["signatures"].as_array().unwrap().clone();
    let new = after
        .into_iter()
        .find(|signature| !before.as_array().unwrap().contains(signature))
        .unwrap();
    let field = |name: &str| new[name].as_str().unwrap().to_string();
    (field("publicKey"), field("r"), field("s"))
}

fn import(
    wallet: &Wallet,
    intent: &MultisigIntent,
    (key, r, s): (String, String, String),
) -> Result<MultisigIntent, CoreError> {
    wallet.core.import_multisig_signature(
        intent.id.clone(),
        key,
        r,
        s,
        wallet.current_state(intent),
    )
}

fn package_of(net: &Value, index: usize) -> String {
    format!(
        r#"{{"intent":{},"kind":"bramble.argent-multisig-intent","version":1}}"#,
        net["proposal"]["stages"][index]["intent"].as_str().unwrap()
    )
}

#[test]
fn pasted_signatures_build_the_extensions_intents_and_the_ready_one_submits() {
    for net in fixture()["networks"].as_array().unwrap() {
        let wallet = wallet(net);
        let mut intent = wallet.propose();
        for index in 1..=2 {
            intent = import(&wallet, &intent, added(net, index)).unwrap();
            assert_eq!(intent.package, package_of(net, index));
        }
        assert_eq!(intent.state, IntentState::Ready);
        let submission = wallet
            .core
            .clone()
            .submit_multisig_intent(intent.id.clone(), wallet.current_state(&intent))
            .unwrap();
        let step = submission.advance(StepReply::Start).unwrap();
        let step = drive(step, &phase(&net["proposal"]["submit"]), |reply| {
            submission.advance(reply).unwrap()
        });
        let hash = net["proposal"]["transactionHash"].as_str().unwrap();
        assert_eq!(step, MultisigTxStep::Submitted { hash: hash.into() });
    }
}

#[test]
fn a_bad_signature_is_refused_and_nothing_is_stored() {
    let net = &fixture()["networks"][0];
    let wallet = wallet(net);
    let intent = wallet.propose();
    let (key, r, s) = added(net, 1);
    let outsider = wallet
        .core
        .account_records(wallet.network)
        .unwrap()
        .into_iter()
        .find(|account| account.id == wallet.outsider())
        .unwrap()
        .public_key;
    let last = if s.ends_with('0') { '1' } else { '0' };
    let tweaked = format!("{}{last}", &s[..s.len() - 1]);
    let upper = key.to_uppercase().replace("0X", "0x");
    assert_ne!(upper, key);
    let invalid = "signature is not valid for this exact Argent multisig intent";
    let spelling = "invalid Argent multisig signature import";
    let cases = [
        ((key.clone(), r.clone(), tweaked), invalid),
        ((outsider, r.clone(), s.clone()), invalid),
        ((upper, r.clone(), s.clone()), spelling),
        (
            (key.clone(), format!("0x0{}", &r[2..]), s.clone()),
            spelling,
        ),
        ((key.clone(), "0x0".into(), s.clone()), spelling),
    ];
    for (signature, expected) in cases {
        let refused = import(&wallet, &intent, signature.clone()).err().unwrap();
        assert_eq!(reason(refused), expected, "{signature:?}");
    }
    let empty = wallet
        .core
        .import_multisig_signature(String::new(), key, r, s, wallet.current_state(&intent))
        .err()
        .unwrap();
    assert_eq!(reason(empty), spelling);
    let stored = wallet.core.multisig_intents(wallet.multisig_id()).unwrap();
    assert_eq!(stored, [intent]);
}

#[test]
fn a_signer_joins_once_and_only_a_collecting_intent() {
    let net = &fixture()["networks"][0];
    let wallet = wallet(net);
    let intent = wallet.propose();
    let once = import(&wallet, &intent, added(net, 1)).unwrap();
    let twice = import(&wallet, &once, added(net, 1)).err().unwrap();
    assert_eq!(
        reason(twice),
        "this Argent multisig signer already signed the intent"
    );
    // A local signer that already signed by paste is refused too.
    let index = net["proposal"]["cosignerAccountIndexes"][0]
        .as_u64()
        .unwrap();
    let local = wallet
        .core
        .sign_multisig_intent(
            once.id.clone(),
            wallet.signer_id(index),
            wallet.current_state(&once),
        )
        .err()
        .unwrap();
    assert_eq!(
        reason(local),
        "this Argent multisig signer already signed the intent"
    );
    let ready = import(&wallet, &once, added(net, 2)).unwrap();
    assert_eq!(ready.state, IntentState::Ready);
    let more = import(&wallet, &ready, added(net, 1)).err().unwrap();
    assert_eq!(
        reason(more),
        "Argent multisig intent is not collecting signatures"
    );
    let unknown = wallet
        .core
        .import_multisig_signature(
            format!("{}0", intent.id),
            added(net, 1).0,
            added(net, 1).1,
            added(net, 1).2,
            wallet.current_state(&intent),
        )
        .err()
        .unwrap();
    assert_eq!(reason(unknown), "intent not found");
}

#[test]
fn a_moved_chain_or_a_lock_refuses_the_import() {
    let net = &fixture()["networks"][0];
    for moved in ["config", "nonce"] {
        let wallet = wallet(net);
        let intent = wallet.propose();
        let mut guids = wallet.guids();
        guids[0] = "0x1234".into();
        guids.sort();
        let state = match moved {
            "config" => wallet.intent_state(&intent.id, wallet.threshold(), &guids, &intent.nonce),
            _ => wallet.intent_state(&intent.id, wallet.threshold(), &wallet.guids(), "0x99"),
        };
        let (key, r, s) = added(net, 1);
        let refused = wallet
            .core
            .import_multisig_signature(intent.id.clone(), key, r, s, state)
            .err()
            .unwrap();
        let expected = match moved {
            "config" => "Argent multisig intent is stale after a configuration change",
            _ => "Argent multisig intent nonce is no longer current",
        };
        assert_eq!(reason(refused), expected);
        let stored = wallet.core.multisig_intents(wallet.multisig_id()).unwrap();
        assert!(stored[0].signed_public_keys.is_empty(), "{moved}");
    }
    let wallet = wallet(net);
    let intent = wallet.propose();
    let state = wallet.current_state(&intent);
    wallet.core.lock();
    let (key, r, s) = added(net, 1);
    let locked = wallet
        .core
        .import_multisig_signature(intent.id.clone(), key, r, s, state)
        .err()
        .unwrap();
    assert!(matches!(locked, CoreError::Locked), "{locked:?}");
    wallet.core.unlock(crate::common::PASSWORD.into()).unwrap();
    let stored = wallet.core.multisig_intents(wallet.multisig_id()).unwrap();
    assert_eq!(stored, [intent]);
}
