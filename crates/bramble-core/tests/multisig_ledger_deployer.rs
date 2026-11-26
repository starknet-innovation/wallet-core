//! P8: a Ledger account deploys an Argent multisig on its device, as the extension offers a
//! Ledger deployer (`multisig-deploy-view.ts`). The fixture multisig's deployer key is added as a
//! Ledger Argent account (same key, another address), which the multisig accepts by key.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

use crate::multisig_support;
use crate::multisig_tx_support;

use bramble_core::ledger::LedgerScheme;
use bramble_core::{AccountFamily, LedgerAccountRequest, SendStep, StepReply};
use multisig_support::fixture;
use multisig_tx_support::*;
use serde_json::Value;

#[test]
fn a_ledger_deployer_signs_the_deployment_on_its_device() {
    let net = fixture()["networks"][0].clone();
    let wallet = wallet(&net);
    let deploy = &net["deploy"];
    let index = deploy["deployerAccountIndex"].as_u64().unwrap();
    let signer = net["signers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|signer| signer["accountIndex"] == index)
        .unwrap();
    let ledger = wallet
        .core
        .add_ledger_account(LedgerAccountRequest {
            network: wallet.network,
            family: AccountFamily::Argent,
            scheme: LedgerScheme::Bramble,
            account_index: 0,
            public_key_x: signer["publicKey"].as_str().unwrap().into(),
        })
        .unwrap();
    assert!(ledger.ledger);
    let plan = wallet
        .core
        .clone()
        .prepare_multisig_deployment(wallet.multisig_id(), ledger.id.clone())
        .unwrap();
    let step = plan.advance(StepReply::Start).unwrap();
    let step = drive_deployment(&plan, step, &phase(&deploy["review"]));
    assert_eq!(step, SendStep::Review);
    let step = plan.advance(StepReply::Confirm).unwrap();
    // Every read before the write, then the device: never the host's key.
    let submit = phase(&deploy["submit"]);
    let write = submit
        .iter()
        .position(|exchange: &Value| exchange["method"] == "starknet_addDeployAccountTransaction")
        .unwrap();
    let step = drive_deployment(&plan, step, &submit[..write]);
    assert!(matches!(step, SendStep::Device { .. }), "{step:?}");
    assert_eq!(plan.signatures_made(), 0);
}
