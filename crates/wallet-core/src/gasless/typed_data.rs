//! `validateGaslessTypedData`: the check of a gateway-built outside execution before anything is
//! signed. The body is untrusted. It is accepted only when it is already the SNIP-9 v2 shape the
//! gateway requires (shortstring chain id, revision-1 types, `0x` felts), it is bound to the
//! pinned forwarder, its window is live and short, its calls are the reviewed calls plus one fee
//! transfer of the fee token to the forwarder, and that transfer is within the quoted maximum
//! and the independent ceiling. Last, Krusty's SNIP-12 hash of the body must equal its hash of
//! the same execution rebuilt from the reviewed calls (`outsideExecution.getTypedData`), so no
//! spelling the shape check allowed can change what the signature commits to. The original body
//! is what execute posts; the rebuild is not.

use super::GaslessPolicy;
use crate::js::json::{self, JsValue};
use crate::send::call_decode::uint256_at;
use crate::send::calls::Call;
use crate::send::u256::U256;
use crate::{felt, kms, Felt, NetworkId};

/// The SNIP-9 domain name (`OUTSIDE_EXECUTION_DOMAIN`).
pub const OUTSIDE_EXECUTION_DOMAIN: &str = "Account.execute_from_outside";

/// starknet.js 10.8's `OutsideExecutionTypesV2`, which a body's `types` must equal.
const OUTSIDE_EXECUTION_TYPES_V2: &str = r#"{"StarknetDomain":[{"name":"name","type":"shortstring"},{"name":"version","type":"shortstring"},{"name":"chainId","type":"shortstring"},{"name":"revision","type":"shortstring"}],"OutsideExecution":[{"name":"Caller","type":"ContractAddress"},{"name":"Nonce","type":"felt"},{"name":"Execute After","type":"u128"},{"name":"Execute Before","type":"u128"},{"name":"Calls","type":"Call*"}],"Call":[{"name":"To","type":"ContractAddress"},{"name":"Selector","type":"selector"},{"name":"Calldata","type":"felt*"}]}"#;

const MESSAGE_FIELDS: [&str; 5] = [
    "Caller",
    "Nonce",
    "Execute After",
    "Execute Before",
    "Calls",
];
const DOMAIN_FIELDS: [&str; 4] = ["chainId", "name", "revision", "version"];
const CALL_FIELDS: [&str; 3] = ["Calldata", "Selector", "To"];
const ENVELOPE_FIELDS: [&str; 4] = ["domain", "message", "primaryType", "types"];

/// Why a body was refused (`GaslessTypedDataReason`). `Ceiling`: the fee transfer is above the
/// quoted maximum or the independent ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Calls,
    Ceiling,
    Forwarder,
    Shape,
    Window,
}

/// What a body is checked against: everything here is the wallet's, never the gateway's.
#[derive(Debug, Clone, Copy)]
pub struct TypedDataCheck<'a> {
    pub account: Felt,
    pub network: NetworkId,
    /// The reviewed calls, in order.
    pub calls: &'a [Call],
    /// The pinned forwarder, never a value read from the quote.
    pub forwarder: Felt,
    pub gas_token: Felt,
    /// The quote's `suggested_max_fee_in_gas_token`.
    pub quoted_max: U256,
    /// The independent maximum in the fee token's base units (`GaslessPolicy::fee_ceiling`).
    pub ceiling: U256,
    pub now: u64,
    /// The live-window floor, the policy's `sign_window_seconds` on the path about to sign.
    pub minimum_remaining: u64,
}

/// A body that may be signed and posted unchanged (`AcceptedGaslessTypedData`).
#[derive(Debug, Clone, PartialEq)]
pub struct Accepted {
    pub caller: Felt,
    pub execute_after: u64,
    pub execute_before: u64,
    pub fee_amount: U256,
    pub typed_data: JsValue,
    /// Krusty's SNIP-12 hash of `typed_data` for the account: what is signed.
    pub message_hash: Felt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedCall {
    to: Felt,
    selector: Felt,
    calldata: Vec<Felt>,
}

/// `sameJson`: equal JSON, object keys in any order.
fn same_json(left: &JsValue, right: &JsValue) -> bool {
    match (left, right) {
        (JsValue::Array(a), JsValue::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| same_json(x, y))
        }
        (JsValue::Object(a), JsValue::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(key, x)| right.get(key).is_some_and(|y| same_json(x, y)))
        }
        _ => left == right,
    }
}

/// `hasKeys`: exactly these keys.
fn has_keys(value: &JsValue, keys: &[&str]) -> bool {
    value.as_object().is_some_and(|entries| {
        entries.len() == keys.len() && keys.iter().all(|key| value.get(key).is_some())
    })
}

/// `isHexFelt`: a string in the strict felt grammar.
fn hex_felt(value: Option<&JsValue>) -> Option<Felt> {
    felt::parse(value?.as_str()?)
}

fn hex_non_zero(value: Option<&JsValue>) -> Option<Felt> {
    hex_felt(value).filter(|felt| *felt != Felt::ZERO)
}

/// `isU64Felt`.
fn hex_u64(value: Option<&JsValue>) -> Option<u64> {
    u64::try_from(hex_felt(value)?).ok()
}

/// `selectorOf`: a felt-shaped entrypoint is its own selector; any other is hashed by name.
pub(super) fn selector_of(entrypoint: &str) -> Felt {
    felt::parse(entrypoint).unwrap_or_else(|| kms::selector(entrypoint))
}

fn parse_call(call: &JsValue) -> Option<ParsedCall> {
    if !has_keys(call, &CALL_FIELDS) {
        return None;
    }
    let calldata = call
        .get("Calldata")?
        .as_array()?
        .iter()
        .map(|item| hex_felt(Some(item)))
        .collect::<Option<Vec<_>>>()?;
    Some(ParsedCall {
        to: hex_non_zero(call.get("To"))?,
        selector: hex_felt(call.get("Selector"))?,
        calldata,
    })
}

struct Message<'a> {
    raw: &'a JsValue,
    caller: Felt,
    execute_after: u64,
    execute_before: u64,
    calls: Vec<ParsedCall>,
}

/// `shapedMessage` and `parsedCalls`.
fn shaped_message(value: &JsValue, network: NetworkId) -> Option<Message<'_>> {
    if !has_keys(value, &ENVELOPE_FIELDS)
        || value.get("primaryType")?.as_str()? != "OutsideExecution"
    {
        return None;
    }
    let types = json::parse(OUTSIDE_EXECUTION_TYPES_V2)?;
    let domain = value.get("domain")?;
    let message = value.get("message")?;
    let domain_matches = domain.get("name")?.as_str()? == OUTSIDE_EXECUTION_DOMAIN
        && domain.get("version")?.as_str()? == "2"
        && domain.get("revision")?.as_str()? == "1"
        && domain.get("chainId")?.as_str()? == network.catalog_id();
    if !same_json(value.get("types")?, &types)
        || !has_keys(domain, &DOMAIN_FIELDS)
        || !has_keys(message, &MESSAGE_FIELDS)
        || !domain_matches
    {
        return None;
    }
    hex_felt(message.get("Nonce"))?;
    let calls = message
        .get("Calls")?
        .as_array()?
        .iter()
        .map(parse_call)
        .collect::<Option<Vec<_>>>()?;
    if calls.is_empty() {
        return None;
    }
    Some(Message {
        raw: message,
        caller: hex_non_zero(message.get("Caller"))?,
        execute_after: hex_u64(message.get("Execute After"))?,
        execute_before: hex_u64(message.get("Execute Before"))?,
        calls,
    })
}

/// `userCallMatches`.
fn call_matches(parsed: &ParsedCall, call: &Call) -> bool {
    parsed.to == call.contract_address
        && parsed.selector == selector_of(&call.entrypoint)
        && parsed.calldata == call.calldata
}

/// `matchingFeeCall`: the reviewed calls in order, then exactly one more, the fee.
fn matching_fee_call<'a>(calls: &'a [ParsedCall], reviewed: &[Call]) -> Option<&'a ParsedCall> {
    let (fee, user) = calls.split_last()?;
    (user.len() == reviewed.len() && user.iter().zip(reviewed).all(|(p, c)| call_matches(p, c)))
        .then_some(fee)
}

/// `feeAmountOf`: `transfer(forwarder, amount)` on the fee token, a canonical non-zero u256
/// within both maxima.
fn fee_amount(fee: &ParsedCall, input: &TypedDataCheck<'_>) -> Result<U256, Refusal> {
    if fee.to != input.gas_token
        || fee.selector != kms::selector("transfer")
        || fee.calldata.len() != 3
        || fee.calldata[0] != input.forwarder
    {
        return Err(Refusal::Calls);
    }
    let amount = uint256_at(&fee.calldata, 1).ok_or(Refusal::Calls)?;
    if amount == U256::ZERO {
        return Err(Refusal::Calls);
    }
    if amount > input.quoted_max || amount > input.ceiling {
        return Err(Refusal::Ceiling);
    }
    Ok(amount)
}

fn json_text(value: &str) -> String {
    serde_json::Value::String(value.into()).to_string()
}

fn outside_call_json(to: &Felt, selector: &Felt, calldata: &[Felt]) -> String {
    let calldata: Vec<String> = calldata
        .iter()
        .map(|f| json_text(&felt::canonical(f)))
        .collect();
    format!(
        r#"{{"To":{},"Selector":{},"Calldata":[{}]}}"#,
        json_text(&felt::canonical(to)),
        json_text(&felt::canonical(selector)),
        calldata.join(",")
    )
}

/// `outsideExecution.getTypedData(chainId, {caller, execute_after, execute_before}, nonce,
/// [...calls, fee], V2)`: the reviewed calls and the fee transfer, under the message's own nonce
/// and bounds. Each call's selector is `selector_of` its entrypoint, as the gateway's build
/// encodes it, so a felt entrypoint is its own selector. starknet.js's builder runs every
/// entrypoint through `getSelectorFromName`, hashing a felt's text as a name, so the extension
/// refuses an honest body for such a call; Bramble #1039 sets the selector from the felt, as
/// this does.
fn rebuilt(
    message: &Message<'_>,
    network: NetworkId,
    calls: &[Call],
    fee: &ParsedCall,
) -> Option<String> {
    let text = |key: &str| message.raw.get(key).map(JsValue::to_json);
    let mut outside: Vec<String> = calls
        .iter()
        .map(|call| {
            outside_call_json(
                &call.contract_address,
                &selector_of(&call.entrypoint),
                &call.calldata,
            )
        })
        .collect();
    outside.push(outside_call_json(
        &fee.to,
        &kms::selector("transfer"),
        &fee.calldata,
    ));
    Some(format!(
        r#"{{"types":{OUTSIDE_EXECUTION_TYPES_V2},"primaryType":"OutsideExecution","domain":{{"name":"{OUTSIDE_EXECUTION_DOMAIN}","version":"2","chainId":"{}","revision":"1"}},"message":{{"Caller":{},"Nonce":{},"Execute After":{},"Execute Before":{},"Calls":[{}]}}}}"#,
        network.catalog_id(),
        json_text(&felt::canonical(&message.caller)),
        text("Nonce")?,
        text("Execute After")?,
        text("Execute Before")?,
        outside.join(",")
    ))
}

/// Accepts the gateway's typed data or says why not (`validateGaslessTypedData`).
pub fn validate_typed_data(
    value: &JsValue,
    input: &TypedDataCheck<'_>,
    policy: &GaslessPolicy,
) -> Result<Accepted, Refusal> {
    if input.forwarder == Felt::ZERO
        || input.gas_token == Felt::ZERO
        || input.account == Felt::ZERO
        || input.calls.is_empty()
    {
        return Err(Refusal::Forwarder);
    }
    let message = shaped_message(value, input.network).ok_or(Refusal::Shape)?;
    if message.caller != input.forwarder {
        return Err(Refusal::Forwarder);
    }
    if !policy.window_is_live(
        message.execute_after,
        message.execute_before,
        input.now,
        input.minimum_remaining,
    ) {
        return Err(Refusal::Window);
    }
    let fee = matching_fee_call(&message.calls, input.calls).ok_or(Refusal::Calls)?;
    let fee_amount = fee_amount(fee, input)?;
    let rebuilt = rebuilt(&message, input.network, input.calls, fee).ok_or(Refusal::Shape)?;
    let supplied = kms::typed_data_message_hash(&value.to_json(), &input.account)
        .map_err(|_| Refusal::Shape)?;
    let expected =
        kms::typed_data_message_hash(&rebuilt, &input.account).map_err(|_| Refusal::Shape)?;
    if supplied != expected {
        return Err(Refusal::Calls);
    }
    Ok(Accepted {
        caller: message.caller,
        execute_after: message.execute_after,
        execute_before: message.execute_before,
        fee_amount,
        typed_data: value.clone(),
        message_hash: supplied,
    })
}

#[cfg(test)]
#[path = "typed_data_tests.rs"]
pub(super) mod tests;

#[cfg(test)]
#[path = "typed_data_vectors.rs"]
mod vectors;
