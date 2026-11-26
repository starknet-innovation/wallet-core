//! A dapp's SNIP-9 `OutsideExecution` sign request: the calls the signature authorizes, which the
//! site can submit later with no second prompt, get the invoke route's review (the extension's
//! `outside-execution-calls.ts` and background `outside-execution-review.ts`).
//!
//! The calls are read with the encoder the hash uses (`typed_data_scalars`) under the canonical
//! field types, so the address, selector and calldata reviewed are the felts the signature commits
//! to. The layout is keyed on the message shape (`calls` is v1, `Calls` is v2), never on
//! `domain.version`. Anything unreadable is malformed and refused, never signed unchecked.
//!
//! A call to the account itself is refused. A payee that mimics a trusted address is refused at
//! [`Stage::Sign`]; at [`Stage::Queue`] it rides on the card as `address-poisoning`, so the
//! answer does not tell the site which addresses the person saved or paid.

use std::iter;

use super::error::WalletRpcError;
use super::typed_data::{describe_typed_data, CheckedTypedData, Revision};
use super::typed_data_scalars::{snip12_scalar, Encoded};
use crate::js::json::JsValue;
use crate::send::call_decode::decoded_entrypoints;
use crate::send::calls::Call;
use crate::send::invoke::{batch_flags, verify_calls};
use crate::send::multisig_review::is_poisoned;
use crate::send::review_flags::{ReviewContext, RiskFlag};
use crate::{felt, kms, Felt, NetworkId};

/// One of the two SNIP-9 message layouts starknet.js and the account contracts use.
struct Shape {
    calls: &'static str,
    to: &'static str,
    to_type: &'static str,
    selector: &'static str,
    selector_type: &'static str,
    calldata: &'static str,
    revision: Revision,
}

const V1: Shape = Shape {
    calls: "calls",
    to: "to",
    to_type: "felt",
    selector: "selector",
    selector_type: "felt",
    calldata: "calldata",
    revision: Revision::Zero,
};

const V2: Shape = Shape {
    calls: "Calls",
    to: "To",
    to_type: "ContractAddress",
    selector: "Selector",
    selector_type: "selector",
    calldata: "Calldata",
    revision: Revision::One,
};

/// Where the review runs: when the card is queued, or again just before the key is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Queue,
    Sign,
}

/// `OutsideExecutionReview`: what the sign card shows for an outside execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutsideExecutionReview {
    /// The calls the signature authorizes, each selector the decoder knows named.
    pub calls: Vec<Call>,
    /// `verifyCalls(..).flags`, each line once; at [`Stage::Queue`] `address-poisoning` too.
    pub flags: Vec<RiskFlag>,
    /// The card's lines: `describe_typed_data`, then each call's decoded summary.
    pub summaries: Vec<String>,
}

/// The felt `value` hashes to as `ty`.
fn felt_as(value: &JsValue, ty: &str, revision: Revision) -> Option<Felt> {
    match snip12_scalar(value, ty, revision)? {
        Encoded::Felt(felt) => Some(felt),
        Encoded::ByteArray => None,
    }
}

/// `ENTRYPOINT_BY_SELECTOR`: the decoder's name for `selector`, else its hex spelling (an opaque
/// call: a gap here under-describes a call and never invents one).
fn entrypoint(selector: Felt) -> String {
    decoded_entrypoints()
        .find(|name| kms::selector(name) == selector)
        .map_or_else(|| felt::canonical(&selector), str::to_string)
}

fn read_call(value: &JsValue, shape: &Shape) -> Option<Call> {
    value.as_object()?;
    let calldata = value.get(shape.calldata)?.as_array()?;
    let to = felt_as(value.get(shape.to)?, shape.to_type, shape.revision)?;
    let selector = felt_as(
        value.get(shape.selector)?,
        shape.selector_type,
        shape.revision,
    )?;
    let calldata = calldata
        .iter()
        .map(|felt| felt_as(felt, "felt", shape.revision))
        .collect::<Option<_>>()?;
    Some(Call {
        contract_address: to,
        entrypoint: entrypoint(selector),
        calldata,
    })
}

/// `outsideExecutionCalls`: `Ok(None)` for typed data that is not an outside execution, the calls
/// for one, and INVALID_REQUEST_PAYLOAD (114) for one whose calls cannot be read exactly.
pub(super) fn outside_execution_calls(
    value: &JsValue,
) -> Result<Option<Vec<Call>>, WalletRpcError> {
    let is_outside = matches!(value, JsValue::Object(_))
        && value.get("primaryType").and_then(JsValue::as_str) == Some("OutsideExecution");
    if !is_outside {
        return Ok(None);
    }
    let malformed = WalletRpcError::invalid_request_payload;
    let message = value
        .get("message")
        .filter(|message| matches!(message, JsValue::Object(_)))
        .ok_or_else(malformed)?;
    let list = |shape: &Shape| message.get(shape.calls).and_then(JsValue::as_array);
    let (shape, calls) = match (list(&V1), list(&V2)) {
        (Some(calls), None) => (&V1, calls),
        (None, Some(calls)) => (&V2, calls),
        _ => return Err(malformed()),
    };
    calls
        .iter()
        .map(|call| read_call(call, shape))
        .collect::<Option<Vec<_>>>()
        .map(Some)
        .ok_or_else(malformed)
}

/// `reviewOutsideExecution` for `account` on `network`: `Ok(None)` for plain SNIP-12 typed data,
/// the review otherwise. `context` is the invoke route's (`Invoke::reviewed`'s): `trusted` holds
/// the person's contacts, other accounts, watched multisigs and paid recipients, `standing` the
/// allowance log (the extension reads it only at [`Stage::Queue`]; refusals never depend on it).
///
/// Refuses a malformed outside execution as INVALID_REQUEST_PAYLOAD (114), and a call to the
/// account itself, or at [`Stage::Sign`] a payee that mimics a trusted address, as UNKNOWN_ERROR
/// (163), the code the extension's sentences reach a page as. Run it when queuing and again
/// before signing: the trusted set can grow while the card is open.
pub fn review_outside_execution(
    typed: &CheckedTypedData,
    network: NetworkId,
    account: &Felt,
    context: &ReviewContext,
    stage: Stage,
) -> Result<Option<OutsideExecutionReview>, WalletRpcError> {
    let Some(calls) = outside_execution_calls(typed.value())? else {
        return Ok(None);
    };
    // `refuseOnAccountSelfCall`: felts compare by value, so no spelling slips past.
    if calls.iter().any(|call| call.contract_address == *account) {
        return Err(WalletRpcError::unknown_error());
    }
    let poisoned: Vec<bool> = calls
        .iter()
        .map(|call| is_poisoned(account, call, context.trusted))
        .collect();
    if stage == Stage::Sign && poisoned.contains(&true) {
        return Err(WalletRpcError::unknown_error());
    }
    let mut reviewed = verify_calls(network, account, &calls, context);
    // `addressPoisoningFlag` is the last rule `verifyCalls` applies to a call.
    for (call, poisoned) in reviewed.iter_mut().zip(poisoned) {
        if poisoned {
            call.flags.push(RiskFlag::AddressPoisoning);
        }
    }
    let flags = batch_flags(&reviewed).into_iter().cloned().collect();
    let summaries = iter::once(describe_typed_data(typed.value()))
        .chain(reviewed.into_iter().map(|call| call.summary))
        .collect();
    Ok(Some(OutsideExecutionReview {
        calls,
        flags,
        summaries,
    }))
}

#[cfg(test)]
#[path = "outside_execution_tests.rs"]
mod tests;
