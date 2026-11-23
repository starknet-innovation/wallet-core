//! Readers for `fixtures/extension-allowances.jsonl`, which the extension's own TypeScript writes
//! (`scripts/mobile-parity-allowances`, `bun run mobile:parity`), into the core's allowance
//! records.
#![allow(dead_code)] // Each allowances suite reads a different subset.

use serde_json::{json, Value};
use starknet_wallet_core::allowances::discovery::{
    AllowanceReading, AllowanceSource, AllowanceView,
};
use starknet_wallet_core::allowances::record::AllowanceRecord;
use starknet_wallet_core::catalog::TokenInfo;
use starknet_wallet_core::felt;
use starknet_wallet_core::js::felt::canonical_felt;
use starknet_wallet_core::send::u256::U256;
use starknet_wallet_core::{Felt, NetworkId};

/// Every case of `block`, in fixture order.
pub fn cases(block: &str) -> Vec<Value> {
    let found: Vec<Value> = include_str!("../fixtures/extension-allowances.jsonl")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("a JSON line"))
        .filter(|case| case["block"] == block)
        .collect();
    assert!(!found.is_empty(), "no {block} cases");
    found
}

pub fn name(case: &Value) -> &str {
    case["name"].as_str().unwrap()
}

pub fn text(value: &Value) -> &str {
    value.as_str().unwrap()
}

pub fn network(value: &Value) -> NetworkId {
    NetworkId::from_catalog_id(text(value)).unwrap()
}

pub fn amount(value: &Value) -> U256 {
    U256::from_decimal(text(value)).unwrap()
}

pub fn felt(value: &Value) -> Felt {
    felt::parse(text(value)).unwrap()
}

/// Both spell the same felt (`sameFelt`), whether hex or decimal.
pub fn same_felt(left: &Value, right: &Value) -> bool {
    canonical_felt(text(left)).is_some()
        && canonical_felt(text(left)) == canonical_felt(text(right))
}

pub fn record(value: &Value) -> AllowanceRecord {
    AllowanceRecord::from_json(value).unwrap_or_else(|| panic!("a valid record: {value}"))
}

pub fn records(value: &Value) -> Vec<AllowanceRecord> {
    value.as_array().unwrap().iter().map(record).collect()
}

pub fn readings(value: &Value) -> Vec<AllowanceReading> {
    let items = value.as_array().map(Vec::as_slice).unwrap_or_default();
    items
        .iter()
        .map(|r| AllowanceReading {
            id: text(&r["id"]).into(),
            amount: amount(&r["amount"]),
        })
        .collect()
}

/// The trusted tokens of a fixture's `assets` (`ApprovedAssets`), for `send::call_decode::Known`.
pub fn tokens(assets: &Value) -> Vec<TokenInfo> {
    let items = assets["tokens"].as_array().unwrap();
    items
        .iter()
        .map(|t| TokenInfo {
            address: text(&t["address"]).into(),
            decimals: t["decimals"].as_u64().unwrap() as u8,
            name: text(&t["name"]).into(),
            symbol: text(&t["symbol"]).into(),
        })
        .collect()
}

pub fn nft_contracts(assets: &Value) -> Vec<String> {
    let items = assets["nftContracts"].as_array().unwrap();
    items.iter().map(|c| text(c).to_string()).collect()
}

fn source(value: &Value) -> AllowanceSource {
    match text(value) {
        "wallet" => AllowanceSource::Wallet,
        "multisig" => AllowanceSource::Multisig,
        _ => AllowanceSource::External,
    }
}

pub fn view(value: &Value) -> AllowanceView {
    let flag = |key: &str| value[key] == true;
    AllowanceView {
        id: text(&value["id"]).into(),
        account_id: text(&value["accountId"]).into(),
        network: network(&value["networkId"]),
        token: text(&value["token"]).into(),
        spender: text(&value["spender"]).into(),
        amount: amount(&value["amount"]),
        lower_bound: flag("lowerBound"),
        revoking: flag("revoking"),
        source: source(&value["source"]),
        unconfirmed_token: flag("unconfirmedToken"),
    }
}

/// An `AllowanceView` as the extension spells it: optional flags only when set.
pub fn view_json(view: &AllowanceView) -> Value {
    let mut out = json!({
        "accountId": view.account_id,
        "amount": view.amount.to_decimal(),
        "id": view.id,
        "networkId": view.network.catalog_id(),
        "source": match view.source {
            AllowanceSource::Wallet => "wallet",
            AllowanceSource::Multisig => "multisig",
            AllowanceSource::External => "external",
        },
        "spender": view.spender,
        "token": view.token,
    });
    for (key, on) in [
        ("lowerBound", view.lower_bound),
        ("revoking", view.revoking),
        ("unconfirmedToken", view.unconfirmed_token),
    ] {
        if on {
            out[key] = json!(true);
        }
    }
    out
}
