//! An Argent multisig's STRK20 proof job (#182, S7) against the extension's recorded multisig
//! lines (`fixtures/extension-strk20-jobs.jsonl`, the lines `strk20_support::is_multisig` names),
//! through `strk20_job_parity`'s replay: every request the job makes is the next one recorded on
//! its route, byte for byte (chain reads as JSON with the job's ids), none invented, none left over
//! unless said.
//!
//! - `multisig-shield-both-signers`: the wallet answers with the recorded aggregate at once; the
//!   job proves without holding.
//! - `multisig-shield-phase-1`: one co-owner short; the job ends held, and the held record, with
//!   Bramble's marker, is the extension's held row and redacted calls.
//! - `multisig-shield-phase-2`: phase 1's record resumed with the recorded aggregate; two chain
//!   reads, then the prover, with no discovery, nonce, randomness or signing.
//! - `multisig-held-aged-out`: phase 1's record resumed past the pool's window ends expired; a
//!   fresh job then holds the action again under a new digest.
//! - `multisig-cosigner-review`: the redaction and the digest only; the review's sentences are
//!   Bramble's UI copy, not the core's.
//! - `multisig-authorization-refusals`: each recorded refusal is one of the aggregate check's.

use std::cell::RefCell;

use bramble_core::catalogs::multisig::argent_multisig_class;
use bramble_core::catalogs::networks;
use bramble_core::strk20::MULTISIG_STRK20_REDACTED_FELT as MARKER;
use serde_json::Value;
use starknet_wallet_core::accounts::multisig::{signer_guid, ArgentMultisigConfig};
use starknet_wallet_core::privacy::call_set::{authorized_aggregate, call_set_hash};
use starknet_wallet_core::privacy::held::{restore_calldata, RedactedHeldProof};
use starknet_wallet_core::privacy::job::{ProofHost, ProofStep};
use starknet_wallet_core::privacy::plan::PrivacyHost;
use starknet_wallet_core::privacy::proof::AuthorizationRefusal;
use starknet_wallet_core::send::types::SignRefusal;
use starknet_wallet_core::{felt, Felt, NetworkId, SecretFelt, StarkSignature};

use crate::strk20_job_parity::{fresh, network, replay, stream, Run};
use crate::strk20_support::{is_multisig, lines};

fn line(id: &str) -> Value {
    lines()
        .into_iter()
        .find(|line| line["id"] == id)
        .unwrap_or_else(|| panic!("no line {id}"))
}

/// `BigInt(text)`: the aggregate's count and signer type are decimal, the rest hex.
fn bigint(value: &Value) -> Felt {
    let text = value.as_str().unwrap();
    felt::parse(text).unwrap_or_else(|| Felt::from_dec_str(text).unwrap())
}

fn felts(value: &Value) -> Vec<Felt> {
    value.as_array().unwrap().iter().map(bigint).collect()
}

/// The multisig wallet: its shared viewing key, and the co-owners' aggregate once they have all
/// signed (`None` while they have not). Never asked for a single signature.
struct Owners {
    network: NetworkId,
    pool: Felt,
    key: Felt,
    aggregate: Option<Vec<Felt>>,
    asked: RefCell<Vec<Felt>>,
}

impl PrivacyHost for Owners {
    fn viewing_key(&self, chain_id: &str, pool_address: &str) -> Option<SecretFelt> {
        assert_eq!(chain_id, networks::info(self.network).chain_id);
        assert_eq!(pool_address, felt::canonical(&self.pool));
        Some(SecretFelt::new(self.key))
    }
}

impl ProofHost for Owners {
    fn sign(&self, _: &Felt) -> Result<StarkSignature, SignRefusal> {
        panic!("a multisig is never asked for one signature")
    }

    fn authorization(&self, hash: &Felt) -> Result<Option<Vec<Felt>>, SignRefusal> {
        self.asked.borrow_mut().push(*hash);
        Ok(self.aggregate.clone())
    }
}

fn owners(job: &Value, aggregate: Option<Vec<Felt>>) -> Owners {
    let inputs = &job["inputs"];
    Owners {
        network: network(job),
        pool: bigint(&inputs["pool"]),
        key: bigint(&inputs["viewingKey"]),
        aggregate,
        asked: RefCell::default(),
    }
}

fn config(job: &Value) -> ArgentMultisigConfig {
    let multisig = &job["inputs"]["multisig"];
    ArgentMultisigConfig {
        threshold: u32::try_from(multisig["threshold"].as_u64().unwrap()).unwrap(),
        signer_public_keys: serde_json::from_value(multisig["signerPublicKeys"].clone()).unwrap(),
    }
}

/// The aggregate the extension handed the prover: the recorded prove body's signature.
fn proved_aggregate(job: &Value) -> Vec<Felt> {
    let prover = job["requests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|request| request["route"] == "prover")
        .unwrap();
    let body: Value = serde_json::from_str(prover["body"].as_str().unwrap()).unwrap();
    felts(&body["params"]["transaction"]["signature"])
}

fn assert_proved(run: &Run, job: &Value) {
    let id = job["id"].as_str().unwrap();
    let ProofStep::Finished { proof } = &run.end else {
        panic!("{id}: {:?}", run.end);
    };
    let expected = &job["outcome"]["callAndProof"];
    let call = &expected["call"];
    assert_eq!(
        proof.call.contract_address,
        bigint(&call["contract_address"])
    );
    assert_eq!(proof.call.entrypoint, call["entry_point"].as_str().unwrap());
    assert_eq!(proof.call.calldata, felts(&call["calldata"]), "{id}");
    let recorded = &expected["proof"];
    assert_eq!(proof.proof.data, recorded["data"].as_str().unwrap(), "{id}");
    assert_eq!(proof.proof.output, felts(&recorded["output"]), "{id}");
    assert_eq!(proof.proof.proof_facts, felts(&recorded["proof_facts"]));
}

/// Phase 1 run fresh: the held record it ends in, and the run.
fn phase_one() -> (RedactedHeldProof, Run) {
    let job = line("multisig-shield-phase-1");
    let (owners, mut stream) = (owners(&job, None), stream(&job));
    let run = replay(&job, &owners, fresh(&job, &mut stream));
    assert_eq!(
        stream.at,
        stream.bytes.len(),
        "every recorded byte is drawn"
    );
    let ProofStep::Held { held } = &run.end else {
        panic!("phase 1: {:?}", run.end);
    };
    assert_eq!(*owners.asked.borrow(), [held.digest]);
    (held.redact(&MARKER).unwrap(), run)
}

/// `line`'s job resumed from `held`, with no randomness to draw.
fn resumed(job: &Value, held: RedactedHeldProof, owners: &Owners) -> Run {
    let mut stream = stream(job);
    let run = replay(job, owners, fresh(job, &mut stream).resumed(held, MARKER));
    assert!(stream.reads.is_empty(), "a resumed job draws nothing");
    run
}

#[test]
fn both_signers_at_once_prove_without_holding() {
    let job = line("multisig-shield-both-signers");
    let signing = &job["signing"][0];
    let aggregate = felts(&signing["signature"]);
    // The extension's aggregate as the SDK hands it on: the invocation's `signature`, as `0x` hex.
    assert_eq!(felts(&job["invocation"]["signature"]), aggregate);
    let (owners, mut stream) = (owners(&job, Some(aggregate)), stream(&job));
    let run = replay(&job, &owners, fresh(&job, &mut stream));
    assert_eq!(*owners.asked.borrow(), [bigint(&signing["digest"])]);
    assert_proved(&run, &job);
    assert!(run.leftover.is_empty(), "{:?}", run.leftover);
}

#[test]
fn phase_one_holds_what_the_extension_held() {
    let job = line("multisig-shield-phase-1");
    let (held, run) = phase_one();
    assert!(run.leftover.is_empty(), "{:?}", run.leftover);
    let multisig = &job["multisig"];
    let authorization = &multisig["authorization"];
    assert_eq!(held.digest, bigint(&authorization["hash"]));
    assert_eq!(held.digest, bigint(&job["signing"][0]["digest"]));
    let [row] = &multisig["held"].as_array().unwrap()[..] else {
        panic!("one held row");
    };
    assert_eq!(held.nonce, bigint(&row["envelope"]["nonce"]));
    assert_eq!(held.proving_block, row["provingBlock"].as_u64().unwrap());
    assert_eq!(
        row["id"].as_str().unwrap(),
        format!(
            "{}:{}",
            authorization["accountId"].as_str().unwrap(),
            felt::canonical(&held.digest)
        )
    );
    // The co-owners' calls are the held calldata with Bramble's marker in the key slot.
    let [call] = &authorization["calls"].as_array().unwrap()[..] else {
        panic!("one call");
    };
    assert_eq!(call["entrypoint"], "compile_actions");
    assert_eq!(held.calldata, felts(&call["calldata"]));
}

#[test]
fn phase_two_resumes_phase_one_straight_to_the_prover() {
    let job = line("multisig-shield-phase-2");
    let (held, _) = phase_one();
    let digest = held.digest;
    assert_eq!(digest, bigint(&job["multisig"]["authorization"]["hash"]));
    let owners = owners(&job, Some(proved_aggregate(&job)));
    // Every recorded request: two chain reads and one prove, each byte for byte.
    let routes: Vec<&str> = job["requests"]
        .as_array()
        .unwrap()
        .iter()
        .map(|request| request["route"].as_str().unwrap())
        .collect();
    assert_eq!(routes, ["blockNumber", "call", "prover"]);
    let run = resumed(&job, held, &owners);
    assert_eq!(*owners.asked.borrow(), [digest]);
    assert_proved(&run, &job);
    assert!(run.leftover.is_empty(), "{:?}", run.leftover);
}

#[test]
fn an_aged_out_hold_expires_and_the_action_is_held_again() {
    let job = line("multisig-held-aged-out");
    // The generator's store still held phase 1's request, its authorization complete since
    // phase 2.
    let (held, _) = phase_one();
    let phase_two = line("multisig-shield-phase-2");
    let owners_ready = owners(&job, Some(proved_aggregate(&phase_two)));
    let run = resumed(&job, held, &owners_ready);
    assert_eq!(run.end, ProofStep::Expired);
    assert_eq!(
        run.leftover.into_iter().collect::<Vec<_>>(),
        [("nonce".to_owned(), 1), ("outgoing".to_owned(), 1)]
    );

    let (owners, mut stream) = (owners(&job, None), stream(&job));
    let run = replay(&job, &owners, fresh(&job, &mut stream));
    assert_eq!(
        stream.at,
        stream.bytes.len(),
        "every recorded byte is drawn"
    );
    let ProofStep::Held { held } = &run.end else {
        panic!("{:?}", run.end);
    };
    assert_eq!(
        held.digest,
        bigint(&job["multisig"]["authorization"]["hash"])
    );
    assert_eq!(
        run.leftover.into_iter().collect::<Vec<_>>(),
        [("blockNumber".to_owned(), 1), ("call".to_owned(), 1)]
    );
    let [row] = &job["multisig"]["held"].as_array().unwrap()[..] else {
        panic!("the aged-out row was dropped, the new one kept");
    };
    assert_eq!(
        row["id"].as_str().unwrap().rsplit(':').next(),
        Some(&*felt::canonical(&held.digest))
    );
    let redacted = held.redact(&MARKER).unwrap();
    assert_eq!(
        redacted.calldata,
        felts(&job["multisig"]["authorization"]["calls"][0]["calldata"])
    );
}

#[test]
fn the_cosigners_calls_restore_to_the_digest() {
    let review = line("multisig-cosigner-review");
    let job = line("multisig-shield-phase-1");
    let (held, _) = phase_one();
    let [call] = &review["calls"].as_array().unwrap()[..] else {
        panic!("one call");
    };
    let shared = felts(&call["calldata"]);
    assert_eq!(
        shared, held.calldata,
        "the co-owner sees phase 1's redaction"
    );
    let key = SecretFelt::new(bigint(&job["inputs"]["viewingKey"]));
    let calldata = restore_calldata(&shared, &key, &MARKER).unwrap();
    let scope = &review["scope"];
    let digest = call_set_hash(
        &bigint(&scope["accountAddress"]),
        &bigint(&call["contractAddress"]),
        call["entrypoint"].as_str().unwrap(),
        &calldata,
        &bigint(&scope["chainId"]),
    );
    assert_eq!(digest, bigint(&review["digest"]));
    assert_eq!(digest, held.digest);
}

#[test]
fn each_recorded_authorization_refusal_is_the_cores() {
    let refusals = line("multisig-authorization-refusals");
    let job = line("multisig-shield-phase-1");
    let config = config(&job);
    assert_eq!(
        u64::from(config.threshold),
        refusals["threshold"].as_u64().unwrap()
    );
    let authorization = &job["multisig"]["authorization"];
    let digest = bigint(&authorization["hash"]);
    let entry = |fragment: &Value| {
        let key = bigint(&fragment["publicKey"]);
        let entry = [
            Felt::ZERO,
            key,
            bigint(&fragment["r"]),
            bigint(&fragment["s"]),
        ];
        (signer_guid(&key).unwrap(), entry)
    };
    let aggregate = |fragments: &[&Value]| {
        let mut entries: Vec<_> = fragments.iter().map(|fragment| entry(fragment)).collect();
        entries.sort_by_key(|(guid, _)| *guid);
        let mut out = vec![Felt::from(entries.len())];
        out.extend(entries.iter().flat_map(|(_, entry)| *entry));
        out
    };
    let check = |aggregate: Vec<Felt>| {
        authorized_aggregate(&aggregate, &digest, &config, argent_multisig_class())
    };
    let own = &authorization["fragments"][0];
    assert_eq!(own["publicKey"], job["inputs"]["multisig"]["activeSigner"]);

    // "Argent multisig requires exactly 2 signature(s)": the one fragment phase 1 collected.
    let message = refusals["belowThresholdAggregate"]["error"]
        .as_str()
        .unwrap();
    assert_eq!(message, "Argent multisig requires exactly 2 signature(s)");
    assert_eq!(
        check(aggregate(&[own])),
        Err(AuthorizationRefusal::BelowThreshold)
    );
    let cases = &refusals["cases"];
    // A fragment from outside the signer set, over the same digest.
    assert!(cases["nonSigner"].is_null());
    let outsider = &refusals["fragments"]["nonSigner"];
    assert_eq!(
        check(aggregate(&[own, outsider])),
        Err(AuthorizationRefusal::NotASigner)
    );
    // The active signer's own fragment again.
    assert!(cases["repeatedSigner"].is_null());
    let (_, twice) = entry(own);
    let mut repeated = vec![Felt::TWO];
    repeated.extend(twice.into_iter().chain(twice));
    assert_eq!(check(repeated), Err(AuthorizationRefusal::RepeatedSigner));
    // The co-signer over another digest. The line's own such signature is not recorded; the
    // co-signer's recorded signature over `multisig-shield-both-signers`' digest stands in.
    assert!(cases["signatureOverAnotherHash"].is_null());
    let both = line("multisig-shield-both-signers");
    let theirs = &both["multisig"]["signCallSet"]["fragments"][1];
    assert_eq!(theirs["signer"], "cosigner");
    assert_ne!(bigint(&both["multisig"]["signCallSet"]["digest"]), digest);
    assert_eq!(
        check(aggregate(&[own, theirs])),
        Err(AuthorizationRefusal::InvalidSignature)
    );
}

#[test]
fn the_multisig_lines_are_all_covered() {
    let ids: Vec<String> = lines()
        .iter()
        .filter(|line| is_multisig(line))
        .map(|line| line["id"].as_str().unwrap().to_owned())
        .collect();
    // Tracks the fixture: the 4 multisig jobs and the review and refusal lines, each a test here.
    assert_eq!(
        ids,
        [
            "multisig-shield-both-signers",
            "multisig-shield-phase-1",
            "multisig-shield-phase-2",
            "multisig-held-aged-out",
            "multisig-cosigner-review",
            "multisig-authorization-refusals",
        ]
    );
}
