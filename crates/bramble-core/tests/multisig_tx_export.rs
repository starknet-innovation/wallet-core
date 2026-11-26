//! P1's export, end to end through `WalletCore` over G5's `extension-multisig-transactions.json`
//! on Sepolia and Mainnet: the fixture multisig deployed by its deployer's vault record, the
//! extension's proposal frozen and stored under this wallet's own review, each co-signer signing
//! with its sealed key by record, the threshold signature submitted, and a second wallet
//! importing the proposal and the ready packages. Every request body must be the recorded one,
//! and every stored intent's package must be the bytes the extension's co-signers exchanged.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

use crate::multisig_support;
use crate::multisig_tx_support;

use bramble_core::{IntentState, MultisigProposalRequest, MultisigTxStep, SendStep, StepReply};
use multisig_support::fixture;
use multisig_tx_support::*;
use serde_json::{json, Value};

fn networks() -> Vec<Value> {
    let networks = fixture()["networks"].as_array().unwrap().clone();
    assert_eq!(networks.len(), 2);
    networks
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap()
}

/// The package the extension's co-signers exchange for a stored intent.
fn package_of(intent: &Value) -> String {
    format!(
        r#"{{"intent":{},"kind":"bramble.argent-multisig-intent","version":1}}"#,
        text(intent)
    )
}

#[test]
fn the_deployer_deploys_the_fixture_multisig() {
    for net in networks() {
        let wallet = wallet(&net);
        let deploy = &net["deploy"];
        let deployer = wallet.signer_id(deploy["deployerAccountIndex"].as_u64().unwrap());
        let plan = wallet
            .core
            .clone()
            .prepare_multisig_deployment(wallet.multisig_id(), deployer.clone())
            .unwrap();
        let step = plan.advance(StepReply::Start).unwrap();
        let step = drive_deployment(&plan, step, &phase(&deploy["review"]));
        assert_eq!(step, SendStep::Review);
        let review = plan.current_review().unwrap();
        assert!(review.can_confirm && !review.already_deployed, "{review:?}");
        assert_eq!(
            review.deployment_fee.unwrap().max_spend,
            text(&deploy["reviewedFee"]["overallFee"])
        );
        assert_eq!(plan.signatures_made(), 0, "the review signs nothing");
        let step = plan.advance(StepReply::Confirm).unwrap();
        let step = drive_deployment(&plan, step, &phase(&deploy["submit"]));
        let hash = text(&deploy["transactionHash"]);
        assert_eq!(step, SendStep::Broadcast { hash: hash.into() });
        assert_eq!(plan.signatures_made(), 1);
        // Recorded under the deployer, whose activity the wallet reads.
        let activity = wallet.core.activity(deployer).unwrap();
        assert_eq!(activity.len(), 1);
        assert_eq!(activity[0].hash, hash);
    }
}

#[test]
fn a_proposal_is_cosigned_and_submitted_as_the_extension_does() {
    for net in networks() {
        let wallet = wallet(&net);
        let proposal = &net["proposal"];
        let stages = proposal["stages"].as_array().unwrap();

        let intent = wallet.propose();
        assert_eq!(intent.package, text(&proposal["proposalPackage"]));
        assert_eq!(intent.package, package_of(&stages[0]["intent"]));
        assert_eq!(intent.transaction_hash, text(&proposal["transactionHash"]));
        assert_eq!(intent.state, IntentState::Collecting);
        assert_eq!(
            intent.max_fee.as_ref().unwrap().max_spend,
            text(&proposal["feeReview"]["overallFee"])
        );
        assert_eq!(
            wallet.core.multisig_intents(wallet.multisig_id()).unwrap(),
            std::slice::from_ref(&intent)
        );

        let mut intent = intent;
        let cosigners = proposal["cosignerAccountIndexes"].as_array().unwrap();
        for (stage, index) in stages[1..].iter().zip(cosigners) {
            intent = wallet.sign(&intent, index.as_u64().unwrap());
            assert_eq!(intent.package, package_of(&stage["intent"]));
        }
        assert_eq!(intent.state, IntentState::Ready);
        assert_eq!(intent.package, text(&proposal["readyPackage"]));

        let submission = wallet
            .core
            .clone()
            .submit_multisig_intent(intent.id.clone(), wallet.current_state(&intent))
            .unwrap();
        let step = submission.advance(StepReply::Start).unwrap();
        let step = drive(step, &phase(&proposal["submit"]), |reply| {
            submission.advance(reply).unwrap()
        });
        let hash = text(&proposal["transactionHash"]).to_string();
        assert_eq!(step, MultisigTxStep::Submitted { hash: hash.clone() });
        let stored = wallet.core.multisig_intents(wallet.multisig_id()).unwrap();
        assert_eq!(stored[0].state, IntentState::Submitted);
        assert_eq!(stored[0].broadcast_transaction_hash, Some(hash));
    }
}

#[test]
fn a_cosigner_imports_the_proposal_and_then_the_ready_package() {
    for net in networks() {
        let wallet = wallet(&net);
        let proposal = &net["proposal"];
        let package = text(&proposal["proposalPackage"]).to_string();
        let nonce: Value = serde_json::from_str(&package).unwrap();
        let nonce = text(&nonce["intent"]["prepared"]["nonce"]).to_string();
        let state = || wallet.state(wallet.threshold(), &wallet.guids(), &nonce);

        let imported = wallet
            .core
            .import_multisig_intent(package.clone(), state(), wallet.tokens())
            .unwrap();
        assert_eq!(imported.package, package);
        assert_eq!(imported.state, IntentState::Collecting);

        let ready = wallet
            .core
            .import_multisig_intent(
                text(&proposal["readyPackage"]).into(),
                state(),
                wallet.tokens(),
            )
            .unwrap();
        assert_eq!(ready.package, text(&proposal["readyPackage"]));
        assert_eq!(ready.state, IntentState::Ready);
        assert_eq!(
            wallet.core.multisig_intents(wallet.multisig_id()).unwrap(),
            [ready]
        );
    }
}

/// A signer change is proposed by name: `extension-multisig-transactions-signers.json`'s four
/// changes, each frozen as the self-call `argentMultisigConfigChangeCall` builds, reviewed as the
/// configuration it leaves, and prepared as the extension prepared it.
#[test]
fn signer_changes_are_proposed_as_the_extension_proposes_them() {
    let wallet = wallet(&networks()[0]);
    let path = format!(
        "{}/tests/fixtures/extension-multisig-transactions-signers.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let signers: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(signers["account"]["id"], wallet.net["account"]["id"]);
    let cases = signers["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 4);
    for case in cases {
        let name = text(&case["name"]);
        let change = serde_json::from_value(case["change"].clone()).unwrap();
        let proposal = wallet
            .core
            .clone()
            .propose_multisig_transaction(
                wallet.current_authority(),
                MultisigProposalRequest::ChangeSigners { change },
                wallet.tokens(),
                false,
            )
            .unwrap();
        let review = proposal.review();
        let expected = &case["review"]["config"];
        assert_eq!(review.summary, text(&case["review"]["summary"]), "{name}");
        assert_eq!(
            serde_json::to_value(&review.change).unwrap(),
            expected["change"]
        );
        let next = review.next_config.unwrap();
        assert_eq!(
            (json!(next.signer_public_keys), json!(next.threshold)),
            (
                expected["nextConfig"]["signerPublicKeys"].clone(),
                expected["nextConfig"]["threshold"].clone()
            ),
            "{name}"
        );
        let step = proposal.advance(StepReply::Start).unwrap();
        let step = drive(step, &phase(&case["prepare"]), |reply| {
            proposal.advance(reply).unwrap()
        });
        assert!(
            matches!(step, MultisigTxStep::Proposed { .. }),
            "{name}: {step:?}"
        );
        let intent = proposal.intent().unwrap();
        assert_eq!(
            serde_json::to_value(&intent.calls).unwrap(),
            json!([case["call"]]),
            "{name}"
        );
        let prepared: Value = serde_json::from_str(text(&case["prepared"])).unwrap();
        assert_eq!(
            intent.transaction_hash,
            text(&prepared["transactionHash"]),
            "{name}"
        );
        assert_eq!(intent.summary, review.summary, "{name}");
    }
    let stored = wallet.core.multisig_intents(wallet.multisig_id()).unwrap();
    assert_eq!(stored.len(), 4);
}
