//! The canned transfer, prices and paymaster replies the gasless export tests share.
#![allow(dead_code, unused_imports)]

pub use super::send_support::*;

use std::sync::Arc;

use bramble_core::gasless::GASLESS_POLICY;
use bramble_core::{
    activity_fee_text, activity_needs_observation, ActivityKind, DisplayCurrency, GaslessBalance,
    GaslessFee, GaslessFeeError, GaslessOffer, GaslessOfferStep, GaslessSubmitStep, HttpOutcome,
    HttpRequest, NetworkId, PricePoint, PriceView, SendError, SendStep, StepReply,
    TransferDirection, TransferPlan, TransferRequest, WalletCore,
};
use serde_json::{json, Value};
use starknet_wallet_core::{felt, kms, Felt};

pub const ETH: &str = "0x49d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7";
pub const USDC: &str = "0x33068f6539f8e6e6b131e6b2b814e6c34a5224bc66947c47dab9dfee93b35fb";
pub const NOW_S: u64 = NOW / 1000;
pub const TYPES: &str = r#"{"StarknetDomain":[{"name":"name","type":"shortstring"},{"name":"version","type":"shortstring"},{"name":"chainId","type":"shortstring"},{"name":"revision","type":"shortstring"}],"OutsideExecution":[{"name":"Caller","type":"ContractAddress"},{"name":"Nonce","type":"felt"},{"name":"Execute After","type":"u128"},{"name":"Execute Before","type":"u128"},{"name":"Calls","type":"Call*"}],"Call":[{"name":"To","type":"ContractAddress"},{"name":"Selector","type":"selector"},{"name":"Calldata","type":"felt*"}]}"#;

pub fn forwarder() -> String {
    felt::canonical(&GASLESS_POLICY.ready_route(NetworkId::Mainnet).unwrap().1)
}

pub fn account_id(core: &WalletCore) -> String {
    oz_id(core, NetworkId::Mainnet, 0)
}

pub fn address(core: &WalletCore) -> Felt {
    felt::parse(account_id(core).split_once(':').unwrap().1).unwrap()
}

/// 0.001 ETH to account 1 from Mainnet account 0, with `strk` base units of STRK to pay with.
pub fn transfer(core: &Arc<WalletCore>, recipient: &str, strk: &str) -> Arc<TransferPlan> {
    transfer_from(core, recipient, strk, true)
}

pub fn transfer_from(
    core: &Arc<WalletCore>,
    recipient: &str,
    strk: &str,
    deployed: bool,
) -> Arc<TransferPlan> {
    transfer_on(
        core,
        recipient,
        strk,
        &mut Node::new(NetworkId::Mainnet, deployed),
    )
}

pub fn transfer_on(
    core: &Arc<WalletCore>,
    recipient: &str,
    strk: &str,
    node: &mut Node,
) -> Arc<TransferPlan> {
    let request = TransferRequest {
        network: NetworkId::Mainnet,
        account_id: account_id(core),
        token_address: ETH.into(),
        recipient: recipient.into(),
        amount: "0.001".into(),
        fee_balance: Some(strk.into()),
        tracked_tokens: vec![],
    };
    let plan = core.clone().prepare_transfer(request).unwrap();
    let first = plan.advance(StepReply::Start).unwrap();
    assert_eq!(run_until(&plan, core, node, first, never), SendStep::Review);
    plan
}

pub fn prices() -> PriceView {
    PriceView {
        currency: DisplayCurrency::Usd,
        prices: vec![
            PricePoint {
                address: STRK.into(),
                value: 0.5,
            },
            PricePoint {
                address: USDC.into(),
                value: 1.0,
            },
        ],
        changes: vec![],
        as_of_ms: Some(NOW as f64),
        stale: false,
    }
}

pub fn balances() -> Vec<GaslessBalance> {
    vec![GaslessBalance {
        token_address: USDC.into(),
        balance: Some("5000".into()),
    }]
}

pub fn result(value: Value) -> HttpOutcome {
    let body = json!({"id": 1, "jsonrpc": "2.0", "result": value}).to_string();
    HttpOutcome {
        status: 200,
        retry_after: None,
        body: Some(body.into_bytes()),
        timed_out: false,
    }
}

pub fn selector(name: &str) -> String {
    felt::canonical(&kms::selector(name))
}

/// The gateway's honest body: the reviewed transfer, then `fee` USDC base units to the forwarder.
pub fn typed_data(amount: &str, fee: &str) -> Value {
    let user = json!({"To": ETH, "Selector": selector("transfer"),
        "Calldata": [ACCOUNT_1, amount, "0x0"]});
    let fee = json!({"To": USDC, "Selector": selector("transfer"),
        "Calldata": [forwarder(), fee, "0x0"]});
    json!({
        "types": serde_json::from_str::<Value>(TYPES).unwrap(),
        "primaryType": "OutsideExecution",
        "domain": {"name": "Account.execute_from_outside", "version": "2", "chainId": "SN_MAIN",
            "revision": "1"},
        "message": {"Caller": forwarder(), "Nonce": "0x5", "Execute After": "0x1",
            "Execute Before": format!("{:#x}", NOW_S + 600), "Calls": [user, fee]},
    })
}

pub fn built(typed: &Value) -> HttpOutcome {
    result(json!({
        "fee": {
            "estimated_fee_in_gas_token": "0x3e8",
            "estimated_fee_in_strk": "0x1",
            "gas_token_price_in_strk": "0x1",
            "suggested_max_fee_in_gas_token": "0x7d0",
            "suggested_max_fee_in_strk": "0x1",
        },
        "parameters": {"fee_mode": {"gas_token": USDC, "mode": "default"}, "version": "0x1"},
        "type": "invoke",
        "typed_data": typed,
    }))
}

/// 0.001 ETH in base units, as the transfer's calldata carries it.
pub const AMOUNT: &str = "0x38d7ea4c68000";

pub fn method(request: &HttpRequest) -> Value {
    let body: Value = serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
    body["method"].clone()
}

/// The round over a paymaster that offers USDC: the fee object and its one offer.
pub fn offered(plan: &Arc<TransferPlan>) -> (Arc<GaslessFee>, GaslessOffer) {
    let started = plan
        .clone()
        .gasless_offers(Some(prices()), balances(), NOW)
        .expect("a gasless choice");
    let replies = [
        result(json!(true)),
        result(json!([{"decimals": 6, "token_address": USDC}])),
        built(&typed_data(AMOUNT, "0x3e8")),
    ];
    let mut step = started.step;
    for (reply, expected) in replies.into_iter().zip([
        "paymaster_isAvailable",
        "paymaster_getSupportedTokens",
        "paymaster_buildTransaction",
    ]) {
        let GaslessOfferStep::Post { post } = step else {
            panic!("expected {expected}, got {step:?}");
        };
        assert_eq!(method(&post.request), expected);
        step = started.fee.accept_offer_reply(reply).unwrap();
    }
    let GaslessOfferStep::Done { offers } = step else {
        panic!("the round did not end: {step:?}");
    };
    assert_eq!(offers.len(), 1);
    (started.fee, offers[0].clone())
}

pub fn post(step: GaslessSubmitStep) -> HttpRequest {
    match step {
        GaslessSubmitStep::Post { post } => post.request,
        other => panic!("expected a post, got {other:?}"),
    }
}
