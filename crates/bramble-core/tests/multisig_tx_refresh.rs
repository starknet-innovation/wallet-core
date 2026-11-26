//! "Update network fee" on a stale intent (`openUpdatedIntent`), through `WalletCore` on the
//! Sepolia fixture multisig: the stale intent's own stored calls are proposed again by its id
//! (`MultisigProposalRequest::Repropose`), under the same resend guard as any proposal, and the
//! version that leaves is the one `multisig_fee_replacement` then opens.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

mod common;
mod multisig_support;
mod multisig_tx_support;

use bramble_core::{
    CoreError, IntentState, MultisigProposalRequest, MultisigTxFailure, MultisigTxStep,
};
use multisig_support::{decode, fixture};
use multisig_tx_support::*;

fn reason(error: CoreError) -> String {
    match error {
        CoreError::InvalidInput { reason } => reason,
        other => panic!("{other:?}"),
    }
}

fn repropose(intent_id: &str) -> MultisigProposalRequest {
    MultisigProposalRequest::Repropose {
        intent_id: intent_id.into(),
    }
}

#[test]
fn a_stale_intent_is_proposed_again_by_id_and_then_opened() {
    let wallet = wallet(&fixture()["networks"][0]);
    let ready = wallet.ready();
    // Not stale yet: nothing to propose again, and no replacement to open.
    let refusal = wallet
        .core
        .clone()
        .propose_multisig_transaction(
            wallet.current_authority(),
            repropose(&ready.id),
            wallet.tokens(),
            false,
        )
        .err()
        .unwrap();
    assert_eq!(reason(refusal), "That transaction is unavailable");
    assert_eq!(
        wallet
            .core
            .multisig_fee_replacement(ready.id.clone())
            .unwrap(),
        None
    );

    // A co-signer sent its own version: this wallet's submit finds the nonce moved, not yet the
    // broadcast, and leaves the intent stale.
    let moved = wallet.intent_state_with(
        &ready.id,
        wallet.threshold(),
        &wallet.guids(),
        "0x6",
        &|_| not_found(),
    );
    assert!(wallet
        .core
        .clone()
        .submit_multisig_intent(ready.id.clone(), moved)
        .is_err());

    // The resend guard holds for the stale intent's own calls as for any proposal.
    let step = propose_at(&wallet, repropose(&ready.id), "0x6", not_found(), false);
    assert_eq!(
        step,
        MultisigTxStep::Failed {
            failure: MultisigTxFailure::ResendUnconfirmed
        }
    );
    let MultisigTxStep::Proposed { intent_id } =
        propose_at(&wallet, repropose(&ready.id), "0x6", not_found(), true)
    else {
        panic!("not proposed")
    };
    let intents = wallet.core.multisig_intents(wallet.multisig_id()).unwrap();
    let stale = intents.iter().find(|intent| intent.id == ready.id).unwrap();
    let next = intents
        .iter()
        .find(|intent| intent.id == intent_id)
        .unwrap();
    assert_eq!(stale.state, IntentState::Stale);
    assert_eq!(next.state, IntentState::Collecting);
    assert_eq!(next.calls, stale.calls);
    assert_eq!(next.summary, stale.summary);
    assert_ne!(next.nonce, stale.nonce);
    // `feeReplacementIntentFor` asks for the stale intent's nonce, so a version at the moved
    // nonce is a new proposal, not its fee replacement.
    assert_eq!(
        wallet
            .core
            .multisig_fee_replacement(ready.id.clone())
            .unwrap(),
        None
    );
}

#[test]
fn an_unknown_intent_has_no_replacement() {
    let wallet = wallet(&fixture()["networks"][0]);
    let refusal = wallet
        .core
        .multisig_fee_replacement("nope".into())
        .err()
        .unwrap();
    assert_eq!(reason(refusal), "That transaction is unavailable");
}

/// The card's case: the node refused the fee at the intent's own nonce, so "Update network
/// fee" proposes its calls at that nonce again, and the new version is the replacement the card
/// opens from then on ("Continue").
#[test]
fn a_fee_stale_intent_is_replaced_at_its_own_nonce() {
    let mut wallet = wallet(&fixture()["networks"][0]);
    let ready = wallet.ready();
    let step = fee_refused(&wallet, &ready, Some(&ready.nonce));
    assert_eq!(
        step,
        MultisigTxStep::Failed {
            failure: MultisigTxFailure::FeeExpired
        }
    );
    // The node quotes the fee again, higher: a new transaction at the same nonce.
    for line in wallet.net["proposal"]["prepare"].as_array_mut().unwrap() {
        let mut exchange = decode(line);
        if exchange["method"] == "starknet_estimateFee" {
            exchange["reply"]["result"][0]["l2_gas_price"] = "0x165a0bc00".into();
            *line = exchange.to_string().into();
        }
    }
    let MultisigTxStep::Proposed { intent_id } = propose_at(
        &wallet,
        repropose(&ready.id),
        &ready.nonce,
        not_found(),
        false,
    ) else {
        panic!("not proposed")
    };
    let replacement = wallet
        .core
        .multisig_fee_replacement(ready.id.clone())
        .unwrap()
        .unwrap();
    assert_eq!(replacement.id, intent_id);
    assert_eq!(replacement.calls, ready.calls);
    assert_eq!(replacement.nonce, ready.nonce);
}
