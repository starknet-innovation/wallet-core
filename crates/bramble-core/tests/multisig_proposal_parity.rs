//! An Argent multisig proposal, co-signed and submitted, must be what the extension makes of it.
//! `extension-multisig-transactions.json` (G5) records, on Sepolia and Mainnet, the extension's
//! `prepareArgentMultisigInvokeV3` against a canned node, the collecting intent and its package,
//! two co-signers merged in turn (each stage checked by the stored-intent validator), the
//! threshold signature, and `submitPreparedArgentMultisigInvokeV3`. This drives
//! `starknet_wallet_core::send::multisig_invoke_plan` with the recorded replies and requires the
//! same request bodies in the same order, and rebuilds every intent, package and signature
//! from Bramble's pinned class, `MULTISIG_FEE_OVERHEAD` and the phrase's own keys.
//!
//! Request bodies are compared as JSON values: the capture keeps starknet.js's key order, and
//! `rpc_bodies` writes keys sorted, as every send-path parity test compares them. Intents and
//! packages are compared as exact strings.

use crate::multisig_support;

use bramble_core::MULTISIG_FEE_OVERHEAD;
use multisig_support::*;
use serde_json::Value;
use starknet_wallet_core::accounts::multisig::{normalize, signer_guid_hex, ArgentMultisigConfig};
use starknet_wallet_core::send::multisig_intent::{ConfigSnapshot, SigningIntent};
use starknet_wallet_core::send::multisig_invoke::{
    serialize_signature, MultisigCall, MultisigSignature,
};
use starknet_wallet_core::send::multisig_invoke_plan::{MultisigStep, ProposePlan, SubmitPlan};
use starknet_wallet_core::send::types::StepReply;
use starknet_wallet_core::{nostr, sign_stark_hash};

fn exchanges(proposal: &Value, phase: &str) -> Vec<Value> {
    proposal[phase]
        .as_array()
        .unwrap()
        .iter()
        .map(decode)
        .collect()
}

fn json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap()
}

#[test]
fn proposals_match_the_extension_on_both_networks() {
    let fixture = fixture();
    let networks = fixture["networks"].as_array().unwrap();
    assert_eq!(networks.len(), 2);
    for network in networks {
        run(network);
    }
}

fn run(network: &Value) {
    let name = network["networkId"].as_str().unwrap();
    let class = class();
    let record = record(network);
    let config = current_config(&record);
    let proposal = &network["proposal"];

    // The phrase's OpenZeppelin accounts are the signers the fixture names.
    for signer_record in network["signers"].as_array().unwrap() {
        let index = signer_record["accountIndex"].as_u64().unwrap() as u32;
        let (_, public_key) = signer(index);
        assert_eq!(public_key, signer_record["publicKey"].as_str().unwrap());
        assert_eq!(
            signer_guid_hex(&public_key).unwrap(),
            signer_record["guid"].as_str().unwrap()
        );
    }

    // Propose: chain id, nonce, the validated estimate, then the frozen transaction.
    let calls: Vec<MultisigCall> = serde_json::from_value(proposal["calls"].clone()).unwrap();
    let mut plan =
        ProposePlan::new(&record, &calls, &config, &class, MULTISIG_FEE_OVERHEAD).unwrap();
    let first = plan.advance(StepReply::Start).unwrap();
    let last = drive(first, &exchanges(proposal, "prepare"), |reply| {
        plan.advance(reply).unwrap()
    });
    let MultisigStep::Prepared(prepared) = last else {
        panic!("{name}: {last:?}");
    };
    let prepared = *prepared;
    let hash = proposal["transactionHash"].as_str().unwrap();
    assert_eq!(prepared.transaction_hash, hash, "{name}");
    assert!(prepared.is_valid(), "{name}");
    let fee = prepared.fee().unwrap();
    assert_eq!(
        (fee.spend.to_decimal(), fee.unit.code()),
        (
            proposal["feeReview"]["overallFee"]
                .as_str()
                .unwrap()
                .to_string(),
            proposal["feeReview"]["unit"].as_str().unwrap()
        ),
        "{name}"
    );

    // The collecting intent: the chain's authority (the current config's GUIDs) and the
    // review's summary, which the call review (not this slice) wrote.
    let stages = proposal["stages"].as_array().unwrap();
    let recorded: SigningIntent =
        serde_json::from_str(stages[0]["intent"].as_str().unwrap()).unwrap();
    let (normalized, guids) = normalize(&config, &class).unwrap();
    let snapshot = ConfigSnapshot::new(&class, normalized.threshold, &guids);
    let mut intent =
        SigningIntent::proposed(&record.id, snapshot, prepared.clone(), &recorded.summary).unwrap();
    let check_stage = |intent: &SigningIntent, stage: &Value| {
        let text = stage["intent"].as_str().unwrap();
        assert_eq!(json(intent), text, "{name}");
        // A co-signer's copy reads back to the same intent and the same bytes.
        let read: SigningIntent = serde_json::from_str(text).unwrap();
        assert_eq!(&read, intent, "{name}");
        assert_eq!(
            intent.is_valid(&class, summary_length()),
            stage["valid"].as_bool().unwrap(),
            "{name}"
        );
    };
    check_stage(&intent, &stages[0]);
    let package = intent.package().to_json();
    assert_eq!(
        package,
        proposal["proposalPackage"].as_str().unwrap(),
        "{name}"
    );
    assert_eq!(
        nostr::is_application_data(&package),
        proposal["proposalPackageFitsNostr"].as_bool().unwrap(),
        "{name}"
    );

    // Each co-signer signs the frozen hash with Krusty and the signature is merged in.
    let cosigners = proposal["cosignerAccountIndexes"].as_array().unwrap();
    let signed = proposal["signatures"].as_array().unwrap();
    assert_eq!(cosigners.len(), signed.len());
    for (step, (index, expected)) in cosigners.iter().zip(signed).enumerate() {
        let (key, public_key) = signer(index.as_u64().unwrap() as u32);
        assert_eq!(expected["hash"].as_str().unwrap(), hash, "{name}");
        let signature = sign_stark_hash(&key, hash).unwrap();
        assert_eq!(signature.public_key, public_key);
        assert_eq!(
            (signature.r.as_str(), signature.s.as_str()),
            (
                expected["r"].as_str().unwrap(),
                expected["s"].as_str().unwrap()
            ),
            "{name}: co-signer {index}"
        );
        let mut incoming = intent.clone();
        incoming.signatures.push(MultisigSignature::from(signature));
        intent = intent.merge_imported(&incoming).unwrap();
        check_stage(&intent, &stages[step + 1]);
    }
    assert_eq!(stages.len(), cosigners.len() + 1);
    assert_eq!(
        intent.package().to_json(),
        proposal["readyPackage"].as_str().unwrap(),
        "{name}"
    );

    // Submit: the generator's config is the signers who signed, at the snapshot's threshold.
    let submit_config = ArgentMultisigConfig {
        threshold: intent.config_snapshot.threshold,
        signer_public_keys: intent
            .signatures
            .iter()
            .map(|s| s.public_key.clone())
            .collect(),
    };
    let serialized = serialize_signature(&intent.signatures, &submit_config, &class).unwrap();
    let expected: Vec<String> = serde_json::from_value(proposal["signature"].clone()).unwrap();
    assert_eq!(serialized, expected, "{name}");

    assert_eq!(intent.prepared, prepared, "{name}");
    let mut submit = SubmitPlan::new(&intent, &submit_config, &class, summary_length()).unwrap();
    let first = submit.advance(StepReply::Start, &|| true).unwrap();
    let last = drive(first, &exchanges(proposal, "submit"), |reply| {
        submit.advance(reply, &|| true).unwrap()
    });
    assert_eq!(
        last,
        MultisigStep::Submitted { hash: hash.into() },
        "{name}"
    );
}
