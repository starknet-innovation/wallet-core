//! `gasless-submit.test.ts`, `gasless-spend.test.ts` and the offer round's per-token judgement.

use super::*;
use crate::accounts::AccountFamily;
use crate::gasless::tests::{call, felt_of, v2_classes, POLICY};
use crate::gasless::typed_data::tests::{quote, standard, BEFORE, FORWARDER, NOW, RECIPIENT, USDC};
use crate::gasless::{strk_affordability, GaslessAccount};
use crate::gasless::{GaslessPolicy, GaslessRoute};
use crate::js::json;
use crate::send::fees::FeeUnit;
use crate::send::sign::Deployment;
use crate::NetworkId;

const ACCOUNT: &str = "0x5dccf9cfdcabb9e4ef45814bda1b056bb23063d0e9b64e5a845e9dfb3d23f10";
const ARGENT_04: &str = "0x36078334509b514626504edc9fb252328d1a240e4e948bef8d0c08dff45927f";
const STRK: &str = "0x4718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d";

fn session(calls: &[crate::send::calls::Call]) -> GaslessSession<'static> {
    let account = GaslessAccount {
        network: NetworkId::Mainnet,
        address: felt_of(ACCOUNT),
        family: AccountFamily::Argent,
        class_hash: felt_of(ARGENT_04),
        deployed: true,
        deployment: None,
    };
    GaslessSession::open(&POLICY, &account, calls, &v2_classes()).unwrap()
}

fn user_calls() -> Vec<crate::send::calls::Call> {
    vec![call(USDC, "transfer", &[RECIPIENT, "0x10", "0x0"])]
}

/// `quoteBody()`: a build result whose fee transfer is 1000 USDC base units.
fn quote_body() -> JsValue {
    quote_body_of("invoke", standard())
}

fn quote_body_of(kind: &str, typed_data: serde_json::Value) -> JsValue {
    let body = serde_json::json!({
        "fee": {
            "estimated_fee_in_gas_token": "0x1",
            "estimated_fee_in_strk": "0x1",
            "gas_token_price_in_strk": "0x1",
            "suggested_max_fee_in_gas_token": "0x3e8",
            "suggested_max_fee_in_strk": "0x1",
        },
        "parameters": {"fee_mode": {"gas_token": USDC, "mode": "default"}, "version": "0x1"},
        "type": kind,
        "typed_data": typed_data,
    });
    json::parse(&body.to_string()).unwrap()
}

fn reviewed(amount: &str) -> GaslessFeeOffer {
    GaslessFeeOffer {
        amount: amount.into(),
        decimals: 6,
        label: "USDC".into(),
        token_address: USDC.into(),
    }
}

fn checked(session: &GaslessSession<'static>) -> CheckedQuote<'static> {
    let (pending, request) = session
        .submit_build(&reviewed("2000"), Some(U256::from_u128(1000)), NOW)
        .unwrap();
    assert!(request.body.unwrap().contains("paymaster_buildTransaction"));
    pending.judge(session, &quote_body()).unwrap()
}

#[test]
fn refuses_an_expired_window_read_after_signing() {
    let session = session(&user_calls());
    let quote = checked(&session);
    assert_eq!(
        quote.execute_request(&[felt_of("0x1"), felt_of("0x2")], BEFORE),
        Err(GaslessCode::InvalidScreeningTransaction.into())
    );
}

#[test]
fn refuses_a_window_longer_than_ten_minutes_after_signing() {
    let session = session(&user_calls());
    let quote = checked(&session);
    assert_eq!(
        quote.execute_request(&[felt_of("0x1"), felt_of("0x2")], BEFORE - 601),
        Err(GaslessCode::InvalidScreeningTransaction.into())
    );
}

#[test]
fn executes_the_gateways_body_unchanged_and_records_the_signed_quote() {
    let session = session(&user_calls());
    let quote = checked(&session);
    assert_eq!(
        quote.user_fee(),
        &UserFee {
            amount: "1000".into(),
            token: "USDC".into(),
            token_address: USDC.into()
        }
    );
    let request = quote
        .execute_request(&[felt_of("0x1"), felt_of("0x2")], NOW)
        .unwrap();
    let typed = json::parse(&standard().to_string()).unwrap().to_json();
    assert_eq!(
        request.body.as_deref(),
        Some(
            format!(
                r#"{{"id":1,"jsonrpc":"2.0","method":"paymaster_executeTransaction","params":{{"parameters":{{"fee_mode":{{"gas_token":"{USDC}","mode":"default"}},"time_bounds":{{"execute_after":1,"execute_before":{}}},"version":"0x1"}},"transaction":{{"invoke":{{"signature":["0x1","0x2"],"typed_data":{typed},"user_address":"{ACCOUNT}"}},"type":"invoke"}}}}}}"#,
                NOW + 600
            )
            .as_str()
        )
    );
    let answer = GaslessAnswer::Result(json::parse(r#"{"transaction_hash":"0x3"}"#).unwrap());
    let submitted = quote.judge_execute(answer).unwrap();
    assert_eq!(submitted.transaction_hash, felt_of("0x3"));
    assert_eq!(submitted.user_fee.amount, "1000");
    let hashless = GaslessAnswer::Result(JsValue::Null);
    assert_eq!(quote.judge_execute(hashless), Err(GaslessError::Lost));
}

#[test]
fn refuses_a_fresh_fee_above_the_reviewed_amount() {
    let session = session(&user_calls());
    let (pending, _) = session
        .submit_build(&reviewed("999"), Some(U256::from_u128(1000)), NOW)
        .unwrap();
    assert_eq!(
        pending.judge(&session, &quote_body()),
        Err(GaslessError::ReviewedFeeExceeded)
    );
}

// The core holds the build between the host's exchanges, so its reply is judged only for the
// account and network it was asked for, never a session the host swapped in.
#[test]
fn judges_a_build_only_for_its_own_account_and_network() {
    let session = session(&user_calls());
    let mut other_account = session.clone();
    other_account.account = felt_of(RECIPIENT);
    let mut other_network = session.clone();
    other_network.network = NetworkId::Sepolia;
    let strk = felt_of(STRK);
    let held = GaslessHeldToken {
        address: felt_of(USDC),
        balance: Some(U256::from_u128(5000)),
        decimals: 6,
        label: "USDC".into(),
    };
    for other in [&other_account, &other_network] {
        let (pending, _) = session
            .submit_build(&reviewed("2000"), Some(U256::from_u128(1000)), NOW)
            .unwrap();
        assert_eq!(
            pending.judge(other, &quote_body()).err(),
            Some(GaslessCode::InvalidGaslessRequest.into())
        );
        let (pending, _) = session
            .offer_build(&held, Some(U256::from_u128(1000)), NOW)
            .unwrap();
        assert_eq!(pending.judge(other, &quote_body(), &strk), None);
    }
    let (pending, _) = session
        .offer_build(&held, Some(U256::from_u128(1000)), NOW)
        .unwrap();
    assert!(pending.judge(&session, &quote_body(), &strk).is_some());
}

#[test]
fn maps_each_body_refusal_to_the_gateways_code_and_needs_a_reference() {
    let session = session(&user_calls());
    let build = |reference: u128| {
        session
            .submit_build(&reviewed("5000"), Some(U256::from_u128(reference)), NOW)
            .unwrap()
            .0
    };
    // 7/2 of 285 is 998, under the 1000 transfer.
    assert_eq!(
        build(285).judge(&session, &quote_body()),
        Err(GaslessCode::AboveCeiling.into())
    );
    assert_eq!(
        build(1000).judge(&session, &JsValue::Null),
        Err(GaslessCode::InvalidGaslessRequest.into())
    );
    assert_eq!(
        session.submit_build(&reviewed("5000"), None, NOW).err(),
        Some(GaslessCode::Unverifiable.into())
    );
}

#[test]
fn an_offer_needs_the_balance_to_cover_the_fee_beside_the_batchs_own_spend() {
    let session = session(&user_calls());
    let strk = felt_of(STRK);
    let held = |balance: u128| GaslessHeldToken {
        address: felt_of(USDC),
        balance: Some(U256::from_u128(balance)),
        decimals: 6,
        label: "USDC".into(),
    };
    let offer = |balance: u128| {
        let (pending, _) = session
            .offer_build(&held(balance), Some(U256::from_u128(1000)), NOW)
            .unwrap();
        pending.judge(&session, &quote_body(), &strk)
    };
    // The batch sends 0x10 USDC and the fee is 1000.
    assert_eq!(offer(1016), Some(reviewed("1000")));
    assert_eq!(offer(1015), None);
    assert!(session.offer_build(&held(1016), None, NOW).is_none());
    let listed = ListedToken {
        address: felt_of(USDC),
        decimals: 6,
    };
    assert_eq!(held_for(&listed, &[held(1)], &strk), Some(&held(1)));
    let other_decimals = ListedToken {
        decimals: 18,
        ..listed
    };
    assert_eq!(held_for(&other_decimals, &[held(1)], &strk), None);
}

#[test]
fn counts_strk_the_calls_spend_not_only_the_network_fee() {
    let strk = felt_of(STRK);
    let fee = Some((U256::from_u128(1), FeeUnit::Fri));
    let transfer = |amount: &str| call(STRK, "transfer", &["0x2", amount, "0x0"]);
    let verdict = |calls: &[_], balance: u128| {
        strk_affordability(calls, &strk, Some(U256::from_u128(balance)), fee)
    };
    assert_eq!(
        verdict(&[transfer("0x64")], 100),
        FeeAffordability::Insufficient
    );
    assert_eq!(
        verdict(&[transfer("0x64")], 101),
        FeeAffordability::Sufficient
    );
    assert_eq!(verdict(&[], 1), FeeAffordability::Sufficient);
    let unknown = call(STRK, "unknown_spend", &["0x1"]);
    assert_eq!(verdict(&[unknown], 1000), FeeAffordability::Unknown);
    let approve = call(STRK, "approve", &[FORWARDER, "0x64", "0x0"]);
    assert_eq!(verdict(&[approve], 1), FeeAffordability::Sufficient);
    let wei = Some((U256::from_u128(1), FeeUnit::Wei));
    assert_eq!(
        strk_affordability(&[], &strk, Some(U256::from_u128(1)), wei),
        FeeAffordability::Unknown
    );
}

#[test]
fn refuses_to_execute_without_a_signature() {
    let session = session(&user_calls());
    let quote = checked(&session);
    assert_eq!(
        quote.execute_request(&[], NOW),
        Err(GaslessCode::Unavailable.into())
    );
}

/// The standard body's calls on `SN_SEPOLIA`.
fn sepolia_typed_data() -> serde_json::Value {
    let calls = standard()["message"]["Calls"].clone();
    quote(calls.as_array().unwrap(), "SN_SEPOLIA")
}

// The chain id check would refuse a Mainnet body under a Sepolia session, so the body here is
// Sepolia's: only the build's own network refuses it.
#[test]
fn judges_a_build_only_for_its_own_network_even_with_a_body_for_the_other() {
    let session = session(&user_calls());
    let mut sepolia = session.clone();
    sepolia.network = NetworkId::Sepolia;
    let body = quote_body_of("invoke", sepolia_typed_data());
    let (pending, _) = session
        .submit_build(&reviewed("2000"), Some(U256::from_u128(1000)), NOW)
        .unwrap();
    assert_eq!(
        pending.clone().judge(&sepolia, &body).err(),
        Some(GaslessCode::InvalidGaslessRequest.into())
    );
    let strk = felt_of(STRK);
    let held = GaslessHeldToken {
        address: felt_of(USDC),
        balance: Some(U256::from_u128(5000)),
        decimals: 6,
        label: "USDC".into(),
    };
    let (offer, _) = session
        .offer_build(&held, Some(U256::from_u128(1000)), NOW)
        .unwrap();
    assert_eq!(offer.clone().judge(&sepolia, &body, &strk), None);
    // The same body is accepted for a build asked for on Sepolia.
    let mut built_on_sepolia = pending;
    built_on_sepolia.bound.network = NetworkId::Sepolia;
    assert!(built_on_sepolia.judge(&sepolia, &body).is_ok());
}

#[test]
fn a_deploy_and_invoke_executes_with_the_builds_deployment() {
    const ROUTES: [GaslessRoute; 1] = [GaslessRoute {
        network: NetworkId::Sepolia,
        url: "https://gateway.example/sepolia",
        enabled: true,
        forwarder: Some(Felt::from_hex_unchecked(FORWARDER)),
    }];
    let policy = GaslessPolicy {
        routes: &ROUTES,
        ..POLICY
    };
    let deployment = Deployment {
        class_hash: felt_of("0x4"),
        salt: felt_of("0x3"),
        constructor_calldata: vec![felt_of("0x5")],
    };
    let address = deployment.address().unwrap();
    let account = GaslessAccount {
        network: NetworkId::Sepolia,
        address,
        family: AccountFamily::Argent,
        class_hash: felt_of(ARGENT_04),
        deployed: false,
        deployment: Some(&deployment),
    };
    let calls = user_calls();
    let session = GaslessSession::open(&policy, &account, &calls, &v2_classes()).unwrap();
    let (pending, _) = session
        .submit_build(&reviewed("2000"), Some(U256::from_u128(1000)), NOW)
        .unwrap();
    let body = quote_body_of("deploy_and_invoke", sepolia_typed_data());
    let quote = pending.judge(&session, &body).unwrap();
    let request = quote.execute_request(&[felt_of("0x1")], NOW).unwrap();
    let sent: serde_json::Value = serde_json::from_str(request.body.as_deref().unwrap()).unwrap();
    let transaction = &sent["params"]["transaction"];
    assert_eq!(transaction["type"], "deploy_and_invoke");
    assert_eq!(
        transaction["deployment"],
        serde_json::json!({
            "address": felt::canonical(&address),
            "calldata": ["0x5"],
            "class_hash": "0x4",
            "salt": "0x3",
            "version": 1,
        })
    );
    assert_eq!(
        transaction["invoke"]["user_address"],
        felt::canonical(&address)
    );
}

#[test]
fn strk_is_never_held_for_a_gasless_fee() {
    let strk = felt_of(STRK);
    let listed = ListedToken {
        address: strk,
        decimals: 18,
    };
    let held = GaslessHeldToken {
        address: strk,
        balance: Some(U256::from_u128(1)),
        decimals: 18,
        label: "STRK".into(),
    };
    assert_eq!(held_for(&listed, &[held], &strk), None);
}
