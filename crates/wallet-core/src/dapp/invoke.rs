//! `wallet_addInvokeTransaction`'s params, read as the extension's `providerInvokeRequest` and
//! `readProviderInvokeCall` read them (`packages/foundation/wallet-protocol/src/transactions/
//! invoke.ts`): the spec's `invoke_transaction` or the legacy `calls`, each call in snake or
//! camel case, every calldata entry a felt. A malformed call refuses the whole request; nothing
//! guesses at a partial batch.

use starknet_types_core::felt::Felt;

use crate::js::felt::canonical_felt;
use crate::js::json::JsValue;
use crate::js::num::utf16_len;
use crate::send::calls::Call;

/// The wallet's bounds on a page's batch (`INVOKE_LIMITS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvokeLimits {
    pub calls: usize,
    pub calldata_felts: usize,
    /// An entrypoint's longest name, in UTF-16 units.
    pub entrypoint_chars: usize,
    /// A calldata string's longest spelling, in UTF-16 units (decimal felts run to 78).
    pub felt_chars: usize,
}

/// What an invoke's params held: the calls, and whether they came with a STRK20 proof.
pub(super) struct InvokeParams {
    pub calls: Vec<Call>,
    pub has_proof: bool,
}

/// `providerInvokeRequest`, short of the proof's own shape, which is STRK20's (P15).
pub(super) fn invoke_params(
    params: Option<&JsValue>,
    limits: &InvokeLimits,
) -> Option<InvokeParams> {
    let params = params?;
    let entries = params.as_object()?;
    let known = ["api_version", "calls", "invoke_transaction", "proof"];
    if !entries.iter().all(|(key, _)| known.contains(&key.as_str()))
        || !super::request::optional_api_version(params.get("api_version"))
        || (params.get("calls").is_some() && params.get("invoke_transaction").is_some())
    {
        return None;
    }
    // `params["invoke_transaction"] ?? params["calls"]`.
    let source = params
        .get("invoke_transaction")
        .filter(|value| **value != JsValue::Null)
        .or_else(|| params.get("calls"))?
        .as_array()?;
    if source.is_empty() || source.len() > limits.calls {
        return None;
    }
    let calls = source
        .iter()
        .map(|call| invoke_call(call, limits))
        .collect::<Option<Vec<_>>>()?;
    // A `null` proof is no proof at all (`isStrk20Proof(null)` is false): refused here as the
    // extension refuses it. Any other proof's shape is P15's to read.
    let proof = params.get("proof");
    if proof == Some(&JsValue::Null) {
        return None;
    }
    Some(InvokeParams {
        calls,
        has_proof: proof.is_some(),
    })
}

/// `readProviderInvokeCall`.
fn invoke_call(call: &JsValue, limits: &InvokeLimits) -> Option<Call> {
    let entries = call.as_object()?;
    let known = [
        "calldata",
        "contractAddress",
        "contract_address",
        "entry_point",
        "entrypoint",
    ];
    if !entries.iter().all(|(key, _)| known.contains(&key.as_str())) {
        return None;
    }
    // The snake-case key wins only when it holds a string, as the extension's ternary reads it.
    let string_of = |first: &str, second: &str| {
        call.get(first)
            .and_then(JsValue::as_str)
            .or_else(|| call.get(second).and_then(JsValue::as_str))
    };
    let contract_address =
        crate::felt::parse_non_zero(string_of("contract_address", "contractAddress")?)?;
    let entrypoint = string_of("entry_point", "entrypoint")?;
    if utf16_len(entrypoint) > limits.entrypoint_chars || !is_entrypoint_name(entrypoint) {
        return None;
    }
    // `call["calldata"] ?? []`.
    let calldata = match call.get("calldata") {
        None | Some(JsValue::Null) => &[][..],
        Some(value) => value.as_array()?,
    };
    if calldata.len() > limits.calldata_felts {
        return None;
    }
    let calldata = calldata
        .iter()
        .map(|item| calldata_felt(item, limits))
        .collect::<Option<Vec<_>>>()?;
    Some(Call {
        contract_address,
        entrypoint: entrypoint.to_owned(),
        calldata,
    })
}

/// `/^[A-Za-z_][A-Za-z0-9_]*$/`.
fn is_entrypoint_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// `toCalldataFelt`: a non-negative integer number, or a string `BigInt` reads as a felt.
fn calldata_felt(value: &JsValue, limits: &InvokeLimits) -> Option<Felt> {
    match value {
        JsValue::Number(number) => integer_felt(*number),
        JsValue::String(text) if utf16_len(text) <= limits.felt_chars => {
            crate::felt::parse(&canonical_felt(text)?)
        }
        _ => None,
    }
}

/// `BigInt(value)` for an integer-valued, non-negative `Number`, if it is below the prime. The
/// only double in `[2^251, P)` is `2^251` itself (doubles that large are `2^199` apart), so
/// anything above it is out of the field.
fn integer_felt(number: f64) -> Option<Felt> {
    if !(number >= 0.0 && number.fract() == 0.0 && number <= 2f64.powi(251)) {
        return None;
    }
    if number < 2f64.powi(64) {
        // Exact: an integer-valued double below 2^64 fits.
        return Some(Felt::from(number as u64));
    }
    // Above 2^64 a double is `mantissa * 2^exponent` with a 53-bit mantissa.
    let bits = number.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as u32 - 1075;
    let mantissa = (bits & ((1 << 52) - 1)) | (1 << 52);
    Some(Felt::from(mantissa) * Felt::TWO.pow(exponent))
}

#[cfg(test)]
#[path = "invoke_tests.rs"]
mod tests;
