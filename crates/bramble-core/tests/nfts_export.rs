//! P17 through mobile's exports, end to end, against what the extension's own code did
//! (`fixtures/extension-nfts.jsonl`): discovery, each candidate's re-proof and a watched NFT's
//! metadata (block `discover`), a fetched metadata document (block `metadata`), and an NFT send
//! from its request to its plan's review, or the same refusal (block `transfer`). The host's
//! work is the test's: it answers each request from the case's recorded exchanges. The gallery's
//! merge of discovered and watched NFTs (`mergeNftCandidates`) is the host's too, so the test
//! reads the merged reply's ids and art only.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

use crate::send_support;

use std::collections::BTreeMap;
use std::sync::Arc;

use bramble_core::{
    nft_metadata_judge, HttpOutcome, NetworkId, NftCoverage, NftDiscoveryStart, NftMetadata,
    NftMetadataSource, RpcReply, RpcRequest, ScanStep, SendStep, StepReply, TokenPreferences,
    TransferPlan,
};
use send_support::*;
use serde_json::{json, Value};
use starknet_wallet_core::js::felt::address_label;
use starknet_wallet_core::Felt;

fn case(block: &str, name: &str) -> Value {
    include_str!("fixtures/extension-nfts.jsonl")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|case| case["block"] == block && case["name"] == name)
        .unwrap_or_else(|| panic!("{block}: {name}"))
}

fn prefs() -> Arc<TokenPreferences> {
    TokenPreferences::new(None)
}

/// `value` with every hex string in one spelling, so a recorded request matches the core's.
fn spelled(value: &Value) -> Value {
    match value {
        Value::String(text) if text.starts_with("0x") => json!(canon(text)),
        Value::Array(items) => Value::Array(items.iter().map(spelled).collect()),
        Value::Object(map) => map.iter().map(|(k, v)| (k.clone(), spelled(v))).collect(),
        other => other.clone(),
    }
}

/// The recorded reply to `request` among `exchanges`, matched by method and params.
fn answer(exchanges: &Value, request: &RpcRequest) -> RpcReply {
    let body: Value = serde_json::from_str(&request.body).unwrap();
    let recorded = exchanges
        .as_array()
        .unwrap()
        .iter()
        .find(|e| {
            e["method"] == body["method"] && spelled(&e["params"]) == spelled(&body["params"])
        })
        .unwrap_or_else(|| panic!("no recorded reply to {body}"));
    RpcReply {
        id: request.id,
        status: 200,
        body: Some(reply_body(request.id, &recorded["reply"])),
    }
}

fn outcome(reply: &Value) -> HttpOutcome {
    HttpOutcome {
        status: 200,
        retry_after: None,
        body: Some(reply["body"].to_string().into_bytes()),
        timed_out: false,
    }
}

/// Drives a read over `exchanges` until it asks for nothing more.
fn drive(
    next: impl Fn() -> Vec<RpcRequest>,
    accept: impl Fn(RpcReply) -> ScanStep,
    exchanges: &Value,
) {
    while let Some(request) = next().first() {
        assert_eq!(accept(answer(exchanges, request)), ScanStep::Continue);
    }
}

#[test]
fn discovery_reproof_and_metadata_through_the_export() {
    let case = case(
        "discover",
        "watched NFTs merge after the indexer's, with metadata for watched ones only",
    );
    let (dir, core) = wallet();
    let account = oz_id(&core, NetworkId::Mainnet, 0);
    // The person's watch list, as the extension stores it.
    let document = json!({ "nfts": case["watched"] }).to_string();
    std::fs::write(dir.path().join("nfts.v1.json"), document).unwrap();

    // Starkscan: one request, judged into untrusted candidates.
    let http = &case["http"][0];
    let NftDiscoveryStart::Send { request, pending } =
        core.nft_discovery(account.clone(), prefs()).unwrap()
    else {
        panic!("Starkscan runs on Mainnet");
    };
    assert_eq!(request.url, http["request"]["url"].as_str().unwrap());
    let discovered = pending.judge(outcome(&http["reply"]));
    assert_eq!(discovered.coverage, NftCoverage::Complete);

    // Every candidate, discovered or watched (spam left out), re-proved on chain.
    let watched = core.watched_nft_candidates(account.clone()).unwrap();
    assert_eq!(watched.len(), 3, "the spam-hidden watch is no candidate");
    let mut candidates: BTreeMap<String, (String, String)> = BTreeMap::new();
    for nft in &discovered.nfts {
        candidates.insert(nft.id.clone(), (nft.contract.clone(), nft.token_id.clone()));
    }
    for nft in &watched {
        candidates.insert(nft.id.clone(), (nft.contract.clone(), nft.token_id.clone()));
    }
    let exchanges = &case["exchanges"];
    let mut owned = Vec::new();
    for (id, (contract, token_id)) in &candidates {
        let read = core
            .nft_ownership_read(account.clone(), contract.clone(), token_id.clone())
            .unwrap();
        drive(|| read.next_calls(), |reply| read.accept(reply), exchanges);
        if read.finish().unwrap().owned {
            owned.push(id.clone());
        }
    }
    let shown: Vec<&Value> = case["reply"]["nfts"].as_array().unwrap().iter().collect();
    let mut expected: Vec<String> = shown
        .iter()
        .map(|nft| nft["id"].as_str().unwrap().into())
        .collect();
    expected.sort();
    assert_eq!(owned, expected, "someone else's token is dropped");

    // Metadata for the watched ones, through the token URI read.
    for nft in shown {
        let (contract, token_id) = &candidates[nft["id"].as_str().unwrap()];
        let read = bramble_core::NftTokenUriRead::new(contract.clone(), token_id.clone()).unwrap();
        drive(|| read.next_calls(), |reply| read.accept(reply), exchanges);
        let NftMetadataSource::Resolved { metadata } = read.finish() else {
            panic!("a data: URI resolves without a request");
        };
        match metadata {
            None => assert_eq!(nft["artUnavailable"], json!(true)),
            Some(metadata) => {
                // Departure (`media.rs`): the core drops a URL the WHATWG parser would respell
                // rather than respell it, so `{id}` left in an image is no image here, where the
                // extension shows `%7Bid%7D`.
                let image = nft["image"].as_str().unwrap();
                let image = if image.contains("%7Bid%7D") {
                    ""
                } else {
                    image
                };
                assert_eq!(
                    (metadata.name.as_str(), metadata.image.as_str()),
                    (nft["name"].as_str().unwrap(), image)
                );
            }
        }
    }
}

#[test]
fn a_fetched_document_is_judged_and_its_animation_left_behind() {
    let case = case("metadata", "https");
    let result = &case["result"];
    assert!(result["animationUrl"].is_string(), "the extension kept one");
    let metadata = nft_metadata_judge(outcome(&case["exchanges"][0]["reply"])).unwrap();
    let NftMetadata {
        name,
        collection,
        description,
        image,
        external_url,
        attributes,
    } = metadata;
    assert_eq!(
        (name, collection, description, image, external_url),
        (
            result["name"].as_str().unwrap().into(),
            result["collection"].as_str().unwrap().into(),
            result["description"].as_str().unwrap().into(),
            result["image"].as_str().unwrap().into(),
            result["externalUrl"].as_str().unwrap().into(),
        )
    );
    let attributes: Vec<Value> = attributes
        .into_iter()
        .map(|a| {
            let mut out = json!({ "traitType": a.trait_type, "value": a.value });
            if let Some(display) = a.display_type {
                out["displayType"] = json!(display);
            }
            out
        })
        .collect();
    assert_eq!(Value::Array(attributes), result["attributes"]);
}

/// The plan run to its review, recording the calldata the fee was estimated over.
fn review(plan: &TransferPlan) -> Vec<String> {
    let mut node = sepolia(true);
    let mut estimated = Vec::new();
    let mut step = plan.advance(StepReply::Start).unwrap();
    loop {
        match &step {
            SendStep::Rpc { .. } => {
                let (id, method, params, _) = rpc_parts(&step);
                if method == "starknet_estimateFee" {
                    estimated = felts(&params["request"][0]["calldata"]);
                }
                let reply = node.reply(&method, &params, None);
                step = plan.advance(rpc_reply(id, &reply)).unwrap();
            }
            SendStep::Review => return estimated,
            other => panic!("unexpected {other:?}"),
        }
    }
}

#[test]
fn an_nft_send_is_planned_and_reviewed_as_the_extension_does() {
    let (_dir, core) = wallet();
    let account = oz_id(&core, NetworkId::Sepolia, 0);
    for name in [
        "an ERC-721 to a saved contact",
        "an ERC-1155 (one edition) to a saved contact",
        "the largest token id",
        "to an address the wallet does not know",
        "to a lookalike of a saved contact",
    ] {
        let case = case("transfer", name);
        let message = &case["review"]["message"];
        let text = |key: &str| message[key].as_str().unwrap().to_string();
        let send = core
            .clone()
            .prepare_nft_transfer(
                account.clone(),
                text("contract"),
                text("tokenId"),
                text("recipient"),
                prefs(),
            )
            .unwrap();
        drive(
            || send.next_calls(),
            |reply| send.accept(reply),
            &case["review"]["exchanges"],
        );
        let plan = match send.plan(None) {
            Err(refusal) => {
                assert_eq!(
                    json!(refusal.to_string()),
                    case["send"]["reply"]["error"],
                    "{name}"
                );
                continue;
            }
            Ok(plan) => plan,
        };
        let estimated = review(&plan);
        let recorded = case["review"]["exchanges"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["method"] == "starknet_estimateFee")
            .unwrap();
        assert_eq!(
            estimated,
            felts(&recorded["params"]["request"][0]["calldata"]),
            "{name}"
        );
        let expected = &case["review"]["reply"]["review"];
        // Departure: the extension's decoder reads starknet.js's decimal calldata, so its line
        // names the recipient in decimal; the core names it as every address is shown
        // (`address_label`).
        let [text] = expected["summaryTexts"].as_array().unwrap().as_slice() else {
            panic!("{name}: one call");
        };
        let decimal = text["facts"]["to"].as_str().unwrap();
        let label = address_label(&format!("{:#x}", Felt::from_dec_str(decimal).unwrap()));
        let summary = expected["summaries"][0].as_str().unwrap();
        assert_eq!(
            plan.current_review().unwrap().summary,
            summary.replace(decimal, &label),
            "{name}"
        );
        assert_eq!(
            plan.risk_flags().len(),
            expected["flags"].as_array().unwrap().len(),
            "{name}"
        );
    }
}

#[test]
fn a_send_of_an_nft_the_account_does_not_hold_is_refused() {
    let (_dir, core) = wallet();
    let account = oz_id(&core, NetworkId::Sepolia, 0);
    for name in [
        "an NFT the account does not own",
        "an ERC-1155 balance of 0",
    ] {
        let case = case("transfer", name);
        let message = &case["review"]["message"];
        let text = |key: &str| message[key].as_str().unwrap().to_string();
        let send = core
            .clone()
            .prepare_nft_transfer(
                account.clone(),
                text("contract"),
                text("tokenId"),
                text("recipient"),
                prefs(),
            )
            .unwrap();
        drive(
            || send.next_calls(),
            |reply| send.accept(reply),
            &case["review"]["exchanges"],
        );
        let refusal = send.plan(None).err().unwrap();
        assert_eq!(
            json!(refusal.to_string()),
            case["review"]["reply"]["error"],
            "{name}"
        );
    }
}

#[test]
fn a_watch_is_stored_only_from_a_read_that_found_the_token() {
    let case = case(
        "discover",
        "Starkscan candidates re-proved: owned kept, someone else's dropped",
    );
    let (_dir, core) = wallet();
    let account = oz_id(&core, NetworkId::Mainnet, 0);
    let exchanges = &case["exchanges"];
    let read = |contract: &str, token: &str| {
        let read = core
            .nft_ownership_read(account.clone(), contract.into(), token.into())
            .unwrap();
        drive(|| read.next_calls(), |reply| read.accept(reply), exchanges);
        read
    };
    let theirs = read(
        "0x3c0ffee1d2c3b4a5968778695a4b3c2d1e0f1a2b3c4d5e6f708192a3b4c5d6e",
        "2",
    );
    let refusal = core.watch_nft(theirs, None).err().unwrap();
    assert_eq!(refusal.to_string(), "this account does not own that NFT");
    let ours = read(
        "0x7a11ce5e1f2d3c4b5a69788796a5b4c3d2e1f0a1b2c3d4e5f60718293a4b5c6",
        "1",
    );
    let watched = core.watch_nft(ours, Some(" Punk ".into())).unwrap();
    assert_eq!(watched.label.as_deref(), Some("Punk"));
    core.set_nft_spam(watched.id.clone(), true).unwrap();
    assert!(core.watched_nft_candidates(account).unwrap().is_empty());
    assert_eq!(core.watched_nfts().unwrap()[0].spam, Some(true));
    // Mobile stores only an https indexer, as for an RPC override.
    assert!(core
        .set_nft_indexer_url("http://localhost:8080".into())
        .is_err());
    core.set_nft_indexer_url(" https://indexer.example/api ".into())
        .unwrap();
    assert_eq!(
        core.nft_indexer_url().unwrap().as_deref(),
        Some("https://indexer.example/api")
    );
}

#[test]
fn a_lock_voids_a_read_and_closes_the_watched_list() {
    let case = case(
        "discover",
        "Starkscan candidates re-proved: owned kept, someone else's dropped",
    );
    let (_dir, core) = wallet();
    let account = oz_id(&core, NetworkId::Mainnet, 0);
    let read = core
        .nft_ownership_read(
            account,
            "0x7a11ce5e1f2d3c4b5a69788796a5b4c3d2e1f0a1b2c3d4e5f60718293a4b5c6".into(),
            "1".into(),
        )
        .unwrap();
    drive(
        || read.next_calls(),
        |reply| read.accept(reply),
        &case["exchanges"],
    );
    core.lock();
    let locked = "wallet is locked";
    assert_eq!(core.watched_nfts().err().unwrap().to_string(), locked);
    assert_eq!(core.nft_indexer_url().err().unwrap().to_string(), locked);
    assert_eq!(core.nfts_shown().err().unwrap().to_string(), locked);
    assert_eq!(
        core.set_nfts_shown(false).err().unwrap().to_string(),
        locked
    );
    assert_eq!(
        core.remove_nft("x".into()).err().unwrap().to_string(),
        locked
    );
    assert_eq!(
        core.set_nft_spam("x".into(), true)
            .err()
            .unwrap()
            .to_string(),
        locked
    );
    let set = core.set_nft_indexer_url("https://indexer.example".into());
    assert_eq!(set.err().unwrap().to_string(), locked);
    // A read made before the lock proves nothing in the next session.
    core.unlock(PASSWORD.into()).unwrap();
    assert_eq!(
        core.watch_nft(read, None).err().unwrap().to_string(),
        locked
    );
    assert!(core.watched_nfts().unwrap().is_empty());
}
