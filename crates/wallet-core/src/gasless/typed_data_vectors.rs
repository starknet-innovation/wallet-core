//! The P14 gate: Krusty's SNIP-12 hash agrees with starknet.js on the outside executions the
//! extension signs. Each literal is starknet.js 10.8.0's `typedData.getMessageHash` over
//! `outsideExecution.getTypedData(chain, {caller, execute_after, execute_before}, nonce, calls,
//! OutsideExecutionVersion.V2)` (or that body respelled as noted), computed with the starknet.js
//! the extension pins (`package.json`, Bramble 898879ff5), as its
//! `packages/features/transactions/src/gasless/gasless-typed-data.ts` builds and hashes them.
//! The documents are rebuilt here the same way, so a match covers the build and the hash.

use super::*;
use crate::gasless::tests::felt_of;
use crate::gasless::tests::POLICY;
use serde_json::{json, Value};

const ACCOUNT: &str = "0x5dccf9cfdcabb9e4ef45814bda1b056bb23063d0e9b64e5a845e9dfb3d23f10";
const FORWARDER: &str = "0x127021a1b5a52d3174c2ab077c2b043c80369250d29428cee956d76ee51584f";
const STRK: &str = "0x4718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d";
const USDC: &str = "0x53c91253bc9682c04929ca02ed00b3e423f6710d2ee7e0d5ebb06f3ecf368a8";
const RECIPIENT: &str = "0x5ef2de1a7ccc0e9ef8d1a1e6e9c4c2b5d6a7f8e9d0c1b2a3f4e5d6c7b8a9f01";
const ROUTER: &str = "0x4270219d365d6b017231b52e92b3fb5d7c8378b05e9abc97724537a80e93b0f";
/// The largest felt, `p - 1`.
const P1: &str = "0x800000000000011000000000000000000000000000000000000000000000000";
const BEFORE: &str = "0x6ae06b9d";

/// `{contractAddress, entrypoint, calldata}` as `getOutsideCall` turns it into a V2 call.
fn call(to: &str, entrypoint: &str, calldata: &[&str]) -> Value {
    let selector = felt::canonical(&kms::selector(entrypoint));
    json!({"To": to, "Selector": selector, "Calldata": calldata})
}

fn fee(token: &str, amount: &str) -> Value {
    call(token, "transfer", &[FORWARDER, amount, "0x0"])
}

/// `outsideExecution.getTypedData(chain, {caller, after, before}, nonce, calls, V2)`.
fn outside(
    chain: &str,
    caller: &str,
    nonce: &str,
    after: &str,
    before: &str,
    calls: Vec<Value>,
) -> Value {
    let types: Value = serde_json::from_str(OUTSIDE_EXECUTION_TYPES_V2).unwrap();
    json!({
        "types": types,
        "primaryType": "OutsideExecution",
        "domain": {"name": OUTSIDE_EXECUTION_DOMAIN, "version": "2", "chainId": chain, "revision": "1"},
        "message": {"Caller": caller, "Nonce": nonce, "Execute After": after, "Execute Before": before, "Calls": calls},
    })
}

fn main_transfer(chain: &str) -> Value {
    let user = call(USDC, "transfer", &[RECIPIENT, "0x10", "0x0"]);
    outside(
        chain,
        FORWARDER,
        "0x5",
        "0x1",
        BEFORE,
        vec![user, fee(USDC, "0x3e8")],
    )
}

fn hash(body: &Value, account: &str) -> String {
    let hash = kms::typed_data_message_hash(&body.to_string(), &felt_of(account)).unwrap();
    felt::canonical(&hash)
}

const MAIN_HASH: &str = "0x3e0b0050e380d58f5d4dc5e1adf6a16f817dc384f3c41ee1b31d3fea20885e1";

#[test]
fn one_transfer_on_each_network() {
    assert_eq!(hash(&main_transfer("SN_MAIN"), ACCOUNT), MAIN_HASH);
    assert_eq!(
        hash(&main_transfer("SN_SEPOLIA"), ACCOUNT),
        "0x6020432453d718412ae41abe59b3c65b30cde91b88f534d30833053d29cee62"
    );
}

#[test]
fn an_approve_a_swap_and_a_transfer() {
    let swap: Vec<String> = (0..12).map(|i| format!("{:#x}", 0xabc0 + i * 7)).collect();
    let swap: Vec<&str> = swap.iter().map(String::as_str).collect();
    let body = outside(
        "SN_MAIN",
        FORWARDER,
        "0x2ae7dc78d36cee88224d7b8430686e1d",
        "0x0",
        "0xffffffffffffffff",
        vec![
            call(STRK, "approve", &[ROUTER, "0xde0b6b3a7640000", "0x0"]),
            call(ROUTER, "multi_route_swap", &swap),
            call(USDC, "transfer", &[RECIPIENT, "0x10", "0x0"]),
            fee(STRK, "0x1b608cce453b450"),
        ],
    );
    assert_eq!(
        hash(&body, ACCOUNT),
        "0x8fc4edafe1340294d173eb03e98e2b174e8752efabe10e48bb303d59b1f8c3"
    );
}

#[test]
fn edge_felts_empty_calldata_and_the_u128_bound() {
    let wide = [
        "0x0",
        P1,
        "0x1",
        "0x100000000000000000000000000000000",
        "0x800000000000000000000000000000000000000000000000000000000000000",
    ];
    let body = outside(
        "SN_SEPOLIA",
        P1,
        P1,
        "0x0",
        "0xffffffffffffffffffffffffffffffff",
        vec![
            call("0x1", "transfer", &[]),
            call(P1, "a", &wide),
            fee(USDC, "0xffffffffffffffffffffffffffffffff"),
        ],
    );
    assert_eq!(
        hash(&body, "0x1"),
        "0x4606c1821153d02173be5fcfd87b2a05c9914e2ec0e3cfc5afa45525d09ae1e"
    );
}

#[test]
fn sixty_four_calls_and_a_fee() {
    let mut calls: Vec<Value> = (0..64u32)
        .map(|i| {
            let data: Vec<String> = (0..i % 7).map(|j| format!("{:#x}", i * 31 + j)).collect();
            let data: Vec<&str> = data.iter().map(String::as_str).collect();
            call(&format!("{:#x}", 0x1000 + i), "transfer", &data)
        })
        .collect();
    calls.push(fee(USDC, "0x3e8"));
    let body = outside("SN_MAIN", FORWARDER, "0x7", "0x1", BEFORE, calls);
    assert_eq!(
        hash(&body, P1),
        "0x5783657ef32ec5179926a83e19f4e3836ab7b7ffe6fd0afded09ec1acefb1ab"
    );
}

/// Spellings starknet.js hashes like the canonical body: leading zeros and 64 digits (which the
/// wallet's felt grammar accepts), the felt chain id (the privacy paymaster's spelling) and a
/// revision-1 `selector` given by name (both of which the gasless shape check refuses).
#[test]
fn respelled_bodies_hash_like_the_canonical_one() {
    let mut padded = main_transfer("SN_MAIN");
    padded["message"]["Nonce"] = json!("0x0005");
    padded["message"]["Execute After"] = json!("0x01");
    padded["message"]["Caller"] = json!(format!("0x0{}", &FORWARDER[2..]));
    padded["message"]["Calls"][0]["Calldata"][1] = json!("0x0010");
    padded["message"]["Calls"][1]["To"] = json!(format!("0x0{}", &USDC[2..]));
    let mut felt_chain = main_transfer("SN_MAIN");
    felt_chain["domain"]["chainId"] = json!("0x534e5f4d41494e");
    let mut named = main_transfer("SN_MAIN");
    named["message"]["Calls"][0]["Selector"] = json!("transfer");
    for body in [&padded, &felt_chain, &named] {
        assert_eq!(hash(body, ACCOUNT), MAIN_HASH);
    }

    // The padded body passes the shape check, and its rebuilt execution hashes alike.
    let calls = [crate::gasless::tests::call(
        USDC,
        "transfer",
        &[RECIPIENT, "0x10", "0x0"],
    )];
    let input = TypedDataCheck {
        account: felt_of(ACCOUNT),
        network: NetworkId::Mainnet,
        calls: &calls,
        forwarder: felt_of(FORWARDER),
        gas_token: felt_of(USDC),
        quoted_max: U256::from_u128(1000),
        ceiling: U256::from_u128(1000),
        now: 0x6ae0_6b9d - 120,
        minimum_remaining: 10,
    };
    let body = json::parse(&padded.to_string()).unwrap();
    let accepted = validate_typed_data(&body, &input, &POLICY).unwrap();
    assert_eq!(felt::canonical(&accepted.message_hash), MAIN_HASH);
}
