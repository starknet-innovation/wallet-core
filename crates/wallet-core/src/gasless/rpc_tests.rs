//! `gasless-rpc.test.ts`. The repeats and the lock check between them are the host's; here each
//! answer's judgement.

use super::*;
use crate::gasless::tests::POLICY;
use serde_json::json;

fn outcome(status: u16, body: &serde_json::Value) -> HttpOutcome {
    HttpOutcome {
        status,
        retry_after: None,
        body: Some(body.to_string().into_bytes()),
        timed_out: false,
    }
}

fn gateway(status: u16, code: &str) -> HttpOutcome {
    outcome(status, &json!({ "error": code }))
}

fn execute(outcome: &HttpOutcome) -> GaslessAnswer {
    classify(outcome, true, &POLICY)
}

fn quote(outcome: &HttpOutcome) -> GaslessAnswer {
    classify(outcome, false, &POLICY)
}

#[test]
fn returns_a_json_rpc_result() {
    let reply = outcome(200, &json!({"id": 1, "jsonrpc": "2.0", "result": true}));
    assert_eq!(quote(&reply), GaslessAnswer::Result(JsValue::Bool(true)));
    let other_id = outcome(200, &json!({"id": 2, "jsonrpc": "2.0", "result": true}));
    assert_eq!(
        quote(&other_id),
        GaslessAnswer::Refused(GaslessCode::Unrecognised)
    );
    assert_eq!(execute(&other_id), GaslessAnswer::Lost);
}

#[test]
fn screening_unavailable_is_repeated_and_a_spent_repeat_is_a_retryable_refusal() {
    let reply = gateway(503, "screening_unavailable");
    assert_eq!(execute(&reply), GaslessAnswer::Retry);
    assert_eq!(quote(&reply), GaslessAnswer::Retry);
    assert_eq!(
        execute(&reply).settle(),
        Err(GaslessError::Refused {
            code: GaslessCode::ScreeningUnavailable,
            retryable: true
        })
    );
}

#[test]
fn another_5xx_on_execute_is_a_lost_reply() {
    assert_eq!(
        execute(&gateway(502, "upstream_rejected")),
        GaslessAnswer::Lost
    );
    assert_eq!(
        quote(&gateway(502, "upstream_rejected")),
        GaslessAnswer::Refused(GaslessCode::UpstreamRejected)
    );
}

#[test]
fn an_allowlisted_code_with_an_error_status_is_a_determinate_pre_forward_refusal() {
    for (status, code, expected) in [
        (
            400,
            "invalid_gasless_request",
            GaslessCode::InvalidGaslessRequest,
        ),
        (403, "address_not_allowed", GaslessCode::AddressNotAllowed),
        (403, "screening_denied", GaslessCode::ScreeningDenied),
        (404, "gasless_disabled", GaslessCode::GaslessDisabled),
    ] {
        let answer = execute(&gateway(status, code));
        assert_eq!(answer, GaslessAnswer::Refused(expected), "{code}");
        assert_eq!(
            answer.settle(),
            Err(GaslessError::Refused {
                code: expected,
                retryable: false
            })
        );
    }
}

#[test]
fn any_other_code_on_execute_is_indeterminate() {
    for (status, code) in [
        (403, "screening_subject_unverifiable"),
        (403, "screening_pending"),
        (400, "invalid_screening_transaction"),
        (400, "upstream_rejected"),
        (400, "some_other_code"),
    ] {
        let answer = execute(&gateway(status, code));
        assert_eq!(answer, GaslessAnswer::Lost, "{code}");
        let text = answer.settle().unwrap_err().to_string();
        assert_eq!(
            text,
            "Transaction submission status is unknown. Check activity and on-chain state before retrying."
        );
    }
}

#[test]
fn an_unreadable_4xx_is_indeterminate_on_execute_and_a_refusal_on_a_quote() {
    let reply = HttpOutcome {
        status: 400,
        retry_after: None,
        body: Some(b"nope".to_vec()),
        timed_out: false,
    };
    assert_eq!(execute(&reply), GaslessAnswer::Lost);
    assert_eq!(
        quote(&reply),
        GaslessAnswer::Refused(GaslessCode::Unrecognised)
    );
}

#[test]
fn a_relay_error_on_execute_is_indeterminate_whatever_code_it_names() {
    for data in [
        "nope",
        "address_not_allowed",
        "gasless_disabled",
        "invalid_gasless_request",
        "screening_denied",
        "screening_unavailable",
    ] {
        let reply = outcome(
            200,
            &json!({"error": {"code": -32000, "data": data, "message": "rejected"}, "id": 1, "jsonrpc": "2.0"}),
        );
        assert_eq!(execute(&reply), GaslessAnswer::Lost, "{data}");
    }
}

#[test]
fn a_2xx_gateway_shaped_code_on_execute_is_indeterminate() {
    for code in [
        "address_not_allowed",
        "gasless_disabled",
        "invalid_gasless_request",
        "screening_denied",
        "screening_unavailable",
    ] {
        assert_eq!(execute(&gateway(200, code)), GaslessAnswer::Lost, "{code}");
    }
}

#[test]
fn the_same_relay_error_on_a_quote_stays_a_determinate_refusal() {
    let reply = outcome(
        200,
        &json!({"error": {"code": -32000, "data": "invalid_gasless_request", "message": "rejected"}, "id": 1, "jsonrpc": "2.0"}),
    );
    assert_eq!(
        quote(&reply),
        GaslessAnswer::Refused(GaslessCode::InvalidGaslessRequest)
    );
}

#[test]
fn an_error_beside_a_result_on_execute_is_indeterminate() {
    let reply = outcome(
        200,
        &json!({"error": {"code": -32000, "data": "invalid_gasless_request", "message": "rejected"}, "id": 1, "jsonrpc": "2.0", "result": {"transaction_hash": "0x1"}}),
    );
    assert_eq!(execute(&reply), GaslessAnswer::Lost);
}

#[test]
fn a_quote_of_upstream_rejected_stays_a_determinate_refusal() {
    let answer = quote(&gateway(400, "upstream_rejected"));
    assert_eq!(
        answer,
        GaslessAnswer::Refused(GaslessCode::UpstreamRejected)
    );
    assert!(answer
        .settle()
        .unwrap_err()
        .to_string()
        .contains("Nothing was submitted"));
}

#[test]
fn a_transport_failure_is_a_lost_reply_on_execute_and_not_on_a_quote() {
    let reply = HttpOutcome {
        status: 0,
        retry_after: None,
        body: None,
        timed_out: true,
    };
    assert_eq!(execute(&reply), GaslessAnswer::Lost);
    assert_eq!(
        quote(&reply),
        GaslessAnswer::Refused(GaslessCode::Unrecognised)
    );
}

#[test]
fn the_request_is_one_json_rpc_post_to_the_route() {
    let request = post(
        &POLICY,
        "https://gateway.example/mainnet",
        "paymaster_isAvailable",
        "[]",
    );
    assert_eq!(request.method, HttpMethod::Post);
    assert_eq!(
        request.body.as_deref(),
        Some(r#"{"id":1,"jsonrpc":"2.0","method":"paymaster_isAvailable","params":[]}"#)
    );
    assert_eq!(request.deadline_ms, POLICY.deadline_ms);
    assert_eq!(request.max_body_bytes, POLICY.max_body_bytes);
}
