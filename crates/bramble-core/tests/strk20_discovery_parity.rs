//! The proof job's discovery walks (`privacy::outgoing`, `privacy::incoming`) against the SDK jobs
//! the extension recorded (`fixtures/extension-strk20-jobs.jsonl`): every `outgoing_state` and
//! `incoming_state` body, byte for byte, fed the recorded reply. The SDK runs the two walks
//! concurrently, so each route is replayed in its own order.
//!
//! The walk's inputs that S3 will derive are read from the transcript instead: the recipients
//! from the first recorded outgoing body (the SDK's `getRecipientsNeeded`), and the token filter
//! from the tokens the actions spend (`transfer`, `withdraw`), the only deficits these jobs have.

use serde_json::Value;
use starknet_wallet_core::felt;
use starknet_wallet_core::privacy::discovery::Walked;
use starknet_wallet_core::privacy::incoming::IncomingWalk;
use starknet_wallet_core::privacy::outgoing::OutgoingWalk;
use starknet_wallet_core::privacy::types::PrivacyFailure;
use starknet_wallet_core::privacy::BlockRef;
use starknet_wallet_core::{Felt, SecretFelt};

use crate::strk20_support::proof_jobs;

fn hex(value: &Value) -> Felt {
    felt::parse(value.as_str().unwrap()).unwrap()
}

fn requests<'a>(job: &'a Value, route: &str) -> Vec<&'a Value> {
    job["requests"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|request| request["route"] == route)
        .collect()
}

fn reply(request: &Value) -> (u16, &[u8]) {
    let status = u16::try_from(request["reply"]["status"].as_u64().unwrap()).unwrap();
    (
        status,
        request["reply"]["body"].as_str().unwrap().as_bytes(),
    )
}

struct Job {
    id: String,
    pool: Felt,
    account: Felt,
    key: SecretFelt,
    block: BlockRef,
}

impl Job {
    fn new(job: &Value) -> Self {
        let inputs = &job["inputs"];
        Self {
            id: job["id"].as_str().unwrap().to_owned(),
            pool: hex(&inputs["pool"]),
            account: hex(&inputs["account"]),
            key: SecretFelt::new(hex(&inputs["viewingKey"])),
            block: BlockRef::Number(inputs["provingBlock"].as_u64().unwrap()),
        }
    }
}

/// Replays one route's requests: each body must be the walk's, each reply advances it, a 409 ends
/// it with `Reorg` and the next request starts a fresh walk. Returns how many walks completed.
fn replay<W>(
    job: &Job,
    recorded: &[&Value],
    fresh: impl Fn() -> W,
    body: impl Fn(&W) -> Vec<u8>,
    step: impl Fn(W, u16, &[u8]) -> Result<Option<W>, PrivacyFailure>,
) -> usize {
    let mut walk = None;
    let mut completed = 0;
    for (index, request) in recorded.iter().enumerate() {
        let current = walk.take().unwrap_or_else(&fresh);
        assert_eq!(
            String::from_utf8(body(&current)).unwrap(),
            request["body"].as_str().unwrap(),
            "{}: request {index}",
            job.id
        );
        let (status, reply) = reply(request);
        match step(current, status, reply) {
            Ok(Some(next)) => walk = Some(next),
            Ok(None) => completed += 1,
            Err(failure) => {
                assert_eq!(
                    (status, failure),
                    (409, PrivacyFailure::Reorg),
                    "{}",
                    job.id
                )
            }
        }
    }
    assert!(walk.is_none(), "{}: a walk was left waiting", job.id);
    completed
}

/// The replies of the walk that completed: those after the route's last 409.
fn completed_replies(recorded: &[&Value]) -> Vec<Value> {
    let start = recorded
        .iter()
        .rposition(|request| request["reply"]["status"] == 409)
        .map_or(0, |at| at + 1);
    recorded[start..]
        .iter()
        .map(|request| serde_json::from_str(request["reply"]["body"].as_str().unwrap()).unwrap())
        .collect()
}

#[test]
fn every_recorded_discovery_body_is_the_walks() {
    let (mut outgoing_walks, mut incoming_walks) = (0, 0);
    // The shadow-account reads' unfiltered walk is `strk20_shadow_parity`'s.
    for line in proof_jobs() {
        let job = Job::new(&line);
        let outgoing = requests(&line, "outgoing");
        if let Some(first) = outgoing.first() {
            let sent: Value = serde_json::from_str(first["body"].as_str().unwrap()).unwrap();
            let recipients: Vec<Felt> = sent["recipients"]
                .as_array()
                .unwrap()
                .iter()
                .map(hex)
                .collect();
            outgoing_walks += replay(
                &job,
                &outgoing,
                || OutgoingWalk::new(job.pool, job.account, recipients.clone(), job.block.clone()),
                |walk| walk.body(&job.key).to_vec(),
                |walk, status, reply| match walk.on_reply(status, reply)? {
                    Walked::More(walk) => Ok(Some(walk)),
                    Walked::Done(found) => {
                        assert!(found.channels.keys().all(|r| recipients.contains(r)));
                        // Every recipient any page named has a channel, keyed iff some page
                        // reported it created (a precomputed entry never displaces one).
                        let pages = completed_replies(&outgoing);
                        let entries: Vec<&Value> = pages
                            .iter()
                            .flat_map(|page| page["channels"].as_array().unwrap())
                            .collect();
                        for recipient in &recipients {
                            let named: Vec<&&Value> = entries
                                .iter()
                                .filter(|entry| hex(&entry["recipient_addr"]) == *recipient)
                                .collect();
                            let created = named.iter().any(|entry| entry["precomputed"] != true);
                            assert_eq!(
                                found.channels.get(recipient).map(|c| c.key.is_some()),
                                (!named.is_empty()).then_some(created),
                                "{}: channel",
                                job.id
                            );
                        }
                        let total = pages.last().unwrap()["cursor"]["total_n_channels"].as_u64();
                        assert_eq!(found.total.map(u64::from), total, "{}: total", job.id);
                        Ok(None)
                    }
                },
            );
        }
        let incoming = requests(&line, "incoming");
        if !incoming.is_empty() {
            let mut tokens: Vec<Felt> = Vec::new();
            for action in line["inputs"]["actions"].as_array().unwrap() {
                if !matches!(action["type"].as_str(), Some("transfer" | "withdraw")) {
                    continue;
                }
                let token = hex(&action["token"]);
                if !tokens.contains(&token) {
                    tokens.push(token);
                }
            }
            incoming_walks += replay(
                &job,
                &incoming,
                || IncomingWalk::new(job.pool, job.account, tokens.clone(), job.block.clone(), 64),
                |walk| walk.body(&job.key).to_vec(),
                |walk, status, reply| match walk.on_reply(status, reply)? {
                    Walked::More(walk) => Ok(Some(walk)),
                    Walked::Done(notes) => {
                        assert!(notes
                            .iter()
                            .all(|(token, found)| { tokens.contains(token) && !found.is_empty() }));
                        // Every page's notes in a wanted token are kept, whichever page carried
                        // the sender's channel.
                        for token in &tokens {
                            let recorded = completed_replies(&incoming)
                                .iter()
                                .flat_map(|page| page["notes"].as_array().unwrap().clone())
                                .filter(|note| hex(&note["token"]) == *token)
                                .count();
                            assert_eq!(
                                notes.get(token).map_or(0, Vec::len),
                                recorded,
                                "{}: notes",
                                job.id
                            );
                        }
                        Ok(None)
                    }
                },
            );
        }
    }
    // Tracks the fixture: 36 proof jobs, every one that reaches discovery discovers its channels
    // (`reorg-restart`'s first walk ends in a 409, so still one completed; the two invoke-phase
    // refusals before any request discover nothing), 34 in all; the 13 that spend notes
    // (`transfer`, `withdraw` with an amount; S6 added `withdraw-plus-invoke`) discover those too.
    assert_eq!((outgoing_walks, incoming_walks), (34, 13));
}
