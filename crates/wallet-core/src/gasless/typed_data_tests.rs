//! `gasless-typed-data.test.ts`, plus the gateway's mainnet fixture hash the extension pins.
//! starknet.js's hashes of the outside executions are `typed_data_vectors.rs`.

use super::*;
use crate::gasless::tests::{call, felt_of, POLICY};
use serde_json::json;

const ACCOUNT: &str = "0x5dccf9cfdcabb9e4ef45814bda1b056bb23063d0e9b64e5a845e9dfb3d23f10";
pub(crate) const FORWARDER: &str =
    "0x127021a1b5a52d3174c2ab077c2b043c80369250d29428cee956d76ee51584f";
const STRK: &str = "0x4718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d";
pub(crate) const USDC: &str = "0x53c91253bc9682c04929ca02ed00b3e423f6710d2ee7e0d5ebb06f3ecf368a8";
pub(crate) const RECIPIENT: &str =
    "0x5ef2de1a7ccc0e9ef8d1a1e6e9c4c2b5d6a7f8e9d0c1b2a3f4e5d6c7b8a9f01";
pub(crate) const BEFORE: u64 = 0x6ae0_6b9d;
/// Inside the ten-minute cap, with the ten-second floor still remaining.
pub(crate) const NOW: u64 = BEFORE - 120;
const TRANSFER: &str = "0x83afd3f4caedc6eebf44246fe54e38c95e3179a5ec9ea81740eca5b482d12e";
const APPROVE: &str = "0x219209e083275171774dab1df80982e9df2096516f06319c5c6d71ae0a8480c";

/// starknet.js's exact output for the user's transfer plus a 1000 fee on `SN_MAIN`.
const STARKNET_JS_MAIN: &str = r#"{"types":{"StarknetDomain":[{"name":"name","type":"shortstring"},{"name":"version","type":"shortstring"},{"name":"chainId","type":"shortstring"},{"name":"revision","type":"shortstring"}],"OutsideExecution":[{"name":"Caller","type":"ContractAddress"},{"name":"Nonce","type":"felt"},{"name":"Execute After","type":"u128"},{"name":"Execute Before","type":"u128"},{"name":"Calls","type":"Call*"}],"Call":[{"name":"To","type":"ContractAddress"},{"name":"Selector","type":"selector"},{"name":"Calldata","type":"felt*"}]},"primaryType":"OutsideExecution","domain":{"name":"Account.execute_from_outside","version":"2","chainId":"SN_MAIN","revision":"1"},"message":{"Caller":"0x127021a1b5a52d3174c2ab077c2b043c80369250d29428cee956d76ee51584f","Nonce":"0x5","Execute After":"0x1","Execute Before":"0x6ae06b9d","Calls":[{"To":"0x53c91253bc9682c04929ca02ed00b3e423f6710d2ee7e0d5ebb06f3ecf368a8","Selector":"0x83afd3f4caedc6eebf44246fe54e38c95e3179a5ec9ea81740eca5b482d12e","Calldata":["0x5ef2de1a7ccc0e9ef8d1a1e6e9c4c2b5d6a7f8e9d0c1b2a3f4e5d6c7b8a9f01","0x10","0x0"]},{"To":"0x53c91253bc9682c04929ca02ed00b3e423f6710d2ee7e0d5ebb06f3ecf368a8","Selector":"0x83afd3f4caedc6eebf44246fe54e38c95e3179a5ec9ea81740eca5b482d12e","Calldata":["0x127021a1b5a52d3174c2ab077c2b043c80369250d29428cee956d76ee51584f","0x3e8","0x0"]}]}}"#;
/// starknet.js's `getMessageHash(STARKNET_JS_MAIN, ACCOUNT)`.
const STARKNET_JS_MAIN_HASH: &str =
    "0x3e0b0050e380d58f5d4dc5e1adf6a16f817dc384f3c41ee1b31d3fea20885e1";
/// The gateway's mainnet fixture hash, as the extension's test pins it.
const GATEWAY_FIXTURE_HASH: &str =
    "0x44d69d150b511e0084b004806ebb0203070e9be95d5178a209891fee25d55cd";

fn user_call() -> Call {
    call(USDC, "transfer", &[RECIPIENT, "0x10", "0x0"])
}

fn fee_call(recipient: &str, amount: &str) -> serde_json::Value {
    json!({"To": USDC, "Selector": TRANSFER, "Calldata": [recipient, amount, "0x0"]})
}

/// `outsideExecution.getTypedData(chain, {FORWARDER, 0x1, BEFORE}, "0x5", calls, V2)`.
pub(crate) fn quote(calls: &[serde_json::Value], chain: &str) -> serde_json::Value {
    let types: serde_json::Value = serde_json::from_str(super::OUTSIDE_EXECUTION_TYPES_V2).unwrap();
    json!({
        "types": types,
        "primaryType": "OutsideExecution",
        "domain": {"name": "Account.execute_from_outside", "version": "2", "chainId": chain, "revision": "1"},
        "message": {
            "Caller": FORWARDER,
            "Nonce": "0x5",
            "Execute After": "0x1",
            "Execute Before": format!("{BEFORE:#x}"),
            "Calls": calls,
        }
    })
}

pub(crate) fn standard() -> serde_json::Value {
    quote(
        &[
            json!({"To": USDC, "Selector": TRANSFER, "Calldata": [RECIPIENT, "0x10", "0x0"]}),
            fee_call(FORWARDER, "0x3e8"),
        ],
        "SN_MAIN",
    )
}

fn js(value: &serde_json::Value) -> JsValue {
    json::parse(&value.to_string()).unwrap()
}

struct Check {
    calls: Vec<Call>,
    network: NetworkId,
    forwarder: &'static str,
    gas_token: &'static str,
    quoted_max: u128,
    ceiling: u128,
    now: u64,
}

impl Default for Check {
    fn default() -> Self {
        Self {
            calls: vec![user_call()],
            network: NetworkId::Mainnet,
            forwarder: FORWARDER,
            gas_token: USDC,
            quoted_max: 1000,
            ceiling: 1000,
            now: NOW,
        }
    }
}

fn check_with(value: &JsValue, check: Check) -> Result<Accepted, Refusal> {
    let input = TypedDataCheck {
        account: felt_of(ACCOUNT),
        network: check.network,
        calls: &check.calls,
        forwarder: felt_of(check.forwarder),
        gas_token: felt_of(check.gas_token),
        quoted_max: U256::from_u128(check.quoted_max),
        ceiling: U256::from_u128(check.ceiling),
        now: check.now,
        minimum_remaining: 10,
    };
    validate_typed_data(value, &input, &POLICY)
}

fn check(value: &serde_json::Value) -> Result<Accepted, Refusal> {
    check_with(&js(value), Check::default())
}

#[test]
fn accepts_the_users_call_plus_one_fee_transfer_and_keeps_the_original_body() {
    let body = json::parse(STARKNET_JS_MAIN).unwrap();
    let accepted = check_with(&body, Check::default()).unwrap();
    assert_eq!(accepted.typed_data, body);
    assert_eq!(accepted.typed_data.to_json(), STARKNET_JS_MAIN);
    assert_eq!(accepted.fee_amount, U256::from_u128(1000));
    assert_eq!(
        felt::canonical(&accepted.message_hash),
        STARKNET_JS_MAIN_HASH
    );
    assert_eq!(
        (accepted.execute_after, accepted.execute_before),
        (1, BEFORE)
    );
}

#[test]
fn hashes_the_gateways_mainnet_fixture_the_way_that_account_does() {
    let mut body = quote(
        &[
            json!({"To": STRK, "Selector": APPROVE, "Calldata": [ACCOUNT, "0x0", "0x0"]}),
            json!({"To": STRK, "Selector": TRANSFER, "Calldata": [FORWARDER, "0x1b608cce453b450", "0x0"]}),
        ],
        "SN_MAIN",
    );
    body["message"]["Nonce"] = json!("0x2ae7dc78d36cee88224d7b8430686e1d");
    let amount = 0x1b6_08cc_e453_b450;
    let accepted = check_with(
        &js(&body),
        Check {
            calls: vec![call(STRK, "approve", &[ACCOUNT, "0x0", "0x0"])],
            gas_token: STRK,
            quoted_max: amount,
            ceiling: amount,
            ..Check::default()
        },
    )
    .unwrap();
    assert_eq!(
        felt::canonical(&accepted.message_hash),
        GATEWAY_FIXTURE_HASH
    );
}

#[test]
fn rejects_the_felt_chain_id_even_though_it_hashes_like_the_shortstring() {
    let mut body = standard();
    body["domain"]["chainId"] = json!("0x534e5f4d41494e");
    assert_eq!(check(&body), Err(Refusal::Shape));
}

#[test]
fn rejects_the_other_networks_chain_id() {
    let mut body = standard();
    body["domain"]["chainId"] = json!("SN_SEPOLIA");
    assert_eq!(check(&body), Err(Refusal::Shape));
    let sepolia = Check {
        network: NetworkId::Sepolia,
        ..Check::default()
    };
    assert_eq!(
        check_with(&js(&standard()), sepolia).err(),
        Some(Refusal::Shape)
    );
}

#[test]
fn rejects_v1_a_decimal_felt_an_extra_call_and_a_substituted_payee() {
    // starknet.js's `getTypedData(.., V1)` output for the same calls.
    let v1 = r#"{"types":{"StarkNetDomain":[{"name":"name","type":"felt"},{"name":"version","type":"felt"},{"name":"chainId","type":"felt"}],"OutsideExecution":[{"name":"caller","type":"felt"},{"name":"nonce","type":"felt"},{"name":"execute_after","type":"felt"},{"name":"execute_before","type":"felt"},{"name":"calls_len","type":"felt"},{"name":"calls","type":"OutsideCall*"}],"OutsideCall":[{"name":"to","type":"felt"},{"name":"selector","type":"felt"},{"name":"calldata_len","type":"felt"},{"name":"calldata","type":"felt*"}]},"primaryType":"OutsideExecution","domain":{"name":"Account.execute_from_outside","version":"1","chainId":"SN_MAIN"},"message":{"caller":"0x127021a1b5a52d3174c2ab077c2b043c80369250d29428cee956d76ee51584f","execute_after":"0x1","execute_before":"0x6ae06b9d","nonce":"0x5","calls_len":2,"calls":[{"to":"0x53c91253bc9682c04929ca02ed00b3e423f6710d2ee7e0d5ebb06f3ecf368a8","selector":"0x83afd3f4caedc6eebf44246fe54e38c95e3179a5ec9ea81740eca5b482d12e","calldata":["0x5ef2de1a7ccc0e9ef8d1a1e6e9c4c2b5d6a7f8e9d0c1b2a3f4e5d6c7b8a9f01","0x10","0x0"],"calldata_len":3},{"to":"0x53c91253bc9682c04929ca02ed00b3e423f6710d2ee7e0d5ebb06f3ecf368a8","selector":"0x83afd3f4caedc6eebf44246fe54e38c95e3179a5ec9ea81740eca5b482d12e","calldata":["0x127021a1b5a52d3174c2ab077c2b043c80369250d29428cee956d76ee51584f","0x3e8","0x0"],"calldata_len":3}]}}"#;
    assert_eq!(
        check_with(&json::parse(v1).unwrap(), Check::default()).err(),
        Some(Refusal::Shape)
    );
    let mut decimal = standard();
    decimal["message"]["Calls"][0]["Calldata"][1] = json!("16");
    assert_eq!(check(&decimal), Err(Refusal::Shape));
    let user = json!({"To": USDC, "Selector": TRANSFER, "Calldata": [RECIPIENT, "0x10", "0x0"]});
    let approve = json!({"To": USDC, "Selector": APPROVE, "Calldata": ["0x1"]});
    let extra = quote(
        &[user.clone(), approve, fee_call(FORWARDER, "0x3e8")],
        "SN_MAIN",
    );
    assert_eq!(check(&extra), Err(Refusal::Calls));
    let swapped = quote(&[user, fee_call(RECIPIENT, "0x3e8")], "SN_MAIN");
    assert_eq!(check(&swapped), Err(Refusal::Calls));
}

#[test]
fn rejects_a_fee_in_another_token_or_by_another_entrypoint_or_of_nothing() {
    let user = json!({"To": USDC, "Selector": TRANSFER, "Calldata": [RECIPIENT, "0x10", "0x0"]});
    let other_token =
        json!({"To": STRK, "Selector": TRANSFER, "Calldata": [FORWARDER, "0x3e8", "0x0"]});
    assert_eq!(
        check(&quote(&[user.clone(), other_token], "SN_MAIN")),
        Err(Refusal::Calls)
    );
    let approve = json!({"To": USDC, "Selector": APPROVE, "Calldata": [FORWARDER, "0x3e8", "0x0"]});
    assert_eq!(
        check(&quote(&[user.clone(), approve], "SN_MAIN")),
        Err(Refusal::Calls)
    );
    assert_eq!(
        check(&quote(&[user, fee_call(FORWARDER, "0x0")], "SN_MAIN")),
        Err(Refusal::Calls)
    );
}

#[test]
fn rejects_reviewed_calls_that_differ_from_the_body() {
    let other = Check {
        calls: vec![call(USDC, "transfer", &[RECIPIENT, "0x11", "0x0"])],
        ..Check::default()
    };
    assert_eq!(
        check_with(&js(&standard()), other).err(),
        Some(Refusal::Calls)
    );
}

#[test]
fn rejects_a_fee_above_the_independent_ceiling_and_above_the_quoted_maximum() {
    let body = js(&quote(
        &[
            json!({"To": USDC, "Selector": TRANSFER, "Calldata": [RECIPIENT, "0x10", "0x0"]}),
            fee_call(FORWARDER, "0x3e9"),
        ],
        "SN_MAIN",
    ));
    let over_ceiling = Check {
        ceiling: 1000,
        quoted_max: 2000,
        ..Check::default()
    };
    assert_eq!(
        check_with(&body, over_ceiling).err(),
        Some(Refusal::Ceiling)
    );
    let over_quote = Check {
        ceiling: 2000,
        quoted_max: 1000,
        ..Check::default()
    };
    assert_eq!(check_with(&body, over_quote).err(), Some(Refusal::Ceiling));
}

#[test]
fn rejects_a_caller_that_is_not_the_pinned_forwarder() {
    let other = Check {
        forwarder: RECIPIENT,
        ..Check::default()
    };
    assert_eq!(
        check_with(&js(&standard()), other).err(),
        Some(Refusal::Forwarder)
    );
}

#[test]
fn rejects_a_closed_execution_window() {
    let closed = Check {
        now: BEFORE,
        ..Check::default()
    };
    assert_eq!(
        check_with(&js(&standard()), closed).err(),
        Some(Refusal::Window)
    );
    let floor = Check {
        now: BEFORE - 9,
        ..Check::default()
    };
    assert_eq!(
        check_with(&js(&standard()), floor).err(),
        Some(Refusal::Window)
    );
}

#[test]
fn rejects_an_execution_window_longer_than_ten_minutes() {
    let long = Check {
        now: BEFORE - 601,
        ..Check::default()
    };
    assert_eq!(
        check_with(&js(&standard()), long).err(),
        Some(Refusal::Window)
    );
    let cap = Check {
        now: BEFORE - 600,
        ..Check::default()
    };
    assert!(check_with(&js(&standard()), cap).is_ok());
}

// The rebuild names every reviewed call by `getSelectorFromName`, so a call whose entrypoint is
// spelled as its selector matches the body felt for felt but not the rebuilt hash.
#[test]
fn refuses_when_the_rebuilt_execution_hashes_differently() {
    let spelled = Check {
        calls: vec![call(USDC, TRANSFER, &[RECIPIENT, "0x10", "0x0"])],
        ..Check::default()
    };
    assert_eq!(
        check_with(&js(&standard()), spelled).err(),
        Some(Refusal::Calls)
    );
}

#[test]
fn rejects_a_fee_transfer_with_extra_calldata() {
    let user = json!({"To": USDC, "Selector": TRANSFER, "Calldata": [RECIPIENT, "0x10", "0x0"]});
    let long =
        json!({"To": USDC, "Selector": TRANSFER, "Calldata": [FORWARDER, "0x3e8", "0x0", "0x1"]});
    assert_eq!(check(&quote(&[user, long], "SN_MAIN")), Err(Refusal::Calls));
}

#[test]
fn rejects_types_other_than_starknet_js_v2() {
    let mut body = standard();
    body["types"]["Call"][1]["type"] = json!("felt");
    assert_eq!(check(&body), Err(Refusal::Shape));
}

// The rebuilt hash would refuse each of these too; the checks before it are what name them, so
// they are pinned on their own.
#[test]
fn the_call_and_fee_checks_refuse_on_their_own_before_any_hash() {
    let body = js(&standard());
    let message = shaped_message(&body, NetworkId::Mainnet).unwrap();
    let reviewed = [user_call()];
    assert!(matching_fee_call(&message.calls, &reviewed).is_some());
    let other = [call(USDC, "transfer", &[RECIPIENT, "0x11", "0x0"])];
    assert!(matching_fee_call(&message.calls, &other).is_none());
    let elsewhere = [call(STRK, "transfer", &[RECIPIENT, "0x10", "0x0"])];
    assert!(matching_fee_call(&message.calls, &elsewhere).is_none());
    let renamed = [call(USDC, "approve", &[RECIPIENT, "0x10", "0x0"])];
    assert!(matching_fee_call(&message.calls, &renamed).is_none());
    assert!(matching_fee_call(&message.calls, &[]).is_none());
    let fee = ParsedCall {
        to: felt_of(USDC),
        selector: felt_of(APPROVE),
        calldata: vec![felt_of(FORWARDER), felt_of("0x3e8"), Felt::ZERO],
    };
    let reviewed = reviewed.to_vec();
    let input = TypedDataCheck {
        account: felt_of(ACCOUNT),
        network: NetworkId::Mainnet,
        calls: &reviewed,
        forwarder: felt_of(FORWARDER),
        gas_token: felt_of(USDC),
        quoted_max: U256::from_u128(1000),
        ceiling: U256::from_u128(1000),
        now: NOW,
        minimum_remaining: 10,
    };
    assert_eq!(fee_amount(&fee, &input), Err(Refusal::Calls));
    let transfer = ParsedCall {
        selector: felt_of(TRANSFER),
        ..fee
    };
    assert_eq!(fee_amount(&transfer, &input), Ok(U256::from_u128(1000)));
}

#[test]
fn rejects_a_nonce_outside_the_felt_grammar_and_a_zero_account() {
    for nonce in ["5", "0X5", ""] {
        let mut body = standard();
        body["message"]["Nonce"] = json!(nonce);
        assert_eq!(check(&body), Err(Refusal::Shape));
    }
    let reviewed = [user_call()];
    let input = TypedDataCheck {
        account: Felt::ZERO,
        network: NetworkId::Mainnet,
        calls: &reviewed,
        forwarder: felt_of(FORWARDER),
        gas_token: felt_of(USDC),
        quoted_max: U256::from_u128(1000),
        ceiling: U256::from_u128(1000),
        now: NOW,
        minimum_remaining: 10,
    };
    assert_eq!(
        validate_typed_data(&js(&standard()), &input, &POLICY),
        Err(Refusal::Forwarder)
    );
}
