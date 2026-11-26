use serde_json::{json, Value};

use super::*;
use crate::privacy::config::testing::proof_limits;
use crate::privacy::invocation::tests::{sdk_invocation, POOL, SDK_INVOCATION};

fn pool() -> Felt {
    Felt::from_hex(POOL).unwrap()
}

/// Bramble's prover policy: `STRK20_PROVER_REQUEST` (`{ requestTimeoutMs: 30_000, retries: 3 }`,
/// `packages/features/privacy/src/sdk/official-sdk-budget.ts`) with the SDK's
/// `DEFAULT_PROVE_BASE_DELAY_MS` (1 s) and `MAX_PROVE_BACKOFF_MS` (30 s), which Bramble does not
/// override.
const BRAMBLE: ProverRetry = ProverRetry {
    max_retries: 3,
    request_timeout_ms: 30_000,
    base_delay_ms: 1_000,
    max_delay_ms: 30_000,
};

/// The pinned SDK's `ProvingService` under bun, with `Date.now` stubbed to 1732600000123 and a
/// recording `fetch`: `callOnce` wraps the invocation as below (checked with the invocation of
/// `invocation::tests::sdk_invocation_json` passed through `prove(invocation, 123456)`).
#[test]
fn prove_body_is_the_sdks() {
    let body = prove_body(1_732_600_000_123, 123_456, &sdk_invocation());
    let expected = format!(
        r#"{{"jsonrpc":"2.0","id":1732600000123,"method":"starknet_proveTransaction","params":{{"block_id":{{"block_number":123456}},"transaction":{SDK_INVOCATION}}}}}"#
    );
    assert_eq!(std::str::from_utf8(&body).unwrap(), expected);
    assert_eq!(body.len(), body.capacity());
}

/// `screeningCalldataSuffix` under the pinned SDK: `undefined` and `{}` gave `["0x1"]`;
/// `{signature: {issued_at: 1732600000, sig_r: "0x00ab", sig_s: "0x0"}}` gave
/// `["0x0","0x674560c0","0xab","0x0"]`.
#[test]
fn screening_suffix_is_cairo_serde_of_the_option() {
    assert_eq!(screening_suffix(None), vec![Felt::ONE]);
    let attestation = ScreeningAttestation {
        issued_at: 1_732_600_000,
        sig_r: Felt::from(0xabu64),
        sig_s: Felt::ZERO,
    };
    assert_eq!(
        screening_suffix(Some(&attestation)),
        vec![
            Felt::ZERO,
            Felt::from(0x6745_60c0u64),
            Felt::from(0xabu64),
            Felt::ZERO
        ]
    );
}

/// Observed: four attempts and waits of 1, 2 and 4 s under Bramble's policy before the error
/// surfaced (`callWithRetry` with a stubbed `setTimeout`).
#[test]
fn backoff_doubles_to_the_cap_and_stops_after_the_retries() {
    let table: Vec<Option<u64>> = (0..5).map(|attempt| BRAMBLE.delay_ms(attempt)).collect();
    assert_eq!(table, [Some(1_000), Some(2_000), Some(4_000), None, None]);
    let long = ProverRetry {
        max_retries: 100,
        ..BRAMBLE
    };
    assert_eq!(long.delay_ms(4), Some(16_000));
    assert_eq!(long.delay_ms(5), Some(30_000), "32 s is capped");
    assert_eq!(long.delay_ms(64), Some(30_000), "no overflow");
    assert_eq!(long.delay_ms(99), Some(30_000));
    assert_eq!(long.delay_ms(100), None);
    let off = ProverRetry {
        max_retries: 0,
        ..BRAMBLE
    };
    assert_eq!(off.delay_ms(0), None);
}

fn pool_message(payload: Value) -> Value {
    json!({ "from_address": POOL, "to_address": "0x0", "payload": payload })
}

fn result() -> Value {
    json!({
        "proof": "cHJvb2Y=",
        "proof_facts": ["0x1", "0x2"],
        "l2_to_l1_messages": [
            { "from_address": "0x9", "to_address": "0x0", "payload": ["0x5"] },
            pool_message(json!(["0xc1a55", "0x1", "0x77"])),
        ],
    })
}

fn reply_with(result: &Value) -> Vec<u8> {
    json!({ "jsonrpc": "2.0", "id": 7, "result": result })
        .to_string()
        .into_bytes()
}

fn judge(status: u16, body: &[u8]) -> ProveOutcome {
    judge_prove_reply(status, body, 7, &pool(), &proof_limits())
}

fn judged(result: &Value) -> ProveOutcome {
    judge(200, &reply_with(result))
}

fn felts(values: &[u64]) -> Vec<Felt> {
    values.iter().map(|value| Felt::from(*value)).collect()
}

#[test]
fn builds_apply_actions_as_build_execute_result() {
    let ProveOutcome::Proved(proved) = judged(&result()) else {
        panic!("refused");
    };
    assert_eq!(proved.call.contract_address, pool());
    assert_eq!(proved.call.entrypoint, "apply_actions");
    // `[...payload.slice(1), ...screeningCalldataSuffix(undefined)]`
    assert_eq!(proved.call.calldata, felts(&[0x1, 0x77, 1]));
    assert_eq!(proved.proof.data, "cHJvb2Y=");
    assert_eq!(proved.proof.output, felts(&[0xc1a55, 0x1, 0x77]));
    assert_eq!(proved.proof.proof_facts, felts(&[1, 2]));

    let mut screened = result();
    screened["additional_data"] =
        json!({ "signature": { "issued_at": 1_732_600_000u64, "sig_r": "0xab", "sig_s": "0x0" } });
    let ProveOutcome::Proved(proved) = judged(&screened) else {
        panic!("refused");
    };
    assert_eq!(
        proved.call.calldata,
        felts(&[0x1, 0x77, 0, 1_732_600_000, 0xab, 0])
    );
    // `additional_data` without a signature is `None`.
    let mut empty = result();
    empty["additional_data"] = json!({});
    let ProveOutcome::Proved(proved) = judged(&empty) else {
        panic!("refused");
    };
    assert_eq!(proved.call.calldata, felts(&[0x1, 0x77, 1]));
}

#[test]
fn only_503_and_service_busy_are_transient() {
    assert_eq!(judge(503, b"busy"), ProveOutcome::Busy);
    for status in [500, 502, 504, 429, 404, 301, 199] {
        assert_eq!(
            judge(status, &reply_with(&result())),
            ProveOutcome::Refused(ProofRefusal::ProverFailed),
            "{status}"
        );
    }
    let error = |code: i64| {
        json!({ "jsonrpc": "2.0", "id": 7, "error": { "code": code, "message": "m", "data": "d" } })
            .to_string()
            .into_bytes()
    };
    assert_eq!(judge(200, &error(-32005)), ProveOutcome::Busy);
    for code in [24, 55, 61, 1000, -32603, 10000, -32004] {
        assert_eq!(
            judge(200, &error(code)),
            ProveOutcome::Refused(ProofRefusal::ProverFailed),
            "{code}"
        );
    }
    // Any 2xx is read, as `res.ok`.
    assert!(matches!(
        judge(204, &reply_with(&result())),
        ProveOutcome::Proved(_)
    ));
}

#[test]
fn refuses_an_unreadable_envelope() {
    let invalid = ProveOutcome::Refused(ProofRefusal::InvalidProverReply);
    assert_eq!(judge(200, b"not json"), invalid);
    assert_eq!(judge(200, &[0xff, 0xfe]), invalid);
    assert_eq!(judge(200, br#"{"jsonrpc":"2.0","id":7}"#), invalid);
    assert_eq!(
        judge(200, br#"{"jsonrpc":"2.0","id":7,"result":null}"#),
        invalid
    );
    // Stricter than `callOnce`, which reads neither: the reply must answer this request.
    let other = json!({ "jsonrpc": "2.0", "id": 8, "result": result() }).to_string();
    assert_eq!(judge(200, other.as_bytes()), invalid);
    let no_version = json!({ "id": 7, "result": result() }).to_string();
    assert_eq!(judge(200, no_version.as_bytes()), invalid);
}

#[test]
fn the_result_schema_is_strict_at_every_level() {
    let mut cases: Vec<(&str, Value)> = Vec::new();
    let mut with = |name, edit: &dyn Fn(&mut Value)| {
        let mut value = result();
        edit(&mut value);
        cases.push((name, value));
    };
    with("extra key", &|v| v["extra"] = json!(1));
    with("no proof", &|v| {
        v.as_object_mut().unwrap().remove("proof");
    });
    with("no facts", &|v| {
        v.as_object_mut().unwrap().remove("proof_facts");
    });
    with("no messages", &|v| {
        v.as_object_mut().unwrap().remove("l2_to_l1_messages");
    });
    with("empty proof", &|v| v["proof"] = json!(""));
    with("numeric proof", &|v| v["proof"] = json!(1));
    with("decimal fact", &|v| v["proof_facts"] = json!(["1"]));
    with("uppercase fact", &|v| v["proof_facts"] = json!(["0xAB"]));
    with("fact past the prime", &|v| {
        v["proof_facts"] = json!([format!("0x{}", "f".repeat(64))])
    });
    with("numeric fact", &|v| v["proof_facts"] = json!([1]));
    with("message extra key", &|v| {
        v["l2_to_l1_messages"][1]["extra"] = json!(1)
    });
    with("message without to", &|v| {
        v["l2_to_l1_messages"][1]
            .as_object_mut()
            .unwrap()
            .remove("to_address");
    });
    with("other message's payload", &|v| {
        v["l2_to_l1_messages"][0]["payload"] = json!(["x"])
    });
    with("bad to", &|v| {
        v["l2_to_l1_messages"][1]["to_address"] = json!("0xg")
    });
    with("bad from", &|v| {
        v["l2_to_l1_messages"][0]["from_address"] = json!("")
    });
    with("pool payload", &|v| {
        v["l2_to_l1_messages"][1]["payload"] = json!(["0x1", "2"])
    });
    with("null additional data", &|v| {
        v["additional_data"] = Value::Null
    });
    with("additional data extra key", &|v| {
        v["additional_data"] = json!({ "other": 1 })
    });
    with("null signature", &|v| {
        v["additional_data"] = json!({ "signature": null })
    });
    let signature = |sig: Value| json!({ "signature": sig });
    let good = json!({ "issued_at": 1, "sig_r": "0x1", "sig_s": "0x2" });
    for (name, edit) in [
        ("signature extra key", json!({ "extra": 1 })),
        ("fractional issued_at", json!({ "issued_at": 1.5 })),
        ("negative issued_at", json!({ "issued_at": -1 })),
        (
            "unsafe issued_at",
            json!({ "issued_at": 9_007_199_254_740_992u64 }),
        ),
        ("string issued_at", json!({ "issued_at": "1" })),
        ("bad sig_r", json!({ "sig_r": "1" })),
        ("bad sig_s", json!({ "sig_s": 2 })),
    ] {
        let mut sig = good.clone();
        for (key, value) in edit.as_object().unwrap() {
            sig[key] = value.clone();
        }
        let mut value = result();
        value["additional_data"] = signature(sig);
        cases.push((name, value));
    }
    let mut missing = good.as_object().unwrap().clone();
    missing.remove("sig_s");
    let mut value = result();
    value["additional_data"] = signature(Value::Object(missing));
    cases.push(("signature without sig_s", value));

    for (name, value) in cases {
        assert_eq!(
            judged(&value),
            ProveOutcome::Refused(ProofRefusal::InvalidProverReply),
            "{name}"
        );
    }
    // The largest safe `issued_at` is accepted.
    let mut value = result();
    value["additional_data"] = signature(json!({
        "issued_at": 9_007_199_254_740_991u64, "sig_r": "0x1", "sig_s": "0x2"
    }));
    assert!(matches!(judged(&value), ProveOutcome::Proved(_)));
}

#[test]
fn exactly_one_message_from_the_pool() {
    let unbound = ProveOutcome::Refused(ProofRefusal::UnboundCallOrProof);
    let mut none = result();
    none["l2_to_l1_messages"] = json!([]);
    assert_eq!(
        judged(&none),
        unbound,
        "the SDK would fall back to no output"
    );
    let mut others = result();
    others["l2_to_l1_messages"] =
        json!([{ "from_address": "0x9", "to_address": "0x0", "payload": ["0x1", "0x0"] }]);
    assert_eq!(judged(&others), unbound);
    let mut two = result();
    two["l2_to_l1_messages"]
        .as_array_mut()
        .unwrap()
        .push(pool_message(json!(["0xc1a55", "0x0"])));
    assert_eq!(judged(&two), unbound, "the SDK would take the first");
    // Two spellings of the pool are the same pool.
    let mut padded = result();
    padded["l2_to_l1_messages"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "from_address": "0x40337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a", "to_address": "0x0", "payload": ["0xc1a55", "0x0"] }));
    assert_eq!(judged(&padded), unbound);
    // A payload must hold the class hash and a span.
    for short in [json!([]), json!(["0xc1a55"])] {
        let mut value = result();
        value["l2_to_l1_messages"][1]["payload"] = short;
        assert_eq!(judged(&value), unbound);
    }
    // The pool's message alone, spelled unpadded, is read as the pool's.
    let mut alone = result();
    alone["l2_to_l1_messages"] = json!([{
        "from_address": "0x40337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a",
        "to_address": "0x0",
        "payload": ["0xc1a55", "0x0"],
    }]);
    assert!(matches!(judged(&alone), ProveOutcome::Proved(_)));
}

/// The largest reply `ProofLimits::CEILING` admits, every felt at its longest spelling: it fits the
/// transport bound and is accepted; one more proof character is refused by the bound, not the
/// transport.
#[test]
fn a_maximum_proof_fits_the_transport_bound() {
    let limits = ProofLimits::CEILING;
    let long = format!("0x{}", "0".repeat(63) + "1");
    let suffix = 4; // a `Some` attestation
    let payload = vec![long.clone(); limits.invoke_calldata_felts - suffix + 1];
    assert!(payload.len() <= limits.proof_output_felts);
    let mut value = json!({
        "proof": "A".repeat(limits.proof_data_characters),
        "proof_facts": vec![long.clone(); limits.proof_facts],
        "l2_to_l1_messages": [{ "from_address": long, "to_address": long, "payload": [] }],
        "additional_data": { "signature": { "issued_at": 9_007_199_254_740_991u64, "sig_r": long, "sig_s": long } },
    });
    value["l2_to_l1_messages"][0]["from_address"] = json!(POOL);
    value["l2_to_l1_messages"][0]["payload"] = json!(payload);
    let body = reply_with(&value);
    assert!(body.len() < MAX_PROVE_RESPONSE_BYTES, "{}", body.len());
    let at_bound = judge_prove_reply(200, &body, 7, &pool(), &limits);
    assert!(matches!(at_bound, ProveOutcome::Proved(_)), "{at_bound:?}");

    value["proof"] = json!("A".repeat(limits.proof_data_characters + 1));
    assert_eq!(
        judge_prove_reply(200, &reply_with(&value), 7, &pool(), &limits),
        ProveOutcome::Refused(ProofRefusal::InvalidCallOrProof)
    );
    // A wallet's tighter limits apply; wider ones are clamped to the ceiling.
    let mut wide = limits.clone();
    wide.proof_data_characters = usize::MAX;
    assert_eq!(
        judge_prove_reply(200, &reply_with(&value), 7, &pool(), &wide),
        ProveOutcome::Refused(ProofRefusal::InvalidCallOrProof)
    );
    let padding = " ".repeat(MAX_PROVE_RESPONSE_BYTES);
    let oversized = format!(
        r#"{{"jsonrpc":"2.0","id":7,"result":{}}}{padding}"#,
        result()
    );
    assert_eq!(
        judge(200, oversized.as_bytes()),
        ProveOutcome::Refused(ProofRefusal::InvalidProverReply)
    );
}

#[test]
fn prover_text_never_reaches_the_outcome() {
    let secret = "SECRET-PROVER-TEXT";
    let error =
        json!({ "jsonrpc": "2.0", "id": 7, "error": { "code": 1000, "message": secret, "data": secret } })
            .to_string();
    let outcome = judge(200, error.as_bytes());
    assert!(!format!("{outcome:?}").contains(secret));
    let outcome = judge(500, secret.as_bytes());
    assert!(!format!("{outcome:?}").contains(secret));
}
