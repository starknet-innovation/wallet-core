//! What the STRK20 parity tests share: the recorded single-signer proof jobs (the lines of
//! `extension-strk20-jobs.jsonl` with neither a `kind` nor a `multisig` record), the multisig jobs
//! (`strk20_multisig_parity`), the extension's actions as the core's reviewed actions, the job's
//! contracts, and the refusal each recorded error names. The `kind` lines are the shadow-account
//! reads (`strk20_shadow_parity`) and the multisig review and refusals (`strk20_multisig_parity`).

use bramble_core::strk20::invoke_calldata_item;
use serde_json::Value;
use starknet_wallet_core::privacy::compiler::{CompileRefusal, Contracts};
use starknet_wallet_core::privacy::entropy::Entropy;
use starknet_wallet_core::privacy::job::{ProofHost, ProofJob, ProofStep};
use starknet_wallet_core::privacy::proof::{CollectPolicy, ReviewedAction};
use starknet_wallet_core::privacy::types::PrivacyReply;
use starknet_wallet_core::send::calls::Call;
use starknet_wallet_core::{encode_short_string, felt, Felt};

pub const JOBS: &str = include_str!("fixtures/extension-strk20-jobs.jsonl");

/// Drives `job` from `Start` (at `now_ms`) until it ends, asking `answer` for the host's side of
/// each `Post`, `Rpc` and `Wait`; returns the terminal step. The one loop the replays and the
/// live smoke harness (`strk20_live_smoke`) share.
pub fn drive<E: Entropy>(
    job: &mut ProofJob<E>,
    host: &dyn ProofHost,
    now_ms: u64,
    mut answer: impl FnMut(&ProofStep) -> PrivacyReply,
) -> ProofStep {
    let mut step = job
        .advance(host, PrivacyReply::Start { now_ms })
        .expect("the job starts");
    while matches!(
        step,
        ProofStep::Post { .. } | ProofStep::Rpc { .. } | ProofStep::Wait { .. }
    ) {
        let reply = answer(&step);
        step = job
            .advance(host, reply)
            .expect("the job accepts each reply");
    }
    step
}

/// Every line of the fixture.
pub fn lines() -> Vec<Value> {
    JOBS.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// The single-signer proof jobs: every line without a `kind` or a `multisig` record.
pub fn proof_jobs() -> Vec<Value> {
    lines()
        .into_iter()
        .filter(|line| line.get("kind").is_none() && !is_multisig(line))
        .collect()
}

/// A multisig line: a job with `inputs.multisig` and a top-level `multisig` record, or a review
/// or refusal line (`multisig-*` kind).
pub fn is_multisig(line: &Value) -> bool {
    let job = line["inputs"].get("multisig").is_some() && line.get("multisig").is_some();
    let kind = line["kind"]
        .as_str()
        .is_some_and(|kind| kind.starts_with("multisig-"));
    job || kind
}

fn hex(value: &Value) -> Felt {
    felt::parse(value.as_str().unwrap()).unwrap()
}

fn decimal(value: &Value) -> Felt {
    Felt::from_dec_str(value.as_str().unwrap()).unwrap()
}

/// `BigInt(text)`: hex with `0x`, else decimal.
fn bigint_u128(value: &Value) -> u128 {
    let text = value.as_str().unwrap();
    match text.strip_prefix("0x") {
        Some(digits) => u128::from_str_radix(digits, 16).unwrap(),
        None => text.parse().unwrap(),
    }
}

/// One `shadow_account_invoke` call (`INVOKE_CALL`).
fn call(value: &Value) -> Call {
    Call {
        contract_address: hex(&value["contract_address"]),
        entrypoint: value["entry_point"].as_str().unwrap().to_owned(),
        calldata: value["calldata"]
            .as_array()
            .unwrap()
            .iter()
            .map(hex)
            .collect(),
    }
}

/// The extension's `Strk20Action`s as `strk20BuilderFor` adds them, after `register()` for the
/// registration and activation builders. An invoke's calldata goes through Bramble's placeholder
/// grammar; a shadow invoke's `dapp_name` is already the origin-bound scope
/// (`bindStrk20ActionsToOrigin`), which the SDK encodes as a short string.
pub fn reviewed(inputs: &Value) -> Vec<ReviewedAction> {
    let mut reviewed = Vec::new();
    if matches!(
        inputs["builder"].as_str().unwrap(),
        "registration" | "activate"
    ) {
        reviewed.push(ReviewedAction::Register);
    }
    for action in inputs["actions"].as_array().unwrap() {
        let token = || hex(&action["token"]);
        let recipient = || hex(&action["recipient"]);
        let amount = &action["amount"];
        reviewed.push(match action["type"].as_str().unwrap() {
            "deposit" => ReviewedAction::Deposit {
                token: token(),
                amount: decimal(amount),
                recipient: action.get("recipient").map(hex),
            },
            "transfer" if amount == "OPEN" => ReviewedAction::OpenTransfer {
                token: token(),
                recipient: recipient(),
            },
            "transfer" => ReviewedAction::Transfer {
                token: token(),
                recipient: recipient(),
                amount: decimal(amount),
            },
            "withdraw" => ReviewedAction::Withdraw {
                token: token(),
                recipient: recipient(),
                amount: decimal(amount),
            },
            "invoke" => ReviewedAction::Invoke {
                contract: hex(&action["contract"]),
                calldata: action["calldata"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|item| invoke_calldata_item(item.as_str().unwrap()).unwrap())
                    .collect(),
            },
            "shadow_account_invoke" => {
                let policy = &action["collect_policy"];
                ReviewedAction::ShadowInvoke {
                    dapp_name: encode_short_string(action["dapp_name"].as_str().unwrap()).unwrap(),
                    nonce: hex(&action["nonce"]),
                    calls: action["calls"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(call)
                        .collect(),
                    collect: match policy["type"].as_str().unwrap() {
                        "all" => CollectPolicy::All,
                        "diff" => CollectPolicy::Diff,
                        "exact" => CollectPolicy::Exact(bigint_u128(&policy["amount"])),
                        other => panic!("unknown collect policy {other}"),
                    },
                }
            }
            other => panic!("unknown action {other}"),
        });
    }
    reviewed
}

/// The job's pool and anonymizer (absent or `null` when the network has none).
pub fn contracts(inputs: &Value) -> Contracts {
    Contracts {
        pool: hex(&inputs["pool"]),
        anonymizer: inputs
            .get("anonymizer")
            .and_then(Value::as_str)
            .map(|anonymizer| felt::parse(anonymizer).unwrap()),
    }
}

/// The compiler refusal a refused job's recorded error names: the SDK's (`createProofInvocation`)
/// or, for what the extension refuses before the SDK runs (nothing signed), the job's outcome.
/// `None` for a job the compiler did not refuse (a `USER_LINKAGE` job signed before the extension
/// refused it).
pub fn compile_refusal(job: &Value) -> Option<CompileRefusal> {
    let message = job["sdk"]["createProofInvocation"]["error"]["message"]
        .as_str()
        .or_else(|| {
            let unsigned = job["signing"].as_array().is_some_and(Vec::is_empty);
            job["outcome"]["error"]["message"]
                .as_str()
                .filter(|_| unsigned)
        })?;
    let refusal = [
        (
            "Insufficient balance for token",
            CompileRefusal::InsufficientBalance,
        ),
        (
            "Missing channel context for recipient",
            CompileRefusal::RecipientNotRegistered,
        ),
        (
            "Channel already exists for",
            CompileRefusal::AlreadyRegistered,
        ),
        // `assertNoInvokePhaseAction`, wrapped by the extension's service error.
        (
            "strk20 services unavailable (discovery/service-unavailable): At most one \
             invoke-phase action",
            CompileRefusal::MultipleInvokes,
        ),
        // `expandInvokeCalldata`: an `${openNoteIds[N]}` past the batch's open notes.
        (
            "invalid strk20 action batch",
            CompileRefusal::OpenNoteOutOfRange,
        ),
    ]
    .into_iter()
    .find(|(prefix, _)| message.starts_with(prefix))
    .map(|(_, refusal)| refusal);
    // `prepareWith`'s bare refusal of a shadow invoke with no anonymizer configured.
    let no_anonymizer =
        message == "strk20 services unavailable" && contracts(&job["inputs"]).anonymizer.is_none();
    Some(
        refusal
            .or(no_anonymizer.then_some(CompileRefusal::NoAnonymizer))
            .unwrap_or_else(|| panic!("an error this test does not map: {message}")),
    )
}
