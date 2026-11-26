//! "Update network fee" on a stale intent (`openUpdatedIntent`), through `WalletCore` on the
//! Sepolia fixture multisig: the stale intent's own stored calls are proposed again by its id
//! (`MultisigProposalRequest::Repropose`), under the same resend guard as any proposal, and the
//! version that leaves is the one `multisig_fee_replacement` then opens.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

use crate::multisig_support;
use crate::multisig_tx_support;

use bramble_core::{
    ArgentMultisigConfig, CoreError, IntentState, MultisigAuthorityRead, MultisigIntent,
    MultisigProposalRequest, MultisigTxFailure, MultisigTxStep, StepReply,
};
use multisig_support::{decode, fixture};
use multisig_tx_support::*;
use serde_json::Value;

fn reason(error: CoreError) -> String {
    match error {
        CoreError::InvalidInput { reason } => reason,
        other => panic!("{other:?}"),
    }
}

/// Answers the proposal's fee estimate with `price` per L2 gas from now on.
fn quote_l2_gas_price(wallet: &mut Wallet, price: &str) {
    for line in wallet.net["proposal"]["prepare"].as_array_mut().unwrap() {
        let mut exchange = decode(line);
        if exchange["method"] == "starknet_estimateFee" {
            exchange["reply"]["result"][0]["l2_gas_price"] = price.into();
            *line = exchange.to_string().into();
        }
    }
}

/// The fixture's ready intent, left stale by a submit after a co-signer's version moved the
/// nonce to `0x6`, the node answering `status` for its hash.
fn stale_after_cosigner(wallet: &Wallet, status: Value) -> MultisigIntent {
    let ready = wallet.ready();
    let moved = wallet.intent_state_with(
        &ready.id,
        wallet.threshold(),
        &wallet.guids(),
        "0x6",
        &|_| status.clone(),
    );
    assert!(wallet
        .core
        .clone()
        .submit_multisig_intent(ready.id.clone(), moved)
        .is_err());
    ready
}

fn stored(wallet: &Wallet, id: &str) -> MultisigIntent {
    wallet
        .core
        .multisig_intents(wallet.multisig_id())
        .unwrap()
        .into_iter()
        .find(|intent| intent.id == id)
        .unwrap()
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
    quote_l2_gas_price(&mut wallet, "0x165a0bc00");
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

/// Found on chain by the time it is proposed again, the stale version refuses its calls
/// (`AlreadySent`) and is filed as submitted.
#[test]
fn a_stale_intent_the_chain_holds_is_not_proposed_again() {
    let wallet = wallet(&fixture()["networks"][0]);
    let stale = stale_after_cosigner(&wallet, not_found());
    let step = propose_at(
        &wallet,
        repropose(&stale.id),
        "0x6",
        status("ACCEPTED_ON_L2", "SUCCEEDED"),
        true,
    );
    assert_eq!(
        step,
        MultisigTxStep::Failed {
            failure: MultisigTxFailure::AlreadySent
        }
    );
    assert_eq!(stored(&wallet, &stale.id).state, IntentState::Submitted);
}

/// "Propose again": a stale version the chain reverted spent its nonce and moved nothing, so
/// its calls go out again at the next nonce without asking.
#[test]
fn a_reverted_stale_intent_is_proposed_again_without_confirming() {
    let wallet = wallet(&fixture()["networks"][0]);
    let stale = stale_after_cosigner(&wallet, status("ACCEPTED_ON_L2", "REVERTED"));
    assert_eq!(stored(&wallet, &stale.id).state, IntentState::Stale);
    let step = propose_at(&wallet, repropose(&stale.id), "0x6", not_found(), false);
    let MultisigTxStep::Proposed { intent_id } = step else {
        panic!("{step:?}")
    };
    assert_eq!(stored(&wallet, &intent_id).calls, stale.calls);
}

/// The calls are reviewed again against the configuration the chain holds now: a stale
/// threshold change the new signer set cannot meet is refused, never frozen.
#[test]
fn a_stale_intents_calls_are_reviewed_again() {
    let wallet = wallet(&fixture()["networks"][0]);
    let path = format!(
        "{}/tests/fixtures/extension-multisig-transactions-signers.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let signers: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let case = &signers["cases"][1];
    assert_eq!(case["change"]["kind"], "change_threshold");
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
    // The chain now holds two signers at threshold one: signing finds it and leaves the intent
    // stale, and the multisig is joined again at that configuration.
    let guids = wallet.guids();
    let nonce = stored(&wallet, &intent_id).nonce;
    let moved = wallet.intent_state(&intent_id, 1, &guids[..2], &nonce);
    assert!(wallet
        .core
        .sign_multisig_intent(intent_id.clone(), wallet.signer_id(0), moved)
        .is_err());
    assert_eq!(stored(&wallet, &intent_id).state, IntentState::Stale);
    wallet
        .core
        .add_multisig(wallet.authority(1, &guids[..2]))
        .unwrap();
    let refusal = wallet
        .core
        .clone()
        .propose_multisig_transaction(
            wallet.authority(1, &guids[..2]),
            repropose(&intent_id),
            wallet.tokens(),
            true,
        )
        .err()
        .unwrap();
    assert_eq!(
        reason(refusal),
        "Invalid Argent multisig threshold or signer count"
    );
}

/// Another multisig's stale intent is not this multisig's to propose.
#[test]
fn another_multisigs_stale_intent_is_refused() {
    let wallet = wallet(&fixture()["networks"][0]);
    let stale = stale_after_cosigner(&wallet, not_found());
    let input = &wallet.net["deploy"]["input"];
    let keys: Vec<String> = input["signerPublicKeys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|key| key.as_str().unwrap().to_string())
        .collect();
    let threshold = input["threshold"].as_u64().unwrap() as u32;
    let other = wallet
        .core
        .save_multisig_setup(
            wallet.network,
            None,
            ArgentMultisigConfig {
                threshold,
                signer_public_keys: keys,
            },
            "0x1234".into(),
            wallet.signer_id(0),
        )
        .unwrap();
    let read = MultisigAuthorityRead::new(wallet.network, other.address.clone()).unwrap();
    for result in wallet.authority_replies(threshold, &wallet.guids()) {
        let request = read.next_request().unwrap();
        read.accept_response(request.id, body(request.id, result))
            .unwrap();
    }
    let refusal = wallet
        .core
        .clone()
        .propose_multisig_transaction(read, repropose(&stale.id), wallet.tokens(), true)
        .err()
        .unwrap();
    assert_eq!(reason(refusal), "That transaction is unavailable");
}

/// `feeReplacementIntentFor`: of the open versions at the stale nonce, the one with the most
/// signatures, the later of equals; proposing again while one exists stores another version.
#[test]
fn the_replacement_is_the_most_signed_open_version() {
    let mut wallet = wallet(&fixture()["networks"][0]);
    let ready = wallet.ready();
    fee_refused(&wallet, &ready, Some(&ready.nonce));
    let mut versions = Vec::new();
    for price in ["0x165a0bc00", "0x218711a00"] {
        quote_l2_gas_price(&mut wallet, price);
        let step = propose_at(
            &wallet,
            repropose(&ready.id),
            &ready.nonce,
            not_found(),
            false,
        );
        let MultisigTxStep::Proposed { intent_id } = step else {
            panic!("{step:?}")
        };
        versions.push(intent_id);
    }
    let replacement = |wallet: &Wallet| {
        wallet
            .core
            .multisig_fee_replacement(ready.id.clone())
            .unwrap()
            .unwrap()
            .id
    };
    assert_eq!(replacement(&wallet), versions[1], "the later of equals");
    wallet.sign(&stored(&wallet, &versions[0]), 0);
    assert_eq!(replacement(&wallet), versions[0], "the most signed");
    // A version is no replacement of itself, nor of an intent that is not stale.
    assert_eq!(
        wallet
            .core
            .multisig_fee_replacement(versions[0].clone())
            .unwrap(),
        None
    );
}
