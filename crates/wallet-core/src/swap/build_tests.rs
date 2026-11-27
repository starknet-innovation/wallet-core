//! The build request, its reply judge and the preparation, against the extension's own cases
//! (`avnu.test.ts`, `swap-ops.test.ts`, `swap-route-binding.test.ts`), one guard per case.

use super::super::fixtures::*;
use super::super::BuildPolicyError;
use super::*;
use crate::send::invoke::InvokeRefusal;
use crate::NetworkId;

fn prepared(slippage: Option<f64>) -> Result<(PendingBuild, HttpRequest), SwapRefusal> {
    quote().prepare(&account(), slippage)
}

fn judge_with(status: u16, body: &str, send: &Policy) -> Result<SwapPreparation, SwapRefusal> {
    let tokens = tokens();
    let (build, _) = prepared(None).unwrap();
    let context = ReviewContext {
        known: known(&tokens),
        ..ReviewContext::default()
    };
    build.judge(&outcome(status, body), &context, send)
}

fn judge(body: &str) -> Result<SwapPreparation, SwapRefusal> {
    judge_with(200, body, &SEND_POLICY)
}

fn honest() -> String {
    build_reply(&[approve_router(), route_call()])
}

/// `honest()` with `edit` applied to the first call's JSON.
fn with_first_call(edit: impl FnOnce(&mut serde_json::Value)) -> String {
    let mut reply: serde_json::Value = serde_json::from_str(&honest()).unwrap();
    edit(&mut reply["calls"][0]);
    reply.to_string()
}

#[test]
fn the_build_request_asks_for_the_approval_at_the_reviewed_slippage() {
    let (_, request) = prepared(None).unwrap();
    assert_eq!(request.method, HttpMethod::Post);
    assert_eq!(request.url, "https://starknet.api.avnu.fi/swap/v3/build");
    assert_eq!(
        request.headers,
        vec![HttpHeader {
            name: "content-type".into(),
            value: "application/json".into()
        }]
    );
    assert_eq!(
        request.body.as_deref(),
        Some(&*format!(
            r#"{{"includeApprove":true,"quoteId":"{QUOTE_ID}","slippage":0.005,"takerAddress":"{ACCOUNT}"}}"#
        ))
    );
    assert_eq!(request.max_body_bytes, 1024 * 1024);
    let (_, request) = prepared(Some(0.05)).unwrap();
    assert!(request.body.unwrap().contains(r#""slippage":0.05,"#));
}

#[test]
fn slippage_and_quote_id_outside_their_rules_are_refused() {
    for slippage in [0.0, -0.01, 0.2, 0.5, f64::NAN, f64::INFINITY] {
        assert_eq!(
            prepared(Some(slippage)).map(|_| ()),
            Err(SwapRefusal::InvalidPrepareRequest)
        );
    }
    // The quote judge takes any id up to the length; preparation takes only the grammar.
    let evil = quote_from({
        let mut reply = quote_json();
        reply["quoteId"] = "../evil".into();
        reply
    });
    assert_eq!(
        evil.prepare(&account(), None).map(|_| ()),
        Err(SwapRefusal::InvalidPrepareRequest)
    );
}

#[test]
fn a_quote_with_no_floor_is_not_reviewable() {
    let zero = quote_from({
        let mut reply = quote_json();
        reply["buyAmount"] = "0x0".into();
        reply
    });
    assert_eq!(
        zero.prepare(&account(), None).map(|_| ()),
        Err(SwapRefusal::NotReviewable)
    );
}

#[test]
fn a_quote_prepares_only_for_its_own_account_and_network() {
    let other = Account {
        address: felt(ATTACKER),
        ..account()
    };
    let sepolia = Account {
        network: NetworkId::Sepolia,
        ..account()
    };
    for account in [other, sepolia] {
        assert_eq!(
            quote().prepare(&account, None).map(|_| ()),
            Err(SwapRefusal::PreparationChanged)
        );
    }
}

#[test]
fn the_build_envelope_is_judged_before_any_call() {
    let chain = |chain_id: &str| {
        let mut reply: serde_json::Value = serde_json::from_str(&honest()).unwrap();
        reply["chainId"] = chain_id.into();
        reply.to_string()
    };
    let sepolia = crate::felt::canonical(&NetworkId::Sepolia.chain_id());
    let five = build_reply(&[
        approve_router(),
        route_call(),
        route_call(),
        route_call(),
        route_call(),
    ]);
    for body in [
        build_reply(&[]),
        five,
        chain("0x1"),
        chain(&sepolia),
        chain("SN_MAIN"),
        r#"{"chainId":"0x534e5f4d41494e"}"#.to_string(),
        "[]".to_string(),
    ] {
        assert_eq!(judge(&body), Err(SwapRefusal::InvalidBuild), "{body}");
    }
}

#[test]
fn every_built_call_must_be_in_its_grammar() {
    let cases: [fn(&mut serde_json::Value); 7] = [
        |call| call["calldata"][0] = "nope".into(),
        |call| call["calldata"] = "0x1".into(),
        |call| call["entrypoint"] = "bad name!".into(),
        |call| call["entrypoint"] = format!("a{}", "b".repeat(64)).into(),
        |call| call["entrypoint"] = "1approve".into(),
        |call| call["contractAddress"] = "0xzz".into(),
        |call| call["calldata"] = vec!["0x1"; 2_001].into(),
    ];
    for edit in cases {
        assert_eq!(judge(&with_first_call(edit)), Err(SwapRefusal::InvalidCall));
    }
    // The longest entrypoint and calldata the grammar admits reach the policy.
    let long_name =
        with_first_call(|call| call["entrypoint"] = format!("a{}", "b".repeat(63)).into());
    assert_eq!(
        judge(&long_name),
        Err(SwapRefusal::Policy(BuildPolicyError::MissingApproval))
    );
    let long_calldata = with_first_call(|call| call["calldata"] = vec!["0x1"; 2_000].into());
    assert_eq!(
        judge(&long_calldata),
        Err(SwapRefusal::Policy(BuildPolicyError::UnexpectedAllowance))
    );
}

#[test]
fn a_failed_build_exchange_never_reaches_the_policy() {
    assert_eq!(
        judge_with(502, &honest(), &SEND_POLICY),
        Err(SwapRefusal::BuildFailed)
    );
    assert_eq!(judge("not json"), Err(SwapRefusal::BuildFailed));
    let padded = format!("{}{}", honest(), " ".repeat(1024 * 1024));
    assert_eq!(judge(&padded), Err(SwapRefusal::BuildFailed));
}

#[test]
fn the_policy_and_the_batch_review_both_judge_the_build() {
    let rogue = build_reply(&[call(
        STRK,
        "approve",
        &["0x1", "0xffffffffffffffffffffffff", "0x0"],
    )]);
    assert_eq!(
        judge(&rogue),
        Err(SwapRefusal::Policy(
            BuildPolicyError::UnexpectedApprovalToken
        ))
    );
    let one_call = Policy {
        max_invoke_calls: 1,
        ..SEND_POLICY
    };
    assert_eq!(
        judge_with(200, &honest(), &one_call),
        Err(SwapRefusal::Invoke(InvokeRefusal::TooManyCalls))
    );
}

#[test]
fn the_honest_build_prepares_with_its_review_and_activity() {
    let preparation = judge(&honest()).unwrap();
    assert_eq!(
        preparation.summaries(),
        [
            "Allow 0x0427…3b0f to spend 0.01 ETH",
            "Swap 0.01 ETH for at least 12.4375 STRK, paid to 0x0123…bcde",
        ]
    );
    assert!(!preparation
        .summaries()
        .join(" ")
        .contains("Call multi_route_swap"));
    assert_eq!(preparation.min_buy_amount_text(), "12.4375");
    assert_eq!(
        preparation.min_buy_amount(),
        U256::from_u128(MIN_BUY_AMOUNT)
    );
    let invoke = preparation.into_invoke(&account()).unwrap();
    assert_eq!(invoke.calls, vec![approve_router(), route_call()]);
}

#[test]
fn a_preparation_is_released_only_to_its_own_account_and_network() {
    let other = Account {
        address: felt(ATTACKER),
        ..account()
    };
    let sepolia = Account {
        network: NetworkId::Sepolia,
        ..account()
    };
    for account in [other, sepolia] {
        assert_eq!(
            judge(&honest()).unwrap().into_invoke(&account).map(|_| ()),
            Err(SwapRefusal::PreparationChanged)
        );
    }
}

#[test]
fn a_prepared_swap_hands_the_plan_its_router_approval_and_its_asset() {
    use crate::allowances::calls::{AllowanceChange, ChangeKind};
    use crate::allowances::record::AllowanceInput;
    use crate::history::explorer::activity_account_id;
    use crate::send::invoke::ActivityAsset;
    use crate::send::plan::PendingKind;

    let invoke = judge(&honest()).unwrap().into_invoke(&account()).unwrap();
    let canonical = |value: &str| crate::felt::canonical(&felt(value));
    let approval = AllowanceChange {
        kind: ChangeKind::Set,
        input: AllowanceInput {
            account_id: activity_account_id(NetworkId::Mainnet, canonical(ACCOUNT)).unwrap(),
            network: NetworkId::Mainnet,
            token: canonical(ETH),
            spender: canonical(ROUTER),
            amount: U256::from_u128(SELL_AMOUNT),
            lower_bound: false,
        },
    };
    let asset = ActivityAsset {
        amount: Some("0.01".into()),
        symbol: "ETH".into(),
        token_address: felt(ETH),
    };
    assert_eq!(
        invoke.pending_kind(),
        PendingKind::Invoke {
            asset: Some(asset),
            counterparty: None,
            allowances: vec![approval],
        }
    );
}

#[test]
fn the_activity_row_is_the_swap_line_not_the_review_line() {
    let invoke = judge(&honest()).unwrap().into_invoke(&account()).unwrap();
    assert_eq!(
        invoke.activity_summary(),
        "Swap 0.01 ETH for ~12.5 STRK via avnu"
    );
    assert_ne!(invoke.activity_summary(), invoke.summary());
}

#[test]
fn the_pinned_router_raises_no_unknown_contract_flag() {
    assert!(judge(&honest()).unwrap().flags().is_empty());
    // The same batch reviewed without the pin reads the router as an unknown contract.
    let tokens = tokens();
    let unpinned = ReviewContext {
        known: known(&tokens),
        ..ReviewContext::default()
    };
    let calls = vec![approve_router(), route_call()];
    let invoke = Invoke::reviewed(&account(), calls, &SEND_POLICY, &unpinned).unwrap();
    let codes: Vec<_> = invoke.flags().iter().map(|flag| flag.code()).collect();
    assert_eq!(codes, ["unknown-contract"]);
}

/// The batch review's poisoned-payee refusal is unreachable from a build: the policy refuses every
/// transfer-family call, and the route's payee must be the account itself, which no lookalike is.
/// So a payee mimicking a trusted address is refused there, before the review.
#[test]
fn a_payee_that_mimics_a_trusted_address_is_refused() {
    use crate::send::calls::is_lookalike_address;
    let mimic = "0x1234000000000000000000000000000000000000000000000000000000bcde";
    assert!(is_lookalike_address(&felt(mimic), &felt(ACCOUNT)));
    let tokens = tokens();
    let trusted = [felt(ACCOUNT)];
    let context = ReviewContext {
        known: known(&tokens),
        trusted: &trusted,
        ..ReviewContext::default()
    };
    let judge = |calls: &[Call]| {
        let (build, _) = prepared(None).unwrap();
        build.judge(&outcome(200, &build_reply(calls)), &context, &SEND_POLICY)
    };
    let route = Route {
        beneficiary: felt(mimic),
        ..Route::honest()
    };
    assert_eq!(
        judge(&[approve_router(), route.call()]).map(|_| ()),
        Err(SwapRefusal::Policy(BuildPolicyError::BeneficiaryMismatch))
    );
    let transfer = call(ETH, "transfer", &[mimic, "0x1", "0x0"]);
    assert_eq!(
        judge(&[approve_router(), route_call(), transfer]).map(|_| ()),
        Err(SwapRefusal::Policy(BuildPolicyError::DirectTransfer))
    );
}
