//! Reading G7h (`fixtures/extension-gasless.jsonl`, Bramble #1034 at `aa944a9c`): its lines by
//! block, the extension's records as the core's types, and the host's half of an exchange.
#![allow(dead_code)] // Each parity file uses its own share.

use serde_json::{json, Value};
use starknet_wallet_core::accounts::AccountFamily;
use starknet_wallet_core::gasless::{GaslessCode, GaslessError, GaslessFeeOffer, GaslessPolicy};
use starknet_wallet_core::gasless::{GaslessRoute, UserFee};
use starknet_wallet_core::portfolio::wire::{HttpMethod, HttpOutcome, HttpRequest};
use starknet_wallet_core::send::calls::Call;
use starknet_wallet_core::send::sign::Deployment;
use starknet_wallet_core::{felt, Felt, NetworkId};

/// Mainnet STRK, as the fixture's held tokens spell it.
pub const STRK: &str = "0x04718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d";

/// Every line of `block`, asserting the count the generator wrote, so a regenerated fixture
/// with more or fewer lines fails here rather than going unread.
pub fn cases(block: &str, count: usize) -> Vec<Value> {
    let found: Vec<Value> = include_str!("../fixtures/extension-gasless.jsonl")
        .lines()
        .map(|line| serde_json::from_str(line).expect("a JSON line"))
        .filter(|case: &Value| case["block"] == block)
        .collect();
    assert_eq!(found.len(), count, "{block} lines");
    found
}

pub fn name(case: &Value) -> String {
    case["name"].as_str().unwrap_or("").to_string()
}

/// A felt as the extension's records write it: hex with any zero padding (an address the
/// generator padded past 64 digits), or decimal calldata (`felt(item, true)`).
pub fn felt_of(text: &str) -> Option<Felt> {
    if let Some(digits) = text.strip_prefix("0x") {
        let significant = digits.trim_start_matches('0');
        felt::parse(&format!(
            "0x{}",
            if significant.is_empty() {
                "0"
            } else {
                significant
            }
        ))
    } else {
        Felt::from_dec_str(text).ok()
    }
}

pub fn felt_at(value: &Value) -> Felt {
    felt_of(value.as_str().expect("a felt string")).expect("a felt")
}

pub fn network(chain: &Value) -> NetworkId {
    NetworkId::from_catalog_id(chain.as_str().unwrap()).expect("a network")
}

/// starknet.js `Call`s, or `None` when a felt does not parse (the extension refuses those too).
pub fn calls(value: &Value) -> Option<Vec<Call>> {
    value
        .as_array()?
        .iter()
        .map(|call| {
            Some(Call {
                contract_address: felt_of(call["contractAddress"].as_str()?)?,
                entrypoint: call["entrypoint"].as_str()?.to_string(),
                calldata: call["calldata"]
                    .as_array()?
                    .iter()
                    .map(|item| felt_of(item.as_str()?))
                    .collect::<Option<_>>()?,
            })
        })
        .collect()
}

/// `accountContractType` as a family; a multisig or read-only account is none.
pub fn family(account: &Value) -> Option<AccountFamily> {
    AccountFamily::from_wire(account["accountContractType"].as_str()?)
}

pub fn deployment(value: &Value) -> Option<Deployment> {
    Some(Deployment {
        class_hash: felt_at(&value["classHash"]),
        salt: felt_at(&value["addressSalt"]),
        constructor_calldata: value["constructorCalldata"]
            .as_array()?
            .iter()
            .map(felt_at)
            .collect(),
    })
}

pub fn offer(value: &Value) -> GaslessFeeOffer {
    GaslessFeeOffer {
        amount: value["amount"].as_str().unwrap().into(),
        decimals: value["decimals"].as_u64().unwrap() as u8,
        label: value["label"].as_str().unwrap().into(),
        token_address: value["tokenAddress"].as_str().unwrap().into(),
    }
}

pub fn offer_json(offer: &GaslessFeeOffer) -> Value {
    json!({
        "amount": offer.amount,
        "decimals": offer.decimals,
        "label": offer.label,
        "tokenAddress": offer.token_address,
    })
}

pub fn user_fee_json(fee: &UserFee) -> Value {
    json!({"amount": fee.amount, "token": fee.token, "tokenAddress": fee.token_address})
}

/// A policy over the case's route (`gaslessRoute(..)` or the generator's Sepolia test route),
/// Bramble's numbers otherwise. The URL lives as long as the test process.
pub fn policy(route: &Value) -> GaslessPolicy {
    let routes = vec![GaslessRoute {
        network: network(&route["chainId"]),
        url: Box::leak(route["url"].as_str().unwrap().to_string().into_boxed_str()),
        enabled: route["enabled"] == true,
        forwarder: route["forwarder"].as_str().and_then(felt::parse),
    }];
    GaslessPolicy {
        routes: Box::leak(routes.into_boxed_slice()),
        ..bramble_core::gasless::GASLESS_POLICY
    }
}

/// The host's reading of one scripted reply. A transport failure, a body streamed past the
/// bound, and a declared `content-length` past the bound or not digits are all a missing body:
/// the host refuses those before handing anything over (`HttpOutcome`'s contract).
pub fn outcome(reply: &Value, limit: u64) -> HttpOutcome {
    let missing = |status| HttpOutcome {
        status,
        retry_after: None,
        body: None,
        timed_out: false,
    };
    let status = reply["status"].as_u64().unwrap_or(0) as u16;
    if reply["throws"] == true || reply.get("fill").is_some() {
        return missing(status);
    }
    if let Some(declared) = reply["contentLength"].as_str() {
        let fits = declared.bytes().all(|b| b.is_ascii_digit())
            && declared.parse::<u64>().is_ok_and(|length| length <= limit);
        if !fits {
            return missing(status);
        }
    }
    HttpOutcome {
        status,
        retry_after: None,
        body: Some(reply["body"].as_str().unwrap().as_bytes().to_vec()),
        timed_out: false,
    }
}

/// What the extension's transport recorded for `request`.
pub fn sent(request: &HttpRequest) -> Value {
    assert_eq!(request.method, HttpMethod::Post);
    let content_type = request
        .headers
        .iter()
        .find(|header| header.name == "content-type")
        .map(|header| header.value.clone());
    json!({
        "body": serde_json::from_str::<Value>(request.body.as_deref().unwrap()).unwrap(),
        "contentType": content_type,
        "method": "POST",
        "url": request.url,
    })
}

pub fn code_text(code: GaslessCode) -> &'static str {
    match code {
        GaslessCode::AddressNotAllowed => "address_not_allowed",
        GaslessCode::ScreeningDenied => "screening_denied",
        GaslessCode::ScreeningPending => "screening_pending",
        GaslessCode::ScreeningSubjectUnverifiable => "screening_subject_unverifiable",
        GaslessCode::ScreeningUnavailable => "screening_unavailable",
        GaslessCode::GaslessDisabled => "gasless_disabled",
        GaslessCode::Unavailable => "unavailable",
        GaslessCode::V2Unsupported => "v2_unsupported",
        GaslessCode::Unverifiable => "unverifiable",
        GaslessCode::AboveCeiling => "above_ceiling",
        GaslessCode::InvalidGaslessRequest => "invalid_gasless_request",
        GaslessCode::InvalidScreeningTransaction => "invalid_screening_transaction",
        GaslessCode::UpstreamRejected => "upstream_rejected",
        GaslessCode::Unrecognised => "unrecognised",
    }
}

/// A refusal as the generator's `thrown` records it.
pub fn thrown(error: &GaslessError) -> Value {
    match error {
        GaslessError::Refused { code, retryable } => json!({
            "code": code_text(*code),
            "message": error.to_string(),
            "name": "GaslessRefusal",
            "retryable": retryable,
        }),
        GaslessError::Lost => json!({
            "code": null,
            "message": error.to_string(),
            "name": "IndeterminateRpcWriteError",
            "retryable": null,
        }),
        GaslessError::ReviewedFeeExceeded => json!({
            "code": null,
            "message": error.to_string(),
            "name": "WalletError",
            "retryable": null,
        }),
    }
}

/// The host's own refusal when the session locked mid-exchange (`WalletError("wallet locked")`).
pub fn locked() -> Value {
    json!({"code": null, "message": "wallet locked", "name": "WalletError", "retryable": null})
}
