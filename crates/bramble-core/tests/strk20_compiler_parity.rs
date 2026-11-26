//! The Rust port of the STRK20 SDK's compiler (#182, S3, `privacy::compiler`) against the SDK jobs
//! the extension recorded (`fixtures/extension-strk20-jobs.jsonl`). For every job: the walks'
//! inputs are the compiler's (the recipients of the first outgoing body, an incoming walk exactly
//! when one was recorded); the walks run over the recorded replies; then, with the job's random
//! bytes replayed, the client actions are the ones the SDK signed for, felt for felt, drawn with
//! the SDK's reads, every byte consumed.

use serde_json::Value;
use starknet_wallet_core::privacy::client_actions::compile_actions_calldata;
use starknet_wallet_core::privacy::compiler::{compile, walk_inputs, CompileRefusal, Compiled};
use starknet_wallet_core::privacy::discovery::{AddressMap, Note, Walked};
use starknet_wallet_core::privacy::engine::EngineAction;
use starknet_wallet_core::privacy::entropy::Entropy;
use starknet_wallet_core::privacy::incoming::IncomingWalk;
use starknet_wallet_core::privacy::outgoing::{OutgoingChannels, OutgoingWalk};
use starknet_wallet_core::privacy::types::PrivacyFailure;
use starknet_wallet_core::privacy::BlockRef;
use starknet_wallet_core::{felt, Error, Felt, SecretFelt};

fn jobs() -> Vec<Value> {
    include_str!("fixtures/extension-strk20-jobs.jsonl")
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn hex(value: &Value) -> Felt {
    felt::parse(value.as_str().unwrap()).unwrap()
}

fn decimal(value: &Value) -> Felt {
    Felt::from_dec_str(value.as_str().unwrap()).unwrap()
}

/// The extension's `Strk20Action`s as `strk20BuilderFor` adds them, after `register()` for the
/// registration and activation builders.
fn reviewed(inputs: &Value) -> Vec<EngineAction> {
    let mut reviewed = Vec::new();
    if matches!(
        inputs["builder"].as_str().unwrap(),
        "registration" | "activate"
    ) {
        reviewed.push(EngineAction::Register);
    }
    for action in inputs["actions"].as_array().unwrap() {
        let token = hex(&action["token"]);
        let recipient = || hex(&action["recipient"]);
        reviewed.push(
            match (action["type"].as_str().unwrap(), &action["amount"]) {
                ("deposit", amount) => EngineAction::Deposit {
                    token,
                    amount: decimal(amount),
                    recipient: action.get("recipient").map(hex),
                },
                ("transfer", amount) if amount == "OPEN" => EngineAction::OpenTransfer {
                    token,
                    recipient: recipient(),
                },
                ("transfer", amount) => EngineAction::Transfer {
                    token,
                    recipient: recipient(),
                    amount: decimal(amount),
                },
                ("withdraw", amount) => EngineAction::Withdraw {
                    token,
                    recipient: recipient(),
                    amount: decimal(amount),
                },
                (other, _) => panic!("unknown action {other}"),
            },
        );
    }
    reviewed
}

fn requests<'a>(job: &'a Value, route: &str) -> Vec<&'a Value> {
    job["requests"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|request| request["route"] == route)
        .collect()
}

/// Replays one route: each body must be the walk's, a 409 ends the walk with `Reorg` and the
/// next request starts a fresh one. Returns the last completed walk's result.
fn replay<W, T>(
    id: &str,
    recorded: &[&Value],
    fresh: impl Fn() -> W,
    body: impl Fn(&W) -> Vec<u8>,
    step: impl Fn(W, u16, &[u8]) -> Result<Walked<W, T>, PrivacyFailure>,
) -> Option<T> {
    let (mut walk, mut done) = (None, None);
    for request in recorded {
        let current = walk.take().unwrap_or_else(&fresh);
        assert_eq!(
            String::from_utf8(body(&current)).unwrap(),
            request["body"].as_str().unwrap(),
            "{id}"
        );
        let status = u16::try_from(request["reply"]["status"].as_u64().unwrap()).unwrap();
        let reply = request["reply"]["body"].as_str().unwrap().as_bytes();
        match step(current, status, reply) {
            Ok(Walked::More(next)) => walk = Some(next),
            Ok(Walked::Done(found)) => done = Some(found),
            Err(failure) => assert_eq!((status, failure), (409, PrivacyFailure::Reorg), "{id}"),
        }
    }
    assert!(walk.is_none(), "{id}: a walk was left waiting");
    done
}

/// Hands out the job's bytes, recording each read's length; a read past the end fails.
struct Stream {
    bytes: Vec<u8>,
    at: usize,
    reads: Vec<usize>,
}

impl Entropy for Stream {
    fn fill(&mut self, out: &mut [u8]) -> Result<(), Error> {
        let end = self.at + out.len();
        let bytes = self
            .bytes
            .get(self.at..end)
            .ok_or_else(|| Error::storage("the recorded stream ran out"))?;
        out.copy_from_slice(bytes);
        self.at = end;
        self.reads.push(out.len());
        Ok(())
    }
}

fn stream(job: &Value) -> Stream {
    let text = job["random"]["bytes"].as_str().unwrap();
    Stream {
        bytes: (0..text.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap())
            .collect(),
        at: 0,
        reads: Vec::new(),
    }
}

/// Runs one job up to the compiler's result, checking the walks' inputs on the way.
fn run(job: &Value) -> (Result<Compiled, CompileRefusal>, Stream) {
    let id = job["id"].as_str().unwrap();
    let inputs = &job["inputs"];
    let (pool, account) = (hex(&inputs["pool"]), hex(&inputs["account"]));
    let key = SecretFelt::new(hex(&inputs["viewingKey"]));
    let block = BlockRef::Number(inputs["provingBlock"].as_u64().unwrap());
    let reviewed = reviewed(inputs);
    let walks = walk_inputs(&account, &reviewed).unwrap();

    let outgoing = requests(job, "outgoing");
    let sent: Value = serde_json::from_str(outgoing[0]["body"].as_str().unwrap()).unwrap();
    let recipients: Vec<Felt> = sent["recipients"]
        .as_array()
        .unwrap()
        .iter()
        .map(hex)
        .collect();
    assert_eq!(walks.recipients, recipients, "{id}: getRecipientsNeeded");
    let channels: OutgoingChannels = replay(
        id,
        &outgoing,
        || OutgoingWalk::new(pool, account, walks.recipients.clone(), block.clone()),
        |walk| walk.body(&key).to_vec(),
        |walk, status, reply| walk.on_reply(status, reply),
    )
    .unwrap();

    let incoming = requests(job, "incoming");
    assert_eq!(
        walks.tokens.is_empty(),
        incoming.is_empty(),
        "{id}: tokensToDiscover"
    );
    let notes: AddressMap<Vec<Note>> = if incoming.is_empty() {
        AddressMap::default()
    } else {
        replay(
            id,
            &incoming,
            || IncomingWalk::new(pool, account, walks.tokens.clone(), block.clone(), 64),
            |walk| walk.body(&key).to_vec(),
            |walk, status, reply| walk.on_reply(status, reply),
        )
        .unwrap()
    };

    let mut stream = stream(job);
    let compiled = compile(&account, &key, &reviewed, &channels, &notes, &mut stream);
    (compiled, stream)
}

#[test]
fn every_recorded_job_compiles_to_what_the_sdk_signed() {
    let mut compiled_jobs = 0;
    for job in jobs() {
        let id = job["id"].as_str().unwrap();
        let (compiled, stream) = run(&job);
        let reads: Vec<usize> = serde_json::from_value(job["random"]["reads"].clone()).unwrap();
        assert_eq!(stream.reads, reads, "{id}: random reads");
        assert_eq!(
            stream.at,
            stream.bytes.len(),
            "{id}: every byte is consumed"
        );
        if id == "insufficient-balance" {
            assert_eq!(compiled.unwrap_err(), CompileRefusal::InsufficientBalance);
            assert!(job["signing"].as_array().unwrap().is_empty());
            continue;
        }
        let compiled = compiled.unwrap_or_else(|refusal| panic!("{id}: {refusal:?}"));
        let inputs = &job["inputs"];
        let calldata = compile_actions_calldata(
            &hex(&inputs["account"]),
            &SecretFelt::new(hex(&inputs["viewingKey"])),
            &compiled.actions,
        )
        .unwrap();
        let ours: Vec<Felt> = calldata.iter().map(|felt| *felt.expose_secret()).collect();
        let call = &job["signing"][0]["calls"][0];
        assert_eq!(call["entrypoint"], "compile_actions", "{id}");
        let signed: Vec<Felt> = call["calldata"]
            .as_array()
            .unwrap()
            .iter()
            .map(decimal)
            .collect();
        assert_eq!(ours, signed, "{id}: compile_actions calldata");
        // The SDK signed `user-linkage` before the extension refused its warning.
        let warned = !job["sdk"]["createProofInvocation"]["warnings"]
            .as_array()
            .unwrap()
            .is_empty();
        assert_eq!(compiled.user_linkage, warned, "{id}: USER_LINKAGE");
        assert_eq!(compiled.user_linkage, id == "user-linkage", "{id}");
        compiled_jobs += 1;
    }
    assert_eq!(compiled_jobs, 12);
}
