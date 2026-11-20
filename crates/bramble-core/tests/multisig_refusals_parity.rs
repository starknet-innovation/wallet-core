//! What the extension's multisig code refuses, the Rust must refuse, with the typed refusal that
//! carries the extension's sentence. `extension-multisig-transactions-refusals.json` (G5,
//! `mobile-multisig-refusals.ts`) hands the real extension functions bad inputs and records the
//! error (`{name, message}`, hashes as `{hash}`; `{name, code}` for a node error passed through)
//! or the verdict. Each case is replayed here against `accounts::multisig`,
//! `send::multisig_invoke{,_plan}` and `send::multisig_intent` under Bramble's pinned class.
//!
//! `calls` is call review (P1b) and is not read here; `deploySubmit` drives the deployment plan
//! and lives with its wire in `multisig_deploy_parity.rs`.

mod multisig_support;

use bramble_core::MULTISIG_FEE_OVERHEAD;
use multisig_support::*;
use serde_json::Value;
use starknet_wallet_core::accounts::multisig::{predict, ArgentMultisigConfig};
use starknet_wallet_core::send::multisig_deploy::{deployment_plan, MultisigDeployRefusal};
use starknet_wallet_core::send::multisig_intent::{IntentState, SigningIntent};
use starknet_wallet_core::send::multisig_invoke::{
    serialize_signature, MultisigCall, MultisigInvokeError as Invoke, MultisigSignature,
};
use starknet_wallet_core::send::multisig_invoke_plan::{
    MultisigFailure as Failure, MultisigStep, ProposePlan, SubmitPlan,
};
use starknet_wallet_core::send::types::StepReply;
use starknet_wallet_core::{felt, sign_stark_hash, Error};

fn refusals() -> Value {
    let path = format!(
        "{}/tests/fixtures/extension-multisig-transactions-refusals.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// The section's cases, of which there must be exactly `count`.
fn section<'a>(fixture: &'a Value, name: &str, count: usize) -> &'a [Value] {
    let cases = fixture[name].as_array().unwrap();
    assert_eq!(cases.len(), count, "{name}");
    cases
}

/// The Rust refusal that carries one extension refusal.
#[derive(Debug, PartialEq)]
enum Rust {
    /// `accounts::multisig`'s fixed reason.
    Input(&'static str),
    Invoke(Invoke),
    Plan(Failure),
}

/// Every refusal this file meets, by `<name>: <message>` (or `<name>: <code>`). Many-to-one:
/// none here; four deploy-config sentences also collapse to `MultisigDeployRefusal::InvalidConfig`
/// once they reach the deployment plan (asserted in `deploy_configs_*`).
fn rust_for(refusal: &Value) -> Rust {
    let name = refusal["name"].as_str().unwrap();
    let detail = match (&refusal["message"], &refusal["code"]) {
        (Value::String(message), _) => message.clone(),
        (_, code) => code.to_string(),
    };
    match format!("{name}: {detail}").as_str() {
        "WalletError: Argent multisig signers must be unique" => {
            Rust::Input("multisig signers must be unique")
        }
        "WalletError: Invalid Argent multisig threshold or signer count" => {
            Rust::Input("invalid multisig threshold or signer count")
        }
        "WalletError: Invalid Argent multisig salt" => Rust::Input("invalid deployment salt"),
        "WalletError: Invalid signer public key" => Rust::Input("invalid signer public key"),
        "WalletError: RPC endpoint chain id does not match the account network" => {
            Rust::Plan(Failure::WrongChain)
        }
        "WalletError: Account is not the pinned Argent multisig class" => {
            Rust::Invoke(Invoke::NotPinnedMultisig)
        }
        "WalletError: Invalid Argent multisig calls" => Rust::Invoke(Invoke::InvalidCalls),
        // The extension names the count; the Rust variant carries no value.
        "WalletError: Argent multisig requires exactly 2 signature(s)" => {
            Rust::Invoke(Invoke::SignatureCount)
        }
        "WalletError: Argent multisig signature is not from a configured signer" => {
            Rust::Invoke(Invoke::NotASigner)
        }
        "WalletError: Argent multisig signatures must use distinct signers" => {
            Rust::Invoke(Invoke::DuplicateSigner)
        }
        "WalletError: Invalid prepared Argent multisig transaction" => {
            Rust::Invoke(Invoke::InvalidPrepared)
        }
        "ArgentMultisigFeeExpiredError: Not submitted \u{2014} network fees changed." => {
            Rust::Plan(Failure::FeeExpired)
        }
        // The node's own error, passed through: nothing was accepted.
        "RpcError: 52" => Rust::Plan(Failure::NodeError),
        // The extension calls a write answered with another hash indeterminate; the plan says
        // which (PR #70): the node accepted something else, so check the chain.
        "IndeterminateRpcWriteError: RPC accepted a different transaction hash. Check on-chain state before retrying." => {
            Rust::Plan(Failure::HashMismatch)
        }
        "IndeterminateRpcWriteError: Transaction submission status is unknown. Check activity and on-chain state before retrying." => {
            Rust::Plan(Failure::SubmissionIndeterminate)
        }
        other => panic!("unmapped extension refusal {other}"),
    }
}

fn input_error(error: Error) -> Rust {
    match error {
        Error::InvalidInput { reason } => Rust::Input(match reason.as_str() {
            "multisig signers must be unique" => "multisig signers must be unique",
            "invalid multisig threshold or signer count" => {
                "invalid multisig threshold or signer count"
            }
            "invalid deployment salt" => "invalid deployment salt",
            "invalid signer public key" => "invalid signer public key",
            other => panic!("unexpected reason {other}"),
        }),
        other => panic!("{other:?}"),
    }
}

fn ready(fixture: &Value) -> SigningIntent {
    serde_json::from_str(fixture["readyIntent"].as_str().unwrap()).unwrap()
}

/// The generator's submit and signature config: the signers who signed, at the threshold.
fn signed_config(intent: &SigningIntent) -> ArgentMultisigConfig {
    ArgentMultisigConfig {
        threshold: intent.config_snapshot.threshold,
        signer_public_keys: intent
            .signatures
            .iter()
            .map(|s| s.public_key.clone())
            .collect(),
    }
}

/// `signAs(signers[index], hash)`: Krusty signing as the phrase's account `index`.
fn sign_as(index: u32, hash: &str) -> MultisigSignature {
    let (key, _) = signer(index);
    MultisigSignature::from(sign_stark_hash(&key, hash).unwrap())
}

const OUTSIDER: u32 = 4;

#[test]
fn deploy_configs_the_extension_refuses_are_refused() {
    let fixture = refusals();
    let class = class();
    let member = felt::parse(&current_config(&record(&fixture)).signer_public_keys[0]).unwrap();
    for case in section(&fixture, "deployConfig", 7) {
        let name = case["name"].as_str().unwrap();
        let input = decode(&case["input"]);
        let config = ArgentMultisigConfig {
            threshold: input["threshold"].as_u64().unwrap() as u32,
            signer_public_keys: serde_json::from_value(input["signerPublicKeys"].clone()).unwrap(),
        };
        let salt = input["salt"].as_str().unwrap();
        let refused = predict(&config, salt, &class).expect_err(name);
        assert_eq!(input_error(refused), rust_for(&case["refusal"]), "{name}");
        let network = starknet_wallet_core::kms::NetworkId::Sepolia;
        let plan = deployment_plan(
            network,
            &config,
            salt,
            &class,
            &member,
            bramble_core::SEND_POLICY,
        );
        assert_eq!(
            plan.err(),
            Some(MultisigDeployRefusal::InvalidConfig),
            "{name}"
        );
    }
}

#[test]
fn proposals_the_extension_refuses_are_refused() {
    let fixture = refusals();
    let primitives = TestPrimitives::new();
    let class = class();
    let account = record(&fixture);
    let config = current_config(&account);
    let transfer: MultisigCall = serde_json::from_value(fixture["transfer"].clone()).unwrap();
    for case in section(&fixture, "prepare", 3) {
        let name = case["name"].as_str().unwrap();
        let mut account = account.clone();
        let mut calls = vec![transfer.clone()];
        match name {
            "notTheMultisigClass" => account.class_hash = "0x1".into(),
            "noCalls" => calls.clear(),
            _ => {}
        }
        let wire: Vec<Value> = case["wire"]
            .as_array()
            .unwrap()
            .iter()
            .map(decode)
            .collect();
        let got = match ProposePlan::new(
            &account,
            &calls,
            &config,
            &class,
            MULTISIG_FEE_OVERHEAD,
            &primitives,
        ) {
            Err(error) => {
                assert!(wire.is_empty(), "{name}");
                Rust::Invoke(error)
            }
            Ok(mut plan) => {
                let first = plan.advance(StepReply::Start).unwrap();
                match drive(first, &wire, |reply| plan.advance(reply).unwrap()) {
                    MultisigStep::Failed(failure) => Rust::Plan(failure),
                    other => panic!("{name}: not refused, {other:?}"),
                }
            }
        };
        assert_eq!(got, rust_for(&case["refusal"]), "{name}");
    }
}

/// The JSON paths where `left` and `right` differ (an array entry only one side has is its index).
fn diff(left: &Value, right: &Value, at: String, out: &mut Vec<String>) {
    match (left, right) {
        (Value::Object(l), Value::Object(r)) => {
            for key in l.keys().chain(r.keys().filter(|k| !l.contains_key(*k))) {
                let (a, b) = (
                    l.get(key).unwrap_or(&Value::Null),
                    r.get(key).unwrap_or(&Value::Null),
                );
                diff(a, b, format!("{at}/{key}"), out);
            }
        }
        (Value::Array(l), Value::Array(r)) => {
            for index in 0..l.len().max(r.len()) {
                match (l.get(index), r.get(index)) {
                    (Some(a), Some(b)) => diff(a, b, format!("{at}/{index}"), out),
                    _ => out.push(format!("{at}/{index}")),
                }
            }
        }
        _ if left != right => out.push(at),
        _ => {}
    }
}

#[test]
fn stored_intents_get_the_extensions_verdict() {
    let fixture = refusals();
    let primitives = TestPrimitives::new();
    let class = class();
    let cases = section(&fixture, "intents", 12);
    let raw = |name: &str| {
        let case = cases.iter().find(|c| c["name"] == name).unwrap();
        decode(&case["intent"])
    };
    // Each refused intent is a valid one (the ready intent, or `collectingOne` for the cases the
    // generator built from it; both asserted valid in this loop) with one change: where it differs from that base is exactly
    // what the case names, so that change alone is what is refused.
    let (ready, collecting) = (raw("ready"), raw("collectingOne"));
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let intent: SigningIntent = serde_json::from_str(case["intent"].as_str().unwrap())
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let valid = intent.is_valid(&class, summary_length(), &primitives);
        assert_eq!(valid, case["valid"].as_bool().unwrap(), "{name}");
        let (base, changed): (&Value, &[&str]) = match name {
            "ready" | "collectingOne" => continue,
            "duplicateSigner" => (
                &ready,
                &[
                    "/signatures/1/publicKey",
                    "/signatures/1/r",
                    "/signatures/1/s",
                ],
            ),
            "nonMemberSignature" => (
                &collecting,
                &[
                    "/signatures/0/publicKey",
                    "/signatures/0/r",
                    "/signatures/0/s",
                ],
            ),
            "outOfGuidOrder" => (
                &ready,
                &[
                    "/signatures/0/publicKey",
                    "/signatures/0/r",
                    "/signatures/0/s",
                    "/signatures/1/publicKey",
                    "/signatures/1/r",
                    "/signatures/1/s",
                ],
            ),
            "overThreshold" => (&ready, &["/signatures/2"]),
            "readyBelowThreshold" => (&ready, &["/signatures/1"]),
            "collectingAtThreshold" => (&ready, &["/state"]),
            // Same claimed key, other (r, s): only verification can refuse it.
            "signatureOverOtherHash" => (&collecting, &["/signatures/0/r", "/signatures/0/s"]),
            "tamperedNonce" => (&ready, &["/prepared/nonce"]),
            "wrongId" => (&ready, &["/id"]),
            "submittedWithoutHash" => (&ready, &["/state"]),
            other => panic!("unknown intent case {other}"),
        };
        let mut paths = Vec::new();
        diff(base, &decode(&case["intent"]), String::new(), &mut paths);
        assert_eq!(paths, changed, "{name}");
    }
    // `nonMemberSignature` is refused by membership before verification, as in the extension.
}

#[test]
fn replacements_get_the_extensions_verdict() {
    let fixture = refusals();
    let primitives = TestPrimitives::new();
    let class = class();
    let intent = ready(&fixture);
    let collecting = SigningIntent {
        signatures: intent.signatures[..1].to_vec(),
        state: IntentState::Collecting,
        ..intent.clone()
    };
    let submitted = SigningIntent {
        broadcast_transaction_hash: Some(intent.prepared.transaction_hash.clone()),
        state: IntentState::Submitted,
        ..intent.clone()
    };
    let resummarised = SigningIntent {
        summary: "Something else".into(),
        ..intent.clone()
    };
    for case in section(&fixture, "replacements", 5) {
        let name = case["name"].as_str().unwrap();
        let (current, next) = match name {
            "collectingToReady" => (&collecting, &intent),
            "dropsASignature" => (&intent, &collecting),
            "readyToSubmitted" => (&intent, &submitted),
            "collectingToSubmitted" => (&collecting, &submitted),
            "changesSummary" => (&collecting, &resummarised),
            other => panic!("unknown replacement {other}"),
        };
        assert_eq!(
            next.signatures.len() as u64,
            case["nextSignatureCount"].as_u64().unwrap(),
            "{name}"
        );
        assert_eq!(serde_json::to_value(next.state).unwrap(), case["nextState"]);
        assert_eq!(
            current.can_replace(next, &class, summary_length(), &primitives),
            case["canReplace"].as_bool().unwrap(),
            "{name}"
        );
    }
}

#[test]
fn reimports_merge_as_the_extension_merges() {
    let fixture = refusals();
    let intent = ready(&fixture);
    let [first, second] = [&intent.signatures[0], &intent.signatures[1]];
    let local = SigningIntent {
        signatures: vec![second.clone()],
        state: IntentState::Collecting,
        ..intent.clone()
    };
    let remote = SigningIntent {
        signatures: vec![first.clone()],
        state: IntentState::Collecting,
        summary: "Remote".into(),
        ..intent.clone()
    };
    let submitted = SigningIntent {
        broadcast_transaction_hash: Some(intent.prepared.transaction_hash.clone()),
        state: IntentState::Submitted,
        ..intent.clone()
    };
    for case in section(&fixture, "merges", 2) {
        let name = case["name"].as_str().unwrap();
        let merged = match name {
            "addsTheOtherSignature" => local.merge_imported(&remote).unwrap(),
            "terminalStateWins" => submitted.merge_imported(&remote).unwrap(),
            other => panic!("unknown merge {other}"),
        };
        let keys: Vec<&str> = merged
            .signatures
            .iter()
            .map(|s| s.public_key.as_str())
            .collect();
        let expected: Vec<&str> = case["signerKeys"]
            .as_array()
            .unwrap()
            .iter()
            .map(|k| k.as_str().unwrap())
            .collect();
        assert_eq!(keys, expected, "{name}");
        assert_eq!(serde_json::to_value(merged.state).unwrap(), case["state"]);
        assert_eq!(merged.summary, case["summary"].as_str().unwrap(), "{name}");
        assert!(
            merged.is_valid(&class(), summary_length(), &TestPrimitives::new()),
            "{name}"
        );
        assert_eq!(merged.stale_outcome, None, "{name}");
        assert_eq!(merged.stale_reason, None, "{name}");
        // Only the kept terminal intent carries its broadcast hash.
        assert_eq!(
            merged.broadcast_transaction_hash.is_some(),
            name == "terminalStateWins",
            "{name}"
        );
    }
}

#[test]
fn signature_sets_the_extension_refuses_are_refused() {
    let fixture = refusals();
    let class = class();
    let intent = ready(&fixture);
    let config = signed_config(&intent);
    let [first, second] = [&intent.signatures[0], &intent.signatures[1]];
    let outsider = sign_as(OUTSIDER, &intent.prepared.transaction_hash);
    for case in section(&fixture, "signatures", 3) {
        let name = case["name"].as_str().unwrap();
        let set = match name {
            "belowThreshold" => vec![first.clone()],
            "nonMember" => vec![first.clone(), outsider.clone()],
            "sameSignerTwice" => vec![second.clone(), second.clone()],
            other => panic!("unknown signature set {other}"),
        };
        let keys: Vec<Value> = set.iter().map(|s| s.public_key.clone().into()).collect();
        assert_eq!(Value::from(keys), case["signerKeys"], "{name}");
        let refused = serialize_signature(&set, &config, &class).expect_err(name);
        assert_eq!(Rust::Invoke(refused), rust_for(&case["refusal"]), "{name}");
    }
}

#[test]
fn submissions_the_extension_refuses_are_refused() {
    let fixture = refusals();
    let primitives = TestPrimitives::new();
    let class = class();
    let intent = ready(&fixture);
    let config = signed_config(&intent);
    let cases = section(&fixture, "submit", 6);
    // The write the extension sent, for the lost-response case whose capture stops before it.
    let write_params = decode(&cases[2]["wire"][1])["params"].clone();
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let mut candidate = intent.clone();
        match name {
            "belowThreshold" => candidate.signatures.truncate(1),
            "tamperedPrepared" => candidate.prepared.nonce = "0x6".into(),
            _ => {}
        }
        let wire: Vec<Value> = case["wire"]
            .as_array()
            .unwrap()
            .iter()
            .map(decode)
            .collect();
        let got = match SubmitPlan::new(&candidate, &config, &class, summary_length(), &primitives)
        {
            Err(error) => {
                assert!(wire.is_empty(), "{name}");
                Rust::Invoke(error)
            }
            Ok(mut plan) => {
                let first = plan.advance(StepReply::Start, &|| true).unwrap();
                let mut advance = |reply| plan.advance(reply, &|| true).unwrap();
                let last = if name == "responseLost" {
                    // Captured up to the chain proof; the write itself got no answer.
                    let (id, _, _, _) = rpc_parts(&first);
                    let write = advance(rpc_reply(id, &wire[0]["reply"]));
                    let (id, method, params, is_write) = rpc_parts(&write);
                    assert_eq!(
                        (method.as_str(), &params, is_write),
                        ("starknet_addInvokeTransaction", &write_params, true)
                    );
                    advance(StepReply::TransportFailure { id, now_ms: 0 })
                } else {
                    drive(first, &wire, advance)
                };
                match last {
                    MultisigStep::Failed(failure) => Rust::Plan(failure),
                    other => panic!("{name}: not refused, {other:?}"),
                }
            }
        };
        assert_eq!(got, rust_for(&case["refusal"]), "{name}");
    }
}
