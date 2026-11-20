//! The multisig proposal and submission plans as state machines, and the refusals of the
//! signature serialiser and the intent validator: what the parity captures cannot show (a reply
//! out of order, a node error, a wrong chain, a lock before the write, a different hash, an
//! expired fee ceiling, a tampered intent). Replies come from the Sepolia capture.

mod multisig_support;

use bramble_core::MULTISIG_FEE_OVERHEAD;
use multisig_support::*;
use serde_json::{json, Value};
use starknet_wallet_core::accounts::multisig::{normalize, ArgentMultisigConfig};
use starknet_wallet_core::send::multisig_intent::{ConfigSnapshot, IntentState, SigningIntent};
use starknet_wallet_core::send::multisig_invoke::{
    serialize_signature, MultisigCall, MultisigInvokeError, MultisigSignature, PreparedInvoke,
};
use starknet_wallet_core::send::multisig_invoke_plan::{
    MultisigFailure, MultisigStep, ProposePlan, SubmitPlan,
};
use starknet_wallet_core::send::types::{SendError, StepReply};
use starknet_wallet_core::sign_stark_hash;

fn sepolia() -> Value {
    fixture()["networks"][0].clone()
}

fn exchanges(phase: &str) -> Vec<Value> {
    sepolia()["proposal"][phase]
        .as_array()
        .unwrap()
        .iter()
        .map(decode)
        .collect()
}

fn propose_plan() -> ProposePlan {
    let network = sepolia();
    let record = record(&network);
    let calls: Vec<MultisigCall> =
        serde_json::from_value(network["proposal"]["calls"].clone()).unwrap();
    ProposePlan::new(
        &record,
        &calls,
        &current_config(&record),
        &class(),
        MULTISIG_FEE_OVERHEAD,
        &TestPrimitives::new(),
    )
    .unwrap()
}

fn prepared() -> PreparedInvoke {
    let mut plan = propose_plan();
    let first = plan.advance(StepReply::Start).unwrap();
    match drive(first, &exchanges("prepare"), |r| plan.advance(r).unwrap()) {
        MultisigStep::Prepared(prepared) => *prepared,
        other => panic!("{other:?}"),
    }
}

/// The intent signed by the fixture's co-signers (accounts 1 and 2), in GUID order.
fn ready_intent() -> SigningIntent {
    let record = record(&sepolia());
    let config = current_config(&record);
    let (normalized, guids) = normalize(&config, &class()).unwrap();
    let prepared = prepared();
    let hash = prepared.transaction_hash.clone();
    let mut intent = SigningIntent::proposed(
        &record.id,
        ConfigSnapshot::new(&class(), normalized.threshold, &guids),
        prepared,
        "Send 1.5 STRK to 0x74a5…7d89",
    )
    .unwrap();
    for index in [1, 2] {
        let (key, _) = signer(index);
        let mut incoming = intent.clone();
        incoming
            .signatures
            .push(sign_stark_hash(&key, &hash).unwrap().into());
        intent = intent.merge_imported(&incoming).unwrap();
    }
    assert_eq!(intent.state, IntentState::Ready);
    intent
}

fn valid(intent: &SigningIntent) -> bool {
    intent.is_valid(&class(), summary_length(), &TestPrimitives::new())
}

fn submit_plan(intent: &SigningIntent) -> SubmitPlan {
    SubmitPlan::new(
        intent,
        &current_config(&record(&sepolia())),
        &class(),
        summary_length(),
        &TestPrimitives::new(),
    )
    .unwrap()
}

/// Proves the chain and returns the write step (authorised).
fn to_write(plan: &mut SubmitPlan) -> MultisigStep {
    let step = plan.advance(StepReply::Start, &|| true).unwrap();
    let (id, method, _, _) = rpc_parts(&step);
    assert_eq!(method, "starknet_chainId");
    plan.advance(
        rpc_reply(id, &json!({"result": "0x534e5f5345504f4c4941"})),
        &|| true,
    )
    .unwrap()
}

#[test]
fn a_reply_out_of_order_closes_the_propose_plan() {
    let mut plan = propose_plan();
    let step = plan.advance(StepReply::Start).unwrap();
    let (id, _, _, _) = rpc_parts(&step);
    let reply = rpc_reply(id + 1, &json!({"result": "0x534e5f5345504f4c4941"}));
    assert_eq!(plan.advance(reply), Err(SendError::OutOfOrder));
    assert_eq!(plan.advance(StepReply::Start), Err(SendError::PlanClosed));
    let mut fresh = propose_plan();
    assert_eq!(
        fresh.advance(StepReply::WaitElapsed),
        Err(SendError::OutOfOrder)
    );
}

#[test]
fn node_errors_and_a_wrong_chain_end_the_proposal() {
    let error = json!({"error": {"code": 20, "message": "Contract not found"}});
    // A wrong chain stops before the nonce read.
    let mut plan = propose_plan();
    let step = plan.advance(StepReply::Start).unwrap();
    let (id, _, _, _) = rpc_parts(&step);
    let mainnet = rpc_reply(id, &json!({"result": "0x534e5f4d41494e"}));
    assert_eq!(
        plan.advance(mainnet),
        Ok(MultisigStep::Failed(MultisigFailure::WrongChain))
    );
    // A nonce error is never read as nonce zero (starknet.js `getNonceForAddress` throws).
    let mut plan = propose_plan();
    let mut step = plan.advance(StepReply::Start).unwrap();
    let chain = &exchanges("prepare")[0]["reply"];
    step = plan.advance(rpc_reply(rpc_parts(&step).0, chain)).unwrap();
    let (id, method, _, _) = rpc_parts(&step);
    assert_eq!(method, "starknet_getNonce");
    assert_eq!(
        plan.advance(rpc_reply(id, &error)),
        Ok(MultisigStep::Failed(MultisigFailure::NodeError))
    );
    assert_eq!(plan.advance(StepReply::Start), Err(SendError::PlanClosed));
    // A transport failure on the estimate: nothing was prepared.
    let mut plan = propose_plan();
    let mut step = plan.advance(StepReply::Start).unwrap();
    for exchange in &exchanges("prepare")[..2] {
        step = plan
            .advance(rpc_reply(rpc_parts(&step).0, &exchange["reply"]))
            .unwrap();
    }
    let (id, method, _, is_write) = rpc_parts(&step);
    assert_eq!((method.as_str(), is_write), ("starknet_estimateFee", false));
    assert_eq!(
        plan.advance(StepReply::TransportFailure { id, now_ms: 0 }),
        Ok(MultisigStep::Failed(
            MultisigFailure::PreparationUnavailable
        ))
    );
}

#[test]
fn submit_writes_once_and_judges_the_answer() {
    let intent = ready_intent();
    let hash = intent.prepared.transaction_hash.clone();
    let cases = [
        (json!({"result": {"transaction_hash": hash}}), None),
        (
            json!({"result": {"transaction_hash": "0x1234"}}),
            Some(MultisigFailure::HashMismatch),
        ),
        (
            json!({"result": {}}),
            Some(MultisigFailure::SubmissionIndeterminate),
        ),
        (
            json!({"error": {"code": 55, "message": "Account validation failed",
                "data": "Max L1 gas price (1) is lower than the actual gas price: 2"}}),
            Some(MultisigFailure::FeeExpired),
        ),
        (
            json!({"error": {"code": 52, "message": "Invalid transaction nonce"}}),
            Some(MultisigFailure::NodeError),
        ),
    ];
    for (reply, failure) in cases {
        let mut plan = submit_plan(&intent);
        let step = to_write(&mut plan);
        let (id, method, _, is_write) = rpc_parts(&step);
        assert_eq!(
            (method.as_str(), is_write),
            ("starknet_addInvokeTransaction", true)
        );
        let expected = match failure {
            None => MultisigStep::Submitted { hash: hash.clone() },
            Some(failure) => MultisigStep::Failed(failure),
        };
        assert_eq!(plan.advance(rpc_reply(id, &reply), &|| true), Ok(expected));
        assert_eq!(
            plan.advance(StepReply::Start, &|| true),
            Err(SendError::PlanClosed)
        );
    }
    // A lost reply to the write is indeterminate, never "not sent".
    let mut plan = submit_plan(&intent);
    let (id, _, _, _) = rpc_parts(&to_write(&mut plan));
    assert_eq!(
        plan.advance(StepReply::TransportFailure { id, now_ms: 0 }, &|| true),
        Ok(MultisigStep::Failed(
            MultisigFailure::SubmissionIndeterminate
        ))
    );
}

#[test]
fn a_lock_or_a_wrong_chain_stops_submit_before_the_write() {
    let intent = ready_intent();
    let chain = |plan: &mut SubmitPlan, result: &str, authorized: bool| {
        let step = plan.advance(StepReply::Start, &|| true).unwrap();
        let reply = rpc_reply(rpc_parts(&step).0, &json!({ "result": result }));
        plan.advance(reply, &|| authorized).unwrap()
    };
    let mut plan = submit_plan(&intent);
    assert_eq!(
        chain(&mut plan, "0x534e5f5345504f4c4941", false),
        MultisigStep::Failed(MultisigFailure::Locked)
    );
    let mut plan = submit_plan(&intent);
    assert_eq!(
        chain(&mut plan, "0x534e5f4d41494e", true),
        MultisigStep::Failed(MultisigFailure::WrongChain)
    );
    // The multisig grammar (`canonicalArgentMultisigFelt`) reads any `BigInt` spelling, so an
    // upper-case id is the same chain (and reaches the lock check); a non-number is unreadable.
    let mut plan = submit_plan(&intent);
    assert_eq!(
        chain(&mut plan, "0x534E5F5345504F4C4941", false),
        MultisigStep::Failed(MultisigFailure::Locked)
    );
    let mut plan = submit_plan(&intent);
    assert_eq!(
        chain(&mut plan, "sepolia", true),
        MultisigStep::Failed(MultisigFailure::PreparationUnavailable)
    );
}

#[test]
fn submit_refuses_a_tampered_transaction_or_an_incomplete_signature() {
    let intent = ready_intent();
    let config = current_config(&record(&sepolia()));
    let primitives = TestPrimitives::new();
    let submit = |candidate: &SigningIntent| {
        SubmitPlan::new(candidate, &config, &class(), summary_length(), &primitives).err()
    };
    let refusal = |prepared: &PreparedInvoke, signatures: &[MultisigSignature]| {
        submit(&SigningIntent {
            prepared: prepared.clone(),
            signatures: signatures.to_vec(),
            ..intent.clone()
        })
    };
    assert_eq!(submit(&intent), None);
    // A member's key with another member's `r`: well-formed, but not a signature over the hash.
    let mut forged = intent.clone();
    forged.signatures[0].r = forged.signatures[1].r.clone();
    assert_eq!(submit(&forged), Some(MultisigInvokeError::InvalidIntent));
    // The extension submits only a ready, stale or submitted intent.
    let collecting = SigningIntent {
        state: IntentState::Collecting,
        ..intent.clone()
    };
    assert_eq!(
        submit(&collecting),
        Some(MultisigInvokeError::InvalidIntent)
    );
    let mut tampered = intent.prepared.clone();
    tampered.nonce = "0x6".into();
    assert_eq!(
        refusal(&tampered, &intent.signatures),
        Some(MultisigInvokeError::InvalidPrepared)
    );
    let mut padded = intent.prepared.clone();
    padded.chain_id = "0x0534e5f5345504f4c4941".into();
    assert_eq!(
        refusal(&padded, &intent.signatures),
        Some(MultisigInvokeError::InvalidPrepared)
    );
    assert_eq!(
        refusal(&intent.prepared, &intent.signatures[..1]),
        Some(MultisigInvokeError::SignatureCount)
    );
}

#[test]
fn the_signature_is_exactly_threshold_distinct_members_in_guid_order() {
    let intent = ready_intent();
    let config = current_config(&record(&sepolia()));
    let sigs = &intent.signatures;
    let ordered = serialize_signature(sigs, &config, &class()).unwrap();
    let reversed: Vec<MultisigSignature> = sigs.iter().rev().cloned().collect();
    assert_eq!(
        serialize_signature(&reversed, &config, &class()).unwrap(),
        ordered
    );
    let twice = vec![sigs[0].clone(), sigs[0].clone()];
    assert_eq!(
        serialize_signature(&twice, &config, &class()),
        Err(MultisigInvokeError::DuplicateSigner)
    );
    let (key, _) = signer(4);
    let outsider: MultisigSignature = sign_stark_hash(&key, &intent.prepared.transaction_hash)
        .unwrap()
        .into();
    assert_eq!(
        serialize_signature(&[sigs[0].clone(), outsider], &config, &class()),
        Err(MultisigInvokeError::NotASigner)
    );
    let zero = MultisigSignature {
        r: "0x0".into(),
        ..sigs[0].clone()
    };
    assert_eq!(
        serialize_signature(&[zero, sigs[1].clone()], &config, &class()),
        Err(MultisigInvokeError::InvalidSignature)
    );
    let bad = ArgentMultisigConfig {
        threshold: 4,
        ..config.clone()
    };
    assert_eq!(
        serialize_signature(sigs, &bad, &class()),
        Err(MultisigInvokeError::InvalidConfig)
    );
}

#[test]
fn the_validator_refuses_what_the_extension_refuses() {
    let intent = ready_intent();
    assert!(valid(&intent));
    let mutate = |change: &dyn Fn(&mut SigningIntent)| {
        let mut copy = intent.clone();
        change(&mut copy);
        valid(&copy)
    };
    // A signature that does not verify over the hash.
    assert!(!mutate(&|i| i.signatures[0].r = i.signatures[1].r.clone()));
    // Signatures out of GUID order.
    assert!(!mutate(&|i| i.signatures.reverse()));
    // A state the signature count does not support.
    assert!(!mutate(&|i| i.state = IntentState::Collecting));
    assert!(!mutate(&|i| i.state = IntentState::Submitted));
    // Submitted with its own broadcast hash is consistent.
    assert!(mutate(&|i| {
        i.state = IntentState::Submitted;
        i.broadcast_transaction_hash = Some(i.prepared.transaction_hash.clone());
    }));
    // The frozen fields, the id, the account and the summary are all bound.
    assert!(!mutate(
        &|i| i.prepared.resource_bounds.l2_gas.max_amount = "0x1".into()
    ));
    assert!(!mutate(&|i| i.id.push('0')));
    assert!(!mutate(
        &|i| i.account_id = i.account_id.replace("SN_SEPOLIA", "SN_MAIN")
    ));
    assert!(!mutate(&|i| i.summary = format!("{} ", i.summary)));
    assert!(!mutate(&|i| i.summary = "x".repeat(summary_length() + 1)));
    assert!(!mutate(&|i| i.config_snapshot.signer_guids.reverse()));
    assert!(!mutate(&|i| i.config_snapshot.class_hash = "0x1".into()));
    assert!(!mutate(&|i| i.stale_outcome = Some(
        starknet_wallet_core::send::multisig_intent::StaleOutcome::Reverted
    )));
    // Unknown keys (a private multisig's `proofFacts` among them) do not read.
    let mut text: Value = serde_json::to_value(&intent).unwrap();
    text["prepared"]["proofFacts"] = json!(["0x1"]);
    assert!(serde_json::from_value::<SigningIntent>(text).is_err());
}

#[test]
fn a_merge_keeps_a_settled_intent_and_adds_only_new_signers() {
    let ready = ready_intent();
    let mut stale = ready.clone();
    stale.state = IntentState::Stale;
    let mut fewer = ready.clone();
    fewer.signatures.truncate(1);
    fewer.state = IntentState::Collecting;
    assert_eq!(stale.merge_imported(&fewer).unwrap(), stale);
    // Re-importing a signer already held changes nothing but the derived state.
    let merged = fewer.merge_imported(&fewer).unwrap();
    assert_eq!(
        (merged.signatures.len(), merged.state),
        (1, IntentState::Collecting)
    );
    let merged = fewer.merge_imported(&ready).unwrap();
    assert_eq!(merged, ready);
    // The local summary wins over an imported one.
    let mut renamed = ready.clone();
    renamed.summary = "Something else".into();
    assert_eq!(
        fewer.merge_imported(&renamed).unwrap().summary,
        ready.summary
    );
}
