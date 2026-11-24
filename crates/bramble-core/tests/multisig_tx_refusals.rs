//! What P1's export refuses, each through `WalletCore` on the Sepolia fixture multisig: a stale
//! or unavailable chain read, a signer outside the snapshot or signing twice, a lock before
//! anything is stored or sent, an intent in the wrong state, a deployer outside the constructor,
//! and a co-signer's package that lies about its calls, names an unsaved multisig or outgrows a
//! Nostr message. A refused operation stores nothing it should not.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

mod common;
mod multisig_support;
mod multisig_tx_support;

use bramble_core::{
    ConfigChange, CoreError, IntentState, MultisigProposalRequest, MultisigTxFailure,
    MultisigTxStep, StepReply,
};
use multisig_support::fixture;
use multisig_tx_support::*;
use serde_json::Value;

fn sepolia() -> Wallet {
    wallet(&fixture()["networks"][0])
}

fn reason(error: CoreError) -> String {
    match error {
        CoreError::InvalidInput { reason } => reason,
        other => panic!("{other:?}"),
    }
}

/// The chain's signers with the first one swapped for another key's GUID.
fn other_guids(wallet: &Wallet) -> Vec<String> {
    let mut guids = wallet.guids();
    guids[0] = "0x1234".into();
    guids.sort();
    guids
}

#[test]
fn a_proposal_needs_the_stored_signers_on_chain() {
    let wallet = sepolia();
    let changed = wallet.authority(wallet.threshold(), &other_guids(&wallet));
    let refused = wallet
        .core
        .clone()
        .propose_multisig_transaction(changed, wallet.transfer(), wallet.tokens(), false)
        .err()
        .unwrap();
    assert_eq!(
        reason(refused),
        "Argent multisig signer configuration changed on-chain"
    );
    let unread =
        bramble_core::MultisigAuthorityRead::new(wallet.network, wallet.address()).unwrap();
    let request = unread.next_request().unwrap();
    unread.transport_failed(request.id);
    let refused = wallet
        .core
        .clone()
        .propose_multisig_transaction(unread, wallet.transfer(), wallet.tokens(), false)
        .err()
        .unwrap();
    assert_eq!(reason(refused), "Argent multisig config unavailable");
}

#[test]
fn a_proposal_holds_a_signer_change_to_the_review() {
    let wallet = sepolia();
    let change = ConfigChange::ChangeThreshold {
        threshold: wallet.threshold(),
    };
    let refused = wallet
        .core
        .clone()
        .propose_multisig_transaction(
            wallet.current_authority(),
            MultisigProposalRequest::ChangeSigners { change },
            wallet.tokens(),
            false,
        )
        .err()
        .unwrap();
    assert_eq!(reason(refused), "Argent multisig threshold is unchanged");
}

/// A transfer request is built in the core, so each of its parts is checked there: a token the
/// wallet does not know, a zero or malformed recipient, and an amount the extension refuses.
#[test]
fn a_transfer_proposal_refuses_what_the_extension_refuses() {
    let wallet = sepolia();
    let MultisigProposalRequest::Transfer {
        token_id,
        recipient,
        amount,
    } = wallet.transfer()
    else {
        unreachable!()
    };
    let propose = |token: &str, to: &str, value: &str| {
        let request = MultisigProposalRequest::Transfer {
            token_id: token.into(),
            recipient: to.into(),
            amount: value.into(),
        };
        reason(
            wallet
                .core
                .clone()
                .propose_multisig_transaction(
                    wallet.current_authority(),
                    request,
                    wallet.tokens(),
                    false,
                )
                .err()
                .unwrap(),
        )
    };
    assert_eq!(propose("0x1234", &recipient, &amount), "unknown token");
    // Ids are per network: Mainnet's STRK is no token on Sepolia.
    let mainnet = wallet.tokens().tracked(bramble_core::NetworkId::Mainnet);
    assert_eq!(
        propose(&mainnet[0].token.id, &recipient, &amount),
        "unknown token"
    );
    assert_eq!(
        propose(&token_id, "0x0", &amount),
        "Invalid transfer address"
    );
    assert_eq!(
        propose(&token_id, "0xnope", &amount),
        "Invalid transfer address"
    );
    assert_eq!(
        propose(&token_id, &recipient, "0"),
        "Transfer amount must be greater than zero"
    );
    assert_eq!(
        propose(&token_id, &recipient, "0.0000000000000000001"),
        "Too many decimal places"
    );
    assert_eq!(
        propose(&token_id, &recipient, "1e18"),
        "Invalid transfer amount"
    );
    assert!(wallet
        .core
        .multisig_intents(wallet.multisig_id())
        .unwrap()
        .is_empty());
}

#[test]
fn a_lock_before_the_estimate_returns_stores_nothing() {
    let wallet = sepolia();
    let proposal = wallet
        .core
        .clone()
        .propose_multisig_transaction(
            wallet.current_authority(),
            wallet.transfer(),
            wallet.tokens(),
            false,
        )
        .unwrap();
    let exchanges = phase(&wallet.net["proposal"]["prepare"]);
    let mut relocked = false;
    let step = proposal.advance(StepReply::Start).unwrap();
    let step = drive(step, &exchanges, |reply| {
        if !relocked {
            wallet.relock();
            relocked = true;
        }
        proposal.advance(reply).unwrap()
    });
    assert_eq!(
        step,
        MultisigTxStep::Failed {
            failure: MultisigTxFailure::Locked
        }
    );
    assert!(wallet
        .core
        .multisig_intents(wallet.multisig_id())
        .unwrap()
        .is_empty());
}

#[test]
fn only_a_snapshot_member_signs_and_only_once() {
    let wallet = sepolia();
    let intent = wallet.propose();
    let outsider = wallet.outsider();
    let refused = wallet
        .core
        .sign_multisig_intent(intent.id.clone(), outsider, wallet.current_state(&intent))
        .err()
        .unwrap();
    assert_eq!(
        reason(refused),
        "selected account is not a current Argent multisig signer"
    );

    let first = wallet.net["proposal"]["cosignerAccountIndexes"][0]
        .as_u64()
        .unwrap();
    let signed = wallet.sign(&intent, first);
    assert_eq!(signed.signed_public_keys.len(), 1);
    let again = wallet
        .core
        .sign_multisig_intent(
            intent.id.clone(),
            wallet.signer_id(first),
            wallet.current_state(&intent),
        )
        .err()
        .unwrap();
    assert_eq!(
        reason(again),
        "this Argent multisig signer already signed the intent"
    );
}

#[test]
fn a_moved_chain_refuses_signing_and_leaves_the_intent_stale() {
    for moved in ["config", "nonce"] {
        let wallet = sepolia();
        let intent = wallet.propose();
        let state = match moved {
            "config" => wallet.intent_state(
                &intent.id,
                wallet.threshold(),
                &other_guids(&wallet),
                &intent.nonce,
            ),
            _ => wallet.intent_state(&intent.id, wallet.threshold(), &wallet.guids(), "0x99"),
        };
        let signer = wallet.signer_id(
            wallet.net["proposal"]["cosignerAccountIndexes"][0]
                .as_u64()
                .unwrap(),
        );
        let refused = wallet
            .core
            .sign_multisig_intent(intent.id.clone(), signer.clone(), state)
            .err()
            .unwrap();
        let expected = match moved {
            "config" => "Argent multisig intent is stale after a configuration change",
            _ => "Argent multisig intent nonce is no longer current",
        };
        assert_eq!(reason(refused), expected);
        let stored = wallet.core.multisig_intents(wallet.multisig_id()).unwrap();
        assert_eq!(stored[0].state, IntentState::Stale, "{moved}");
        assert!(stored[0].signed_public_keys.is_empty());
        let after = wallet
            .core
            .sign_multisig_intent(intent.id.clone(), signer, wallet.current_state(&intent))
            .err()
            .unwrap();
        assert_eq!(
            reason(after),
            "Argent multisig intent is not collecting signatures"
        );
    }
}

#[test]
fn a_locked_wallet_signs_nothing() {
    let wallet = sepolia();
    let intent = wallet.propose();
    let state = wallet.current_state(&intent);
    let signer = wallet.signer_id(0);
    wallet.core.lock();
    let refused = wallet
        .core
        .sign_multisig_intent(intent.id.clone(), signer, state)
        .err()
        .unwrap();
    assert!(matches!(refused, CoreError::Locked), "{refused:?}");
}

#[test]
fn only_a_ready_unsent_intent_is_submitted() {
    let wallet = sepolia();
    let intent = wallet.propose();
    let refused = wallet
        .core
        .clone()
        .submit_multisig_intent(intent.id.clone(), wallet.current_state(&intent))
        .err()
        .unwrap();
    assert_eq!(reason(refused), "Argent multisig intent is not ready");

    let ready = wallet.ready();
    let stale = wallet
        .core
        .clone()
        .submit_multisig_intent(
            ready.id.clone(),
            wallet.intent_state(&ready.id, wallet.threshold(), &wallet.guids(), "0x99"),
        )
        .err()
        .unwrap();
    assert_eq!(
        reason(stale),
        "Argent multisig intent nonce is no longer current"
    );
}

#[test]
fn a_submission_stops_at_a_lock_before_the_write() {
    let wallet = sepolia();
    let ready = wallet.ready();
    let submission = wallet
        .core
        .clone()
        .submit_multisig_intent(ready.id.clone(), wallet.current_state(&ready))
        .unwrap();
    let step = submission.advance(StepReply::Start).unwrap();
    wallet.relock();
    let exchanges = phase(&wallet.net["proposal"]["submit"]);
    let step = drive(step, &exchanges[..1], |reply| {
        submission.advance(reply).unwrap()
    });
    assert_eq!(
        step,
        MultisigTxStep::Failed {
            failure: MultisigTxFailure::Locked
        }
    );
    let stored = wallet.core.multisig_intents(wallet.multisig_id()).unwrap();
    assert_eq!(stored[0].state, IntentState::Ready);
}

#[test]
fn a_submitted_intent_is_never_sent_again() {
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

#[test]
fn only_a_constructor_member_deploys() {
    let wallet = sepolia();
    let outsider = wallet.outsider();
    let refused = wallet
        .core
        .clone()
        .prepare_multisig_deployment(wallet.multisig_id(), outsider)
        .err()
        .unwrap();
    assert_eq!(
        reason(refused),
        "selected signer is not a member of the reviewed constructor"
    );
}

/// The ready package with its summary rewritten: the import keeps this wallet's own review.
#[test]
fn an_import_replaces_a_lying_summary_with_the_wallets_review() {
    let wallet = sepolia();
    let proposal = &wallet.net["proposal"];
    let mut package: Value =
        serde_json::from_str(proposal["readyPackage"].as_str().unwrap()).unwrap();
    let honest = package["intent"]["summary"].clone();
    package["intent"]["summary"] = "Receive 1000 STRK".into();
    let nonce = package["intent"]["prepared"]["nonce"].as_str().unwrap();
    let state = wallet.state(wallet.threshold(), &wallet.guids(), nonce);
    let imported = wallet
        .core
        .import_multisig_intent(package.to_string(), state, wallet.tokens())
        .unwrap();
    assert_eq!(imported.summary, honest.as_str().unwrap());
    assert_eq!(imported.package, proposal["readyPackage"].as_str().unwrap());
}

#[test]
fn an_import_needs_the_chain_a_saved_multisig_and_a_bounded_package() {
    let wallet = sepolia();
    let package = wallet.net["proposal"]["proposalPackage"]
        .as_str()
        .unwrap()
        .to_string();
    let parsed: Value = serde_json::from_str(&package).unwrap();
    let nonce = parsed["intent"]["prepared"]["nonce"].as_str().unwrap();

    let moved = wallet.state(wallet.threshold(), &wallet.guids(), "0x99");
    let refused = wallet
        .core
        .import_multisig_intent(package.clone(), moved, wallet.tokens())
        .err()
        .unwrap();
    assert_eq!(
        reason(refused),
        "Argent multisig intent nonce is no longer current"
    );

    let oversized = format!("{package}{}", " ".repeat(32_768));
    let state = wallet.state(wallet.threshold(), &wallet.guids(), nonce);
    let refused = wallet
        .core
        .import_multisig_intent(oversized, state.clone(), wallet.tokens())
        .err()
        .unwrap();
    assert_eq!(reason(refused), "invalid Argent multisig intent package");

    wallet.core.remove_multisig(wallet.multisig_id()).unwrap();
    let refused = wallet
        .core
        .import_multisig_intent(package, state, wallet.tokens())
        .err()
        .unwrap();
    assert_eq!(reason(refused), "Argent multisig not found");
    assert!(wallet
        .core
        .multisig_intents(wallet.multisig_id())
        .unwrap()
        .is_empty());
}

/// The state read takes its nonce only as the answer to its own request, and a nonce it could
/// not read leaves the read unavailable, so no operation proceeds on it.
#[test]
fn a_state_read_judges_only_its_own_nonce() {
    let wallet = sepolia();
    let read = bramble_core::MultisigStateRead::new(wallet.network, wallet.address()).unwrap();
    for result in wallet.authority_replies(wallet.threshold(), &wallet.guids()) {
        let request = read.next_request().unwrap();
        read.accept_response(request.id, body(request.id, result))
            .unwrap();
    }
    let nonce = read.next_request().unwrap();
    assert!(read
        .accept_response(nonce.id + 1, body(nonce.id + 1, "0x0".into()))
        .is_err());
    assert_eq!(read.status(), None, "still waiting for its nonce");
    read.transport_failed(nonce.id);
    assert_eq!(
        read.status(),
        Some(bramble_core::MultisigAuthorityStatus::NetworkUnavailable)
    );
    let intent = wallet.propose();
    let refused = wallet
        .core
        .sign_multisig_intent(intent.id, wallet.signer_id(0), read)
        .err()
        .unwrap();
    assert_eq!(reason(refused), "Argent multisig config unavailable");
}
