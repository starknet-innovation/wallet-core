//! The remote prover (the SDK's `ProvingService.proveTransaction` and
//! `ProvingServiceProofProvider.prove`, then `buildExecuteResult`): the
//! `starknet_proveTransaction` body, the judge of one attempt's reply, the retry rule, and the
//! pool's `apply_actions` call with its trailing screening attestation, through
//! `proof::judge_proof`'s bounds.
//!
//! Nothing the prover says is kept: every failure is a fixed `ProofRefusal` or `Busy`.

use serde::Deserialize;
use serde_json::Value;
use zeroize::Zeroizing;

use super::invocation::ProofInvocation;
use super::json::MAX_SAFE_INTEGER;
use super::proof::{judge_proof, CallAndProof, Proof, ProofLimits, ProofRefusal, APPLY_ACTIONS};
use crate::send::calls::Call;
use crate::send::judge::{answer_within, Answer};
use crate::{felt, Felt};

/// Longest prove reply the core parses; hosts stop reading at this many bytes. The privacy
/// service's bound, which a legitimate maximum fits with room to spare: `ProofLimits::CEILING`
/// admits 512 Ki characters of base64 proof (ASCII, so as many bytes) and 4,096 felts each of
/// output and facts (at most 69 bytes each as `"0x<64>",`), about 1.04 MiB with the envelope.
/// `rpc::MAX_RESPONSE_BYTES` (64 KiB) would refuse real proofs.
pub const MAX_PROVE_RESPONSE_BYTES: usize = super::judge::MAX_RESPONSE_BYTES;

/// The prover's "service busy" JSON-RPC code, the one error it asks to be retried.
const SERVICE_BUSY: i64 = -32005;
/// The one HTTP status retried on the plain-fetch transport (`TRANSIENT_HTTP_STATUS`).
const SERVICE_UNAVAILABLE: u16 = 503;

/// `starknet_proveTransaction` at `block_number` (the proving block), as `callOnce` writes it:
/// `{"jsonrpc":"2.0","id":<id>,"method":…,"params":{"block_id":{"block_number":N},
/// "transaction":<invocation>}}`. The SDK's `id` is `Date.now()`; the host passes its own. The
/// body holds the invocation, so it carries the viewing key: same handling as
/// `PrivacyStep::Post::body`.
pub fn prove_body(id: u64, block_number: u64, invocation: &ProofInvocation) -> Zeroizing<Vec<u8>> {
    let before = format!(
        "{{\"jsonrpc\":\"2.0\",\"id\":{id},\"method\":\"starknet_proveTransaction\",\
         \"params\":{{\"block_id\":{{\"block_number\":{block_number}}},\"transaction\":"
    );
    invocation.json_within(&before, "}}")
}

/// The wallet's retry policy for transient prove failures (the SDK's `ProvingRetryOptions`, its
/// per-attempt `requestTimeoutMs` and `MAX_PROVE_BACKOFF_MS`). Bramble's is its
/// `STRK20_PROVER_REQUEST` with the SDK's defaults for the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProverRetry {
    /// Retries after the first attempt; 0 disables them.
    pub max_retries: u32,
    /// How long the host waits for one attempt. A timeout is a transport failure, which the SDK
    /// does not retry.
    pub request_timeout_ms: u64,
    /// The wait before retry `n` (from 0) is `base_delay_ms * 2^n`, at most `max_delay_ms`.
    pub base_delay_ms: u64,
    pub max_delay_ms: u64,
}

impl ProverRetry {
    /// `callWithRetry`: after attempt `attempt` (from 0) came back [`ProveOutcome::Busy`], the
    /// wait before trying again, or `None` once the retries are spent.
    pub fn delay_ms(&self, attempt: u32) -> Option<u64> {
        (attempt < self.max_retries).then(|| {
            let factor = 1u64.checked_shl(attempt).unwrap_or(u64::MAX);
            self.base_delay_ms
                .saturating_mul(factor)
                .min(self.max_delay_ms)
        })
    }
}

/// What one prove attempt came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProveOutcome {
    /// The judged `apply_actions` call and its proof.
    Proved(CallAndProof),
    /// HTTP 503 or JSON-RPC `-32005`: try again per [`ProverRetry::delay_ms`].
    Busy,
    Refused(ProofRefusal),
}

/// The screening attestation a prover attaches to a screened deposit
/// (`additional_data.signature`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreeningAttestation {
    /// Unix seconds.
    pub issued_at: u64,
    pub sig_r: Felt,
    pub sig_s: Felt,
}

/// `screeningCalldataSuffix`: the Serde-encoded `Option<ScreeningAttestation>` that ends
/// `apply_actions`' calldata, as `CallData.compile([CairoOption])` writes it: `[1]` for `None`,
/// `[0, issued_at, sig_r, sig_s]` for `Some`.
pub fn screening_suffix(attestation: Option<&ScreeningAttestation>) -> Vec<Felt> {
    match attestation {
        None => vec![Felt::ONE],
        Some(attestation) => vec![
            Felt::ZERO,
            Felt::from(attestation.issued_at),
            attestation.sig_r,
            attestation.sig_s,
        ],
    }
}

// The result schema (`ProveTransactionResultSchema`): zod's `.strict()` is `deny_unknown_fields`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProveResult {
    proof: String,
    proof_facts: Vec<String>,
    l2_to_l1_messages: Vec<MessageToL1>,
    additional_data: Option<AdditionalData>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MessageToL1 {
    from_address: String,
    to_address: String,
    payload: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AdditionalData {
    signature: Option<ScreeningSignature>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScreeningSignature {
    issued_at: u64,
    sig_r: String,
    sig_s: String,
}

/// Judges one prove attempt's reply: `callOnce`'s HTTP and JSON-RPC handling, the result schema,
/// then `prove` and `buildExecuteResult` for `pool` under `limits` (clamped to
/// `ProofLimits::CEILING`).
pub fn judge_prove_reply(
    status: u16,
    body: &[u8],
    id: u64,
    pool: &Felt,
    limits: &ProofLimits,
) -> ProveOutcome {
    if !(200..300).contains(&status) {
        return if status == SERVICE_UNAVAILABLE {
            ProveOutcome::Busy
        } else {
            ProveOutcome::Refused(ProofRefusal::ProverFailed)
        };
    }
    let answer = std::str::from_utf8(body)
        .ok()
        .and_then(|text| answer_within(text, id, MAX_PROVE_RESPONSE_BYTES));
    match answer {
        Some(Answer::Error {
            code: Some(SERVICE_BUSY),
            ..
        }) => ProveOutcome::Busy,
        Some(Answer::Error { .. }) => ProveOutcome::Refused(ProofRefusal::ProverFailed),
        Some(Answer::Result(result)) => match call_and_proof(result, pool, limits) {
            Ok(proof) => ProveOutcome::Proved(proof),
            Err(refusal) => ProveOutcome::Refused(refusal),
        },
        None => ProveOutcome::Refused(ProofRefusal::InvalidProverReply),
    }
}

fn felts(values: &[String]) -> Option<Vec<Felt>> {
    values.iter().map(|value| felt::parse(value)).collect()
}

/// zod's `.optional()` admits an absent key, not `null`, which serde's `Option` would.
fn has_null(value: &Value, key: &str) -> bool {
    value.get(key).is_some_and(Value::is_null)
}

fn call_and_proof(
    result: Value,
    pool: &Felt,
    limits: &ProofLimits,
) -> Result<CallAndProof, ProofRefusal> {
    let invalid = ProofRefusal::InvalidProverReply;
    let signature_null = result
        .get("additional_data")
        .is_some_and(|data| has_null(data, "signature"));
    if has_null(&result, "additional_data") || signature_null {
        return Err(invalid);
    }
    let result: ProveResult = serde_json::from_value(result).map_err(|_| invalid)?;
    if result.proof.is_empty() {
        return Err(invalid);
    }
    let proof_facts = felts(&result.proof_facts).ok_or(invalid)?;
    // Every message must be well formed; only the pool's is read. The SDK takes the first from
    // the pool (by its hex spelling) and falls back to no output; here there must be exactly one
    // (by value), and its payload must hold the class hash and a span.
    let mut from_pool = Vec::new();
    for message in &result.l2_to_l1_messages {
        let from = felt::parse(&message.from_address).ok_or(invalid)?;
        felt::parse(&message.to_address).ok_or(invalid)?;
        let payload = felts(&message.payload).ok_or(invalid)?;
        if from == *pool {
            from_pool.push(payload);
        }
    }
    let output = match <[Vec<Felt>; 1]>::try_from(from_pool) {
        Ok([payload]) if payload.len() >= 2 => payload,
        _ => return Err(ProofRefusal::UnboundCallOrProof),
    };
    let attestation = match result.additional_data.and_then(|data| data.signature) {
        None => None,
        Some(signature) if signature.issued_at <= MAX_SAFE_INTEGER => Some(ScreeningAttestation {
            issued_at: signature.issued_at,
            sig_r: felt::parse(&signature.sig_r).ok_or(invalid)?,
            sig_s: felt::parse(&signature.sig_s).ok_or(invalid)?,
        }),
        Some(_) => return Err(invalid),
    };
    let mut calldata = output.get(1..).unwrap_or_default().to_vec();
    calldata.extend(screening_suffix(attestation.as_ref()));
    let call = Call {
        contract_address: *pool,
        entrypoint: APPLY_ACTIONS.into(),
        calldata,
    };
    let proof = Proof {
        data: result.proof,
        output,
        proof_facts,
    };
    judge_proof(call, proof, pool, &limits.clamped())
}

#[cfg(test)]
#[path = "prover_tests.rs"]
mod tests;
