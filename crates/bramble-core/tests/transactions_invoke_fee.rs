//! A call batch's fee affordability: `resolveFeeAffordability` as the dapp invoke card asks it
//! (`estimateInvokeApprovalFee`, `apps/extension/src/background/approvals/invoke-fee-review.ts`),
//! over the STRK balance the host read. The verdict is fee-only, even when the calls spend STRK;
//! an undeployed account's total adds the deployment; a non-FRI fee or no balance is no claim.
//! Ported from `packages/features/transactions/tests/routing/fee-funds.test.ts` and
//! `background-invoke-approval.test.ts` ("the verdict rides the fee review").

mod plan_support;

use plan_support::node::*;
use plan_support::*;
use starknet_wallet_core::catalog::TokenInfo;
use starknet_wallet_core::send::call_decode::Known;
use starknet_wallet_core::send::calls::Call;
use starknet_wallet_core::send::plan::{Invoke, Kind, Plan};
use starknet_wallet_core::send::review_flags::ReviewContext;
use starknet_wallet_core::send::types::{FeeAffordability, SendStep, StepReply, TransferReview};
use starknet_wallet_core::send::u256::U256;
use starknet_wallet_core::Felt;

/// A batch from the OpenZeppelin account that spends `amount` STRK, reviewed against `node`,
/// with `balance` as the host's last STRK read.
fn review(node: &mut Node, amount: u128, balance: Option<U256>) -> TransferReview {
    let host = Host::new();
    let account = oz_account(&host);
    let tokens = [TokenInfo {
        address: STRK.into(),
        decimals: 18,
        name: "Starknet Token".into(),
        symbol: "STRK".into(),
    }];
    let context = ReviewContext {
        known: Known {
            tokens: &tokens,
            nft_contracts: &[],
        },
        ..ReviewContext::default()
    };
    let calls = vec![Call {
        contract_address: f(STRK),
        entrypoint: "transfer".into(),
        calldata: vec![f(ACCOUNT_1), Felt::from(amount), Felt::ZERO],
    }];
    let policy = bramble_core::SEND_POLICY;
    let mut batch = Invoke::reviewed(&account, calls, &policy, &context).unwrap();
    if let Some(balance) = balance {
        batch = batch.with_fee_balance(balance);
    }
    let mut plan = Plan::new(account, Kind::Invoke(Box::new(batch)), policy);
    let first = plan.advance(&host, StepReply::Start).unwrap();
    assert_eq!(
        run_until(&mut plan, &host, node, first, never),
        SendStep::Review
    );
    plan.current_review().unwrap().clone()
}

fn spend(review: &TransferReview) -> U256 {
    let fee = U256::from_decimal(&review.fee.as_ref().unwrap().max_spend).unwrap();
    match &review.deployment_fee {
        Some(deploy) => fee
            .checked_add(U256::from_decimal(&deploy.max_spend).unwrap())
            .unwrap(),
        None => fee,
    }
}

fn verdict(deployed: bool, amount: u128, balance: impl Fn(U256) -> U256) -> FeeAffordability {
    let fee = spend(&review(&mut Node::sepolia(deployed), amount, None));
    review(&mut Node::sepolia(deployed), amount, Some(balance(fee))).affordability
}

fn plus(n: u128) -> impl Fn(U256) -> U256 {
    move |fee| fee.checked_add(U256::from_u128(n)).unwrap()
}

fn minus(n: u128) -> impl Fn(U256) -> U256 {
    move |fee| fee.checked_sub(U256::from_u128(n)).unwrap()
}

#[test]
fn a_balance_at_or_above_the_fee_is_sufficient_and_below_it_insufficient() {
    assert_eq!(verdict(true, 1, plus(1)), FeeAffordability::Sufficient);
    assert_eq!(verdict(true, 1, plus(0)), FeeAffordability::Sufficient);
    assert_eq!(verdict(true, 1, minus(1)), FeeAffordability::Insufficient);
}

#[test]
fn the_strk_a_batch_spends_does_not_count() {
    // The card's own verdict stays fee-only; only the gasless offer counts the calls' STRK.
    let amount = 1_500_000_000_000_000_000;
    assert_eq!(verdict(true, amount, plus(0)), FeeAffordability::Sufficient);
}

#[test]
fn an_undeployed_account_must_cover_the_deployment_too() {
    let shown = review(&mut Node::sepolia(false), 1, None);
    assert!(shown.requires_deployment && shown.deployment_fee.is_some());
    let invoke_only = U256::from_decimal(&shown.fee.as_ref().unwrap().max_spend).unwrap();
    let shown = review(&mut Node::sepolia(false), 1, Some(invoke_only));
    assert_eq!(shown.affordability, FeeAffordability::Insufficient);
    assert_eq!(verdict(false, 1, plus(0)), FeeAffordability::Sufficient);
}

#[test]
fn no_balance_no_fee_or_a_wei_fee_makes_no_claim() {
    let mut node = Node::sepolia(true);
    assert_eq!(
        review(&mut node, 1, None).affordability,
        FeeAffordability::Unknown
    );

    let mut failed = Node::sepolia(true);
    failed.invoke_estimate = serde_json::json!({"unit": "FRI"});
    let shown = review(&mut failed, 1, Some(U256::ZERO));
    assert!(shown.fee_estimate_failed);
    assert_eq!(shown.affordability, FeeAffordability::Unknown);

    let mut wei = Node::sepolia(true);
    wei.invoke_estimate["unit"] = "WEI".into();
    let shown = review(&mut wei, 1, Some(U256::ZERO));
    assert!(shown.fee.is_some());
    assert_eq!(shown.affordability, FeeAffordability::Unknown);
}
