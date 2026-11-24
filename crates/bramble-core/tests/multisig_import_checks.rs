//! `multisig_import::import_intent` end to end, over the ready intent of
//! `extension-multisig-transactions-refusals.json`: the chain's authority and nonce, the
//! wallet's own review in place of the remote summary, the record that must be the frozen
//! transaction's account, and the stored intent it merges into. The extension recorded no import
//! cases, so these hold the Rust to `importIntent` directly.

mod multisig_import_support;
mod multisig_support;

use multisig_import_support::*;
use multisig_support::*;
use serde_json::{json, Value};
use starknet_wallet_core::accounts::multisig_read::{
    MultisigAuthorityRead, MultisigAuthorityStatus,
};
use starknet_wallet_core::accounts::multisig_records::Record;
use starknet_wallet_core::send::multisig_import::ImportError;
use starknet_wallet_core::send::multisig_intent::{IntentState, SigningIntent};
use starknet_wallet_core::send::multisig_invoke::PreparedInvoke;
use starknet_wallet_core::send::multisig_review::CallReviewError;
use starknet_wallet_core::{Felt, NetworkId};

#[test]
fn an_import_is_stored_with_the_wallets_own_summary() {
    let ready = ready();
    let submitted = SigningIntent {
        broadcast_transaction_hash: Some(ready.prepared.transaction_hash.clone()),
        state: IntentState::Submitted,
        summary: "Receive 1000 STRK".into(),
        ..ready.clone()
    };
    let package = parse(&submitted.package().to_json()).unwrap();
    let stored = import_with(&package, &account(), None, chain(&ready)).unwrap();
    assert_eq!(
        stored, ready,
        "lifecycle derived locally, summary the wallet's review"
    );
    assert_eq!(stored.summary, "Send 1 units to 0x0000…0456");

    // Into a stored intent: merged, and the stored summary kept.
    let local = local(&ready);
    let merged = import_with(&lying(&ready), &account(), Some(&local), chain(&ready)).unwrap();
    assert_eq!(merged, ready);
}

#[test]
fn a_signer_change_is_reviewed_against_the_records_keys() {
    let signers: Value = serde_json::from_str(
        &std::fs::read_to_string(format!(
            "{}/tests/fixtures/extension-multisig-transactions-signers.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap(),
    )
    .unwrap();
    let case = &signers["cases"][0];
    let prepared: PreparedInvoke =
        serde_json::from_str(case["prepared"].as_str().unwrap()).unwrap();
    let ready = ready();
    let proposal = SigningIntent::proposed(
        &ready.account_id,
        ready.config_snapshot.clone(),
        prepared,
        "Pay nobody",
    )
    .unwrap();
    let package = parse(&sent(&proposal)).unwrap();
    let stored = import_with(&package, &account(), None, chain(&proposal)).unwrap();
    assert_eq!(stored.summary, case["review"]["summary"].as_str().unwrap());
    assert_eq!(stored.state, IntentState::Collecting);

    // A record without the keys behind the chain's GUIDs cannot review a signer change.
    let keyless = Record {
        current_config: None,
        deployment: None,
        ..account()
    };
    assert_eq!(
        import_with(&package, &keyless, None, chain(&proposal)),
        Err(ImportError::Review(CallReviewError::MissingCurrentConfig))
    );
    let mut moved = account();
    moved.current_config.as_mut().unwrap().threshold = 3;
    assert_eq!(
        import_with(&package, &moved, None, chain(&proposal)),
        Err(ImportError::Review(CallReviewError::MissingCurrentConfig)),
        "keys that no longer match the chain are not used"
    );
}

#[test]
fn an_import_the_chain_has_moved_past_is_refused() {
    let ready = ready();
    let package = lying(&ready);
    let other = Record {
        id: format!("{}0", account().id),
        ..account()
    };
    assert_eq!(
        import_with(&package, &other, None, chain(&ready)),
        Err(ImportError::UnknownAccount)
    );
    let (threshold, guids, nonce) = chain(&ready);
    for (threshold, guids) in [
        (threshold + 1, guids.clone()),
        (threshold, guids[1..].to_vec()),
    ] {
        assert_eq!(
            import_with(&package, &account(), None, (threshold, guids, nonce)),
            Err(ImportError::ConfigChanged)
        );
    }
    for moved in [nonce + Felt::ONE, nonce - Felt::ONE] {
        assert_eq!(
            import_with(
                &package,
                &account(),
                None,
                (threshold, guids.clone(), moved)
            ),
            Err(ImportError::NonceNotCurrent)
        );
    }
    assert_eq!(
        ImportError::NonceNotCurrent.to_string(),
        "Argent multisig intent nonce is no longer current"
    );
}

#[test]
fn an_import_the_store_would_refuse_is_refused() {
    let ready = ready();
    let package = lying(&ready);
    let mut other_transaction = local(&ready);
    other_transaction.prepared.nonce = "0x6".into();
    assert_eq!(
        import_with(
            &package,
            &account(),
            Some(&other_transaction),
            chain(&ready)
        ),
        Err(ImportError::ConflictingId)
    );
    let mut other_snapshot = local(&ready);
    other_snapshot.config_snapshot.threshold = 3;
    let broadcast = SigningIntent {
        broadcast_transaction_hash: Some(ready.prepared.transaction_hash.clone()),
        ..local(&ready)
    };
    for stored in [other_snapshot, broadcast] {
        assert_eq!(
            import_with(&package, &account(), Some(&stored), chain(&ready)),
            Err(ImportError::Conflict)
        );
    }
    // A package built without `parse` is still validated before it is stored.
    let mut reversed = ready.package();
    reversed.intent.signatures.reverse();
    assert_eq!(
        import_with(&reversed, &account(), None, chain(&ready)),
        Err(ImportError::Invalid)
    );
}

#[test]
fn an_import_needs_an_available_read_of_its_own_account() {
    let ready = ready();
    let package = lying(&ready);
    let (threshold, guids, nonce) = chain(&ready);

    // The node never answered: the read ended `NetworkUnavailable`.
    let mut lost =
        MultisigAuthorityRead::new(NetworkId::Sepolia, &account().address, &class()).unwrap();
    let request = lost.next_request().unwrap();
    lost.transport_failed(request.id);
    assert_eq!(
        lost.status(),
        Some(MultisigAuthorityStatus::NetworkUnavailable)
    );
    assert_eq!(
        import_stored(&package, &account(), None, &lost, &nonce),
        Err(ImportError::AuthorityUnavailable)
    );

    // An available read of another address, even one reporting the same authority.
    let elsewhere = read_of("0x456", threshold, &guids);
    assert_eq!(
        import_stored(&package, &account(), None, &elsewhere, &nonce),
        Err(ImportError::ReadForAnotherAccount)
    );
    // The same address read on another network (refused before its status is looked at).
    let mainnet =
        MultisigAuthorityRead::new(NetworkId::Mainnet, &account().address, &class()).unwrap();
    assert_eq!(
        import_stored(&package, &account(), None, &mainnet, &nonce),
        Err(ImportError::ReadForAnotherAccount)
    );
}

#[test]
fn every_read_end_but_available_is_authority_unavailable() {
    let ready = ready();
    let package = lying(&ready);
    let (_, _, nonce) = chain(&ready);
    let ended = |class_hash: Value, threshold: Option<Value>| {
        let mut read =
            MultisigAuthorityRead::new(NetworkId::Sepolia, &account().address, &class()).unwrap();
        respond(&mut read, json!("0x534e5f5345504f4c4941"));
        respond(&mut read, json!({"block_hash": "0x1", "block_number": 1}));
        let request = read.next_request().unwrap();
        let body = match class_hash {
            Value::Null => json!({"jsonrpc": "2.0", "id": request.id,
                "error": {"code": 20, "message": "Contract not found"}}),
            hash => json!({"jsonrpc": "2.0", "id": request.id, "result": hash}),
        };
        read.accept_response(request.id, &body.to_string()).unwrap();
        if let Some(threshold) = threshold {
            respond(&mut read, threshold);
        }
        read
    };
    for (read, status) in [
        (
            ended(Value::Null, None),
            MultisigAuthorityStatus::Undeployed,
        ),
        (
            ended(json!("0x1"), None),
            MultisigAuthorityStatus::UnsupportedClass,
        ),
        (
            ended(json!(class().class_hash()), Some(json!([]))),
            MultisigAuthorityStatus::InvalidConfig,
        ),
    ] {
        assert_eq!(read.status(), Some(status));
        assert_eq!(
            import_stored(&package, &account(), None, &read, &nonce),
            Err(ImportError::AuthorityUnavailable),
            "{status:?}"
        );
    }
}

#[test]
fn the_record_must_be_the_frozen_transactions_account() {
    // Ids alone agree: the record's address or network is another account's, which the review
    // and the read would otherwise use in place of the transaction's sender.
    let ready = ready();
    let package = lying(&ready);
    let elsewhere = Record {
        address: "0x456".into(),
        ..account()
    };
    let mainnet = Record {
        network_id: "SN_MAIN".into(),
        ..account()
    };
    for record in [elsewhere, mainnet] {
        assert_eq!(
            import_with(&package, &record, None, chain(&ready)),
            Err(ImportError::UnknownAccount),
            "{record:?}"
        );
    }
}

#[test]
fn an_import_into_a_finished_intent_returns_it_unchanged() {
    let ready = ready();
    for state in [IntentState::Submitted, IntentState::Accepted] {
        let stored = SigningIntent {
            broadcast_transaction_hash: Some(ready.prepared.transaction_hash.clone()),
            state,
            ..ready.clone()
        };
        assert_eq!(
            import_with(&lying(&ready), &account(), Some(&stored), chain(&ready)),
            Ok(stored.clone()),
            "{state:?}"
        );
    }
}
