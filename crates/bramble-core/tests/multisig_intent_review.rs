//! The intent card's review (`intentCard`) through `WalletCore::multisig_intent_review`, on the
//! Sepolia fixture multisig of `extension-multisig-transactions.json`: each frozen call's
//! `verifyCalls` line is the summary the extension recorded for it, a signer change pins the
//! multisig (`knownContracts`) so its self-call raises no `unknown-contract`, and
//! `configMatchesIntent` holds only while the chain's threshold and GUIDs are the snapshot.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

use crate::multisig_support;
use crate::multisig_tx_support;

use bramble_core::{CallKind, CoreError, MultisigProposalRequest, MultisigTxStep, StepReply};
use multisig_support::fixture;
use multisig_tx_support::*;
use serde_json::Value;

fn sepolia() -> Wallet {
    wallet(&fixture()["networks"][0])
}

#[test]
fn a_transfer_reads_as_the_extension_summarised_it() {
    let wallet = sepolia();
    let intent = wallet.propose();
    let package: Value =
        serde_json::from_str(wallet.net["proposal"]["proposalPackage"].as_str().unwrap()).unwrap();
    assert_eq!(intent.sender, wallet.address());
    let review = wallet
        .core
        .multisig_intent_review(
            intent.id.clone(),
            wallet.tokens(),
            wallet.current_authority(),
        )
        .unwrap();
    assert_eq!(review.calls.len(), 1);
    let call = &review.calls[0];
    assert_eq!(call.kind, CallKind::Transfer);
    assert_eq!(call.summary, package["intent"]["summary"].as_str().unwrap());
    assert_eq!(call.contract_address, intent.calls[0].contract_address);
    assert!(review.risk_flags.is_empty(), "{:?}", review.risk_flags);
    assert!(review.has_fresh_authority);
}

#[test]
fn a_changed_signer_set_is_not_fresh_authority() {
    let wallet = sepolia();
    let intent = wallet.propose();
    let guids = wallet.guids();
    for read in [
        wallet.authority(wallet.threshold() + 1, &guids),
        wallet.authority(1, &guids[..2]),
    ] {
        let review = wallet
            .core
            .multisig_intent_review(intent.id.clone(), wallet.tokens(), read)
            .unwrap();
        assert!(!review.has_fresh_authority);
    }
}

#[test]
fn a_read_of_another_account_is_refused() {
    let wallet = sepolia();
    let intent = wallet.propose();
    let other = bramble_core::MultisigAuthorityRead::new(wallet.network, "0x1234".into()).unwrap();
    match wallet
        .core
        .multisig_intent_review(intent.id, wallet.tokens(), other)
    {
        Err(CoreError::InvalidInput { reason }) => {
            assert_eq!(
                reason,
                "Argent multisig authority read is for another account"
            )
        }
        other => panic!("{other:?}"),
    }
}

/// `configReviewFor` holds, so the card pins the multisig: the self-call is no unknown contract.
#[test]
fn a_signer_change_pins_the_multisig() {
    let wallet = sepolia();
    let path = format!(
        "{}/tests/fixtures/extension-multisig-transactions-signers.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let signers: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let case = &signers["cases"][0];
    let proposal = wallet
        .core
        .clone()
        .propose_multisig_transaction(
            wallet.current_authority(),
            MultisigProposalRequest::ChangeSigners {
                change: serde_json::from_value(case["change"].clone()).unwrap(),
            },
            wallet.tokens(),
            false,
        )
        .unwrap();
    let step = proposal.advance(StepReply::Start).unwrap();
    let MultisigTxStep::Proposed { intent_id } = drive(step, &phase(&case["prepare"]), |reply| {
        proposal.advance(reply).unwrap()
    }) else {
        panic!("not proposed");
    };
    let review = wallet
        .core
        .multisig_intent_review(intent_id, wallet.tokens(), wallet.current_authority())
        .unwrap();
    assert_eq!(review.calls.len(), 1);
    assert_eq!(review.calls[0].contract_address, wallet.address());
    assert!(review.risk_flags.is_empty(), "{:?}", review.risk_flags);
    assert!(review.has_fresh_authority);
}
