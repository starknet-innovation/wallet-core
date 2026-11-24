//! `gasless-rpc.ts`: the gasless exchange and how its answer is judged. It does not share the
//! private relay's rule that every 5xx is a lost broadcast: a 5xx whose body is
//! `screening_unavailable` means screening stopped before the relay was asked.
//!
//! An execute is a determinate refusal only for an allowlisted pre-forward code in the gateway's
//! own `{"error": code}` body with an error status. The gateway writes those before it forwards
//! and passes a relay's 2xx reply through unchanged, so a 2xx body, or a code inside a JSON-RPC
//! `error`, may come from a relay that already holds the signed authorization. Every other
//! execute answer is `Lost`. A quote never broadcasts, so its failures stay refusals.

use super::{GaslessCode, GaslessError, GaslessPolicy, GASLESS_RPC_ID};
use crate::js::json::{self, JsValue};
use crate::portfolio::wire::{HttpHeader, HttpMethod, HttpOutcome, HttpRequest};

/// One judged answer (`Classified`). The host repeats `Retry` up to the policy's
/// `screening_retries` (rechecking the session before an execute's repeat), then settles it with
/// [`GaslessAnswer::settle`].
#[derive(Debug, Clone, PartialEq)]
pub enum GaslessAnswer {
    Result(JsValue),
    Retry,
    Refused(GaslessCode),
    Lost,
}

impl GaslessAnswer {
    /// `throwClassified` once no repeat is left: a spent `Retry` is a retryable
    /// `screening_unavailable` refusal, since nothing was submitted.
    pub fn settle(self) -> Result<JsValue, GaslessError> {
        match self {
            Self::Result(value) => Ok(value),
            Self::Retry => Err(GaslessError::Refused {
                code: GaslessCode::ScreeningUnavailable,
                retryable: true,
            }),
            Self::Refused(code) => Err(code.into()),
            Self::Lost => Err(GaslessError::Lost),
        }
    }
}

/// The POST of one JSON-RPC call to `url` (`sendGasless`), `params` already JSON.
pub(super) fn post(policy: &GaslessPolicy, url: &str, method: &str, params: &str) -> HttpRequest {
    let method = serde_json::Value::String(method.into());
    HttpRequest {
        method: HttpMethod::Post,
        url: url.into(),
        headers: vec![HttpHeader {
            name: "content-type".into(),
            value: "application/json".into(),
        }],
        body: Some(format!(
            r#"{{"id":{GASLESS_RPC_ID},"jsonrpc":"2.0","method":{method},"params":{params}}}"#
        )),
        max_body_bytes: policy.max_body_bytes,
        deadline_ms: policy.deadline_ms,
    }
}

/// Execute codes that mean the relay was not asked.
const PRE_FORWARD_EXECUTE_CODES: [GaslessCode; 5] = [
    GaslessCode::AddressNotAllowed,
    GaslessCode::GaslessDisabled,
    GaslessCode::InvalidGaslessRequest,
    GaslessCode::ScreeningDenied,
    GaslessCode::ScreeningUnavailable,
];

/// `CODE_PATTERN`: `^[a-z0-9_]{1,64}$`.
fn code_text(value: Option<&JsValue>) -> Option<&str> {
    let text = value?.as_str()?;
    ((1..=64).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'))
    .then_some(text)
}

fn named(code: &str) -> GaslessCode {
    GaslessCode::from_gateway(code).unwrap_or(GaslessCode::Unrecognised)
}

/// `hasOnlyKnownKeys`.
fn only_keys(value: &JsValue, keys: &[&str]) -> bool {
    value
        .as_object()
        .is_some_and(|entries| entries.iter().all(|(key, _)| keys.contains(&key.as_str())))
}

/// `gatewayCode`: the gateway's own `{"error": "<code>"}` body.
fn gateway_code(body: &JsValue) -> Option<GaslessCode> {
    if !only_keys(body, &["error"]) {
        return None;
    }
    code_text(body.get("error")).map(named)
}

/// `relayCode`: the `data` of a JSON-RPC error.
fn relay_code(error: &JsValue) -> GaslessCode {
    code_text(error.get("data")).map_or(GaslessCode::Unrecognised, named)
}

fn failed(executes: bool) -> GaslessAnswer {
    if executes {
        GaslessAnswer::Lost
    } else {
        GaslessAnswer::Refused(GaslessCode::Unrecognised)
    }
}

/// `statusClass`.
fn status_class(status: u16, code: Option<GaslessCode>, executes: bool) -> Option<GaslessAnswer> {
    if status >= 500 {
        return Some(match code {
            Some(GaslessCode::ScreeningUnavailable) => GaslessAnswer::Retry,
            _ if executes => GaslessAnswer::Lost,
            code => GaslessAnswer::Refused(code.unwrap_or(GaslessCode::Unrecognised)),
        });
    }
    if let Some(code) = code {
        let refused = !executes || (status >= 400 && PRE_FORWARD_EXECUTE_CODES.contains(&code));
        return Some(if refused {
            GaslessAnswer::Refused(code)
        } else {
            GaslessAnswer::Lost
        });
    }
    (status >= 400).then(|| failed(executes))
}

/// `resultClass`: a JSON-RPC envelope answering this client's id.
fn result_class(body: &JsValue, executes: bool) -> GaslessAnswer {
    if !only_keys(body, &["error", "id", "jsonrpc", "result"])
        || body.get("jsonrpc").and_then(JsValue::as_str) != Some("2.0")
        || body.get("id").and_then(JsValue::as_f64) != Some(GASLESS_RPC_ID as f64)
    {
        return failed(executes);
    }
    if let Some(error) = body.get("error") {
        // On execute a JSON-RPC error is the relay's answer passed through, so even a
        // pre-forward code there cannot prove the relay was never asked.
        return if executes {
            GaslessAnswer::Lost
        } else {
            GaslessAnswer::Refused(relay_code(error))
        };
    }
    match body.get("result") {
        Some(result) => GaslessAnswer::Result(result.clone()),
        None => failed(executes),
    }
}

/// Judges one exchange (`exchange`): a missing, over-budget or non-JSON body is a failure (a lost
/// reply on execute), and anything else is classified by status, then as a JSON-RPC envelope.
pub fn classify(outcome: &HttpOutcome, executes: bool, policy: &GaslessPolicy) -> GaslessAnswer {
    let body = outcome
        .body
        .as_deref()
        .filter(|body| body.len() as u64 <= policy.max_body_bytes)
        .and_then(|body| json::parse(&json::decode_utf8(body)));
    let Some(body) = body else {
        return failed(executes);
    };
    status_class(outcome.status, gateway_code(&body), executes)
        .unwrap_or_else(|| result_class(&body, executes))
}

/// One paymaster exchange and its screening repeats (`postGasless`): the request, whether it
/// executes, and how many repeats it has had.
#[derive(Debug, Clone, PartialEq)]
pub struct Exchange {
    request: HttpRequest,
    executes: bool,
    repeats: u32,
}

/// What the host does after [`Exchange::judge`].
#[derive(Debug, Clone, PartialEq)]
pub enum Exchanged {
    /// Wait `delay_ms`, then post [`Exchange::request`] again. Before an execute's repeat the
    /// host checks the session is still unlocked; a lock refuses (`postGasless`'s
    /// `isAuthorized`), and nothing more is posted.
    Repeat { delay_ms: u64 },
    /// The exchange is over: its result, or why it failed ([`GaslessAnswer::settle`]).
    Settled(Result<JsValue, GaslessError>),
}

impl Exchange {
    pub fn new(request: HttpRequest, executes: bool) -> Self {
        Self {
            request,
            executes,
            repeats: 0,
        }
    }

    /// The request to post, the same on every repeat.
    pub fn request(&self) -> &HttpRequest {
        &self.request
    }

    /// Judges one answer ([`classify`]): `screening_unavailable` is repeated up to the policy's
    /// `screening_retries`, the n-th after `250 * n` ms, and then settles as a retryable refusal.
    pub fn judge(&mut self, outcome: &HttpOutcome, policy: &GaslessPolicy) -> Exchanged {
        match classify(outcome, self.executes, policy) {
            GaslessAnswer::Retry if self.repeats < policy.screening_retries => {
                self.repeats += 1;
                Exchanged::Repeat {
                    delay_ms: 250 * u64::from(self.repeats),
                }
            }
            answer => Exchanged::Settled(answer.settle()),
        }
    }
}

#[cfg(test)]
#[path = "rpc_tests.rs"]
mod tests;
