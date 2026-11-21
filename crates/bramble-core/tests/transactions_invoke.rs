//! A call batch (`Kind::Invoke`) through `send::plan::Plan`. No Bramble generator records one yet,
//! so the batch's own path is pinned two ways: every transfer capture driven as a one-call batch
//! must put the same requests on the wire and sign the same hashes as the transfer kind (the
//! extension's `submitErc20Transfer` is `submitInvokeTransaction` with one call), and a scripted
//! node drives a multicall, its refusals and its failures.

mod multisig_support;
mod plan_support;

use multisig_support::TestPrimitives;
use plan_support::node::*;
use plan_support::*;
use serde_json::{json, Value};
use starknet_wallet_core::catalog::TokenInfo;
use starknet_wallet_core::send::call_decode::Known;
use starknet_wallet_core::send::calls::Call;
use starknet_wallet_core::send::multisig_invoke::StarkPrimitives;
use starknet_wallet_core::send::multisig_review::CallReviewError;
use starknet_wallet_core::send::plan::{
    Account, Authorization, Invoke, InvokeRefusal, Kind, PendingKind, Plan, Policy,
};
use starknet_wallet_core::send::review_flags::{ReviewContext, RiskFlag};
use starknet_wallet_core::send::types::{
    FeeAffordability, SendError, SendFailureCode as Code, SendStep, StepReply,
};
use starknet_wallet_core::{felt, Felt, NetworkId};

/// 1.5 STRK in base units.
const AMOUNT: u128 = 1_500_000_000_000_000_000;

fn strk() -> TokenInfo {
    TokenInfo {
        address: STRK.into(),
        decimals: 18,
        name: "Starknet Token".into(),
        symbol: "STRK".into(),
    }
}

fn invoke(account: &Account, calls: Vec<Call>, trusted: &[Felt]) -> Result<Invoke, InvokeRefusal> {
    let tokens = [strk()];
    let context = ReviewContext {
        known: Known {
            tokens: &tokens,
            nft_contracts: &[],
        },
        trusted,
        ..ReviewContext::default()
    };
    Invoke::reviewed(
        account,
        calls,
        &bramble_core::SEND_POLICY,
        &context,
        &TestPrimitives::new(),
    )
}

fn call(contract: &str, entrypoint: &str, calldata: Vec<Felt>) -> Call {
    Call {
        contract_address: f(contract),
        entrypoint: entrypoint.into(),
        calldata,
    }
}

fn transfer_call(recipient: &str) -> Call {
    call(
        STRK,
        "transfer",
        vec![f(recipient), Felt::from(AMOUNT), Felt::ZERO],
    )
}

fn batch_plan(host: &Host, calls: Vec<Call>) -> Plan {
    let account = oz_account(host);
    let batch = invoke(&account, calls, &[]).unwrap();
    Plan::new(
        account,
        Kind::Invoke(Box::new(batch)),
        bramble_core::SEND_POLICY,
    )
}

/// Starts `plan` and answers it from `node` up to the review.
fn review(plan: &mut Plan, host: &Host, node: &mut Node) {
    let first = plan.advance(host, StepReply::Start).unwrap();
    assert_eq!(run_until(plan, host, node, first, never), SendStep::Review);
}

fn two_calls() -> Vec<Call> {
    vec![
        call(STRK, "approve", vec![f(ACCOUNT_1), Felt::ONE, Felt::ZERO]),
        call(
            ETH,
            "transfer",
            vec![f(ACCOUNT_1), Felt::from(7u8), Felt::ZERO],
        ),
    ]
}

#[test]
fn a_one_call_batch_puts_every_transfer_capture_on_the_wire() {
    let mut count = 0;
    for (name, family) in [
        ("wire", "openzeppelin"),
        ("argent", "argent"),
        ("argent-upgraded", "argent"),
        ("braavos", "braavos"),
    ] {
        for scenario in capture(name)["scenarios"].as_array().unwrap() {
            // Same requests, same signed hashes in the same order, same transaction hash.
            run_with(scenario, family, |scenario: &Value, account: Account| {
                if scenario["flow"] == "deployment" {
                    return plan(scenario, account);
                }
                let recipient = scenario["recipient"].as_str().unwrap_or(ACCOUNT_1);
                let batch = invoke(&account, vec![transfer_call(recipient)], &[]).unwrap();
                Plan::new(
                    account,
                    Kind::Invoke(Box::new(batch)),
                    bramble_core::SEND_POLICY,
                )
            });
            count += 1;
        }
    }
    assert_eq!(count, 20);
}

#[test]
fn a_multicall_signs_and_submits_every_call() {
    // `rpc-network-ops.test.ts`, "signs and submits a standard-speed invoke": the hash the node
    // accepts is the one signed, here for a batch of two.
    let host = Host::new();
    let mut node = Node::sepolia(true);
    let mut plan = batch_plan(&host, two_calls());
    review(&mut plan, &host, &mut node);
    let shown = plan.current_review().unwrap().clone();
    assert_eq!(
        shown.summary,
        "Allow 0x3fa7…41a3 to spend 0.000000000000000001 STRK (+1 more)"
    );
    assert!(shown.can_confirm && !shown.requires_deployment && shown.fee.is_some());
    assert_eq!(shown.affordability, FeeAffordability::Unknown);
    assert!(shown.recipient.is_empty() && shown.flags.is_empty());

    let step = plan.advance(&host, StepReply::Confirm).unwrap();
    let write = run_until(&mut plan, &host, &mut node, step, |_, is_write| is_write);
    let (id, method, params, _) = parts(&write);
    assert_eq!(method, "starknet_addInvokeTransaction");
    let primitives = TestPrimitives::new();
    let selector = |name: &str| felt::canonical(&primitives.selector(name).unwrap());
    let expected = json!([
        "0x2",
        STRK,
        selector("approve"),
        "0x3",
        ACCOUNT_1,
        "0x1",
        "0x0",
        ETH,
        selector("transfer"),
        "0x3",
        ACCOUNT_1,
        "0x7",
        "0x0"
    ]);
    assert_eq!(params["invoke_transaction"]["calldata"], expected);

    let signed = latest_hash(&host);
    let accepted = node.reply(&method, &params, Some(signed.clone()));
    assert_eq!(
        plan.advance(&host, reply(id, &accepted)).unwrap(),
        SendStep::Broadcast {
            hash: signed.clone()
        }
    );
    assert_eq!(
        felt::canonical(host.signed.borrow().last().unwrap()),
        signed
    );
    assert_eq!(plan.signatures_made(), 1);
    let records = host.records.borrow();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].kind, PendingKind::Invoke);
    assert_eq!(records[0].summary, shown.summary);
}

#[test]
fn the_plan_shows_the_flags_of_the_batch_it_signs() {
    // An unlimited approval on STRK and a payment through an untracked contract: the card beside
    // the plan's review lists both, from the batch the plan holds.
    let host = Host::new();
    let mut node = Node::sepolia(true);
    let unlimited = call(STRK, "approve", vec![f(ACCOUNT_1), Felt::ONE, Felt::ONE]);
    let mut plan = batch_plan(&host, vec![unlimited, two_calls().remove(1)]);
    review(&mut plan, &host, &mut node);
    assert!(plan.current_review().unwrap().flags.is_empty());
    assert_eq!(
        plan.risk_flags(),
        [
            &RiskFlag::UnlimitedApproval,
            &RiskFlag::UnknownContract { contract: f(ETH) }
        ]
    );
}

#[test]
fn the_builders_activity_text_is_what_the_row_records() {
    // provider-ops.ts: a dapp's batch is recorded as "<origin> submitted N calls", while the
    // review still shows the calls.
    let host = Host::new();
    let mut node = Node::sepolia(true);
    let account = oz_account(&host);
    let batch = invoke(&account, two_calls(), &[])
        .unwrap()
        .with_dapp_origin("https://app.example");
    let mut plan = Plan::new(
        account,
        Kind::Invoke(Box::new(batch)),
        bramble_core::SEND_POLICY,
    );
    review(&mut plan, &host, &mut node);
    let shown = plan.current_review().unwrap().clone();
    assert!(shown.summary.ends_with("(+1 more)"));
    let step = plan.advance(&host, StepReply::Confirm).unwrap();
    let write = run_until(&mut plan, &host, &mut node, step, |_, is_write| is_write);
    let (id, method, params, _) = parts(&write);
    let accepted = node.reply(&method, &params, Some(latest_hash(&host)));
    plan.advance(&host, reply(id, &accepted)).unwrap();
    let records = host.records.borrow();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].summary, "https://app.example submitted 2 calls");
}

#[test]
fn an_undeployed_account_deploys_then_submits_the_batch() {
    let host = Host::new();
    let mut node = Node::sepolia(false);
    let mut plan = batch_plan(&host, two_calls());
    review(&mut plan, &host, &mut node);
    let shown = plan.current_review().unwrap();
    assert!(shown.requires_deployment && shown.deployment_fee.is_some() && shown.can_confirm);
    let step = plan.advance(&host, StepReply::Confirm).unwrap();
    let SendStep::Broadcast { hash } = run_until(&mut plan, &host, &mut node, step, never) else {
        panic!("no broadcast");
    };
    let records = host.records.borrow();
    let kinds: Vec<_> = records.iter().map(|record| record.kind.clone()).collect();
    assert_eq!(kinds, vec![PendingKind::Deployment, PendingKind::Invoke]);
    assert_eq!(felt::canonical(&records[1].hash), hash);
    assert_eq!(plan.signatures_made(), 2);
}

#[test]
fn an_empty_or_poisoned_batch_never_becomes_a_plan() {
    let host = Host::new();
    let account = oz_account(&host);
    assert_eq!(
        invoke(&account, Vec::new(), &[]).unwrap_err(),
        InvokeRefusal::NoCalls
    );
    // A twin of the account itself in a payee position, after an innocent call.
    let lookalike = "0x2083610000000000000000000000000000000000000000000000000000098d9";
    let calls = vec![transfer_call(ACCOUNT_1), transfer_call(lookalike)];
    assert_eq!(
        invoke(&account, calls, &[]).unwrap_err(),
        InvokeRefusal::Review(CallReviewError::PoisonedRecipient)
    );
}

#[test]
fn a_failed_review_estimate_cannot_be_confirmed() {
    let host = Host::new();
    let mut node = Node::sepolia(true);
    node.invoke_estimate = json!({"unit": "FRI"});
    let mut plan = batch_plan(&host, two_calls());
    review(&mut plan, &host, &mut node);
    let shown = plan.current_review().unwrap();
    assert!(shown.fee_estimate_failed && !shown.can_confirm);
    assert_eq!(
        plan.advance(&host, StepReply::Confirm),
        Err(SendError::OutOfOrder)
    );
    assert_eq!(plan.signatures_made(), 0);
}

#[test]
fn a_lost_nonce_read_before_signing_signs_nothing() {
    let host = Host::new();
    let mut node = Node::sepolia(true);
    let mut plan = batch_plan(&host, two_calls());
    review(&mut plan, &host, &mut node);
    let step = plan.advance(&host, StepReply::Confirm).unwrap();
    // The second read at submit is the nonce the INVOKE would be signed with.
    let reads = std::cell::Cell::new(0);
    let nonce = run_until(&mut plan, &host, &mut node, step, |method, _| {
        reads.set(reads.get() + u8::from(method == "starknet_getNonce"));
        reads.get() == 2
    });
    let (id, ..) = parts(&nonce);
    let lost = StepReply::TransportFailure { id, now_ms: NOW };
    let failed = failure(plan.advance(&host, lost).unwrap());
    assert_eq!(
        (failed.code, failed.hash),
        (Code::PreparationUnavailable, None)
    );
    assert_eq!(plan.signatures_made(), 0);
    assert!(host.records.borrow().is_empty());
}

#[test]
fn a_fee_past_the_reviewed_headroom_signs_nothing() {
    let host = Host::new();
    let mut node = Node::sepolia(true);
    let mut plan = batch_plan(&host, two_calls());
    review(&mut plan, &host, &mut node);
    // Twice the L2 gas the review was quoted: past REVIEWED_FEE_HEADROOM (3/2).
    node.invoke_estimate["l2_gas_consumed"] = json!("0x246940");
    let step = plan.advance(&host, StepReply::Confirm).unwrap();
    let failed = failure(run_until(&mut plan, &host, &mut node, step, never));
    assert_eq!(failed.code, Code::ReviewedFeeExceeded);
    assert_eq!(plan.signatures_made(), 0);
}

#[test]
fn a_different_broadcast_hash_is_a_mismatch_carrying_the_signed_hash() {
    let host = Host::new();
    let mut node = Node::sepolia(true);
    let mut plan = batch_plan(&host, two_calls());
    review(&mut plan, &host, &mut node);
    let step = plan.advance(&host, StepReply::Confirm).unwrap();
    let (id, ..) = parts(&run_until(&mut plan, &host, &mut node, step, |_, w| w));
    let signed = latest_hash(&host);
    let other = json!({"result": {"transaction_hash": "0x1"}});
    let failed = failure(plan.advance(&host, reply(id, &other)).unwrap());
    assert_eq!(
        (failed.code, failed.hash),
        (Code::HashMismatch, Some(signed))
    );
    // The signed write's record stays: it may still land.
    assert_eq!(host.records.borrow().len(), 1);
}

#[test]
fn the_plan_holds_a_batch_to_its_own_account_network_and_policy() {
    // A batch reviewed for another account, network or policy is refused before any request:
    // its self-call check, trusted set, known tokens and bounds were that one's.
    let host = Host::new();
    let account = oz_account(&host);
    let refused = |batch: Invoke, policy: Policy| {
        let mut plan = Plan::new(account.clone(), Kind::Invoke(Box::new(batch)), policy);
        let failed = failure(plan.advance(&host, StepReply::Start).unwrap());
        assert_eq!(failed.code, Code::PreparationUnavailable);
        assert_eq!(plan.signatures_made(), 0);
    };
    let mut elsewhere = account.clone();
    elsewhere.address = f(ACCOUNT_1);
    // From ACCOUNT_1's point of view, a call to the plan's own account is an ordinary call.
    let to_self = call(ACCOUNT_0, "upgrade", vec![Felt::ONE]);
    refused(
        invoke(&elsewhere, vec![to_self], &[]).unwrap(),
        bramble_core::SEND_POLICY,
    );
    // Calls this account could send too: only the sender tells them apart.
    refused(
        invoke(&elsewhere, two_calls(), &[]).unwrap(),
        bramble_core::SEND_POLICY,
    );
    // The same address on the other network.
    let mut other_network = account.clone();
    other_network.network = match account.network {
        NetworkId::Mainnet => NetworkId::Sepolia,
        NetworkId::Sepolia => NetworkId::Mainnet,
    };
    refused(
        invoke(&other_network, two_calls(), &[]).unwrap(),
        bramble_core::SEND_POLICY,
    );
    // A plan whose policy is tighter than the one the batch was checked against.
    let tight = Policy {
        max_invoke_calls: 1,
        ..bramble_core::SEND_POLICY
    };
    refused(invoke(&account, two_calls(), &[]).unwrap(), tight);
    assert!(host.signed.borrow().is_empty());
}

#[test]
fn a_self_call_or_an_oversized_batch_never_becomes_a_plan() {
    let host = Host::new();
    let account = oz_account(&host);
    // A padded spelling of the account is the account.
    let padded = format!("0x0{}", &ACCOUNT_0[2..]);
    let to_self = Call {
        contract_address: f(&padded),
        ..call(STRK, "upgrade", vec![Felt::ONE])
    };
    assert_eq!(
        invoke(&account, vec![transfer_call(ACCOUNT_1), to_self], &[]).unwrap_err(),
        InvokeRefusal::Review(CallReviewError::AccountSelfCall)
    );
    assert_eq!(
        invoke(&account, vec![transfer_call(ACCOUNT_1); 17], &[]).unwrap_err(),
        InvokeRefusal::TooManyCalls
    );
    // Bramble's numbers are the extension's `INVOKE_LIMITS`.
    assert_eq!(
        (
            bramble_core::SEND_POLICY.max_invoke_calls,
            bramble_core::SEND_POLICY.max_call_calldata_felts
        ),
        (16, 4_096)
    );
}

#[test]
fn a_multisig_never_signs_a_batch_with_one_key() {
    let host = Host::new();
    let mut account = oz_account(&host);
    let batch = invoke(&account, two_calls(), &[]).unwrap();
    account.authorization = Authorization::ArgentMultisig {
        class_hash: Felt::from(7u8),
        query_signer: account.public_key,
    };
    let mut plan = Plan::new(
        account,
        Kind::Invoke(Box::new(batch)),
        bramble_core::SEND_POLICY,
    );
    let failed = failure(plan.advance(&host, StepReply::Start).unwrap());
    assert_eq!(failed.code, Code::AccountClassUnsupported);
    assert_eq!(plan.signatures_made(), 0);
}
