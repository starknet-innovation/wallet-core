//! Shadow accounts and the invoke phase (#182, S6) against the pinned SDK and the extension:
//! `fixtures/extension-strk20-shadow.json` (commitments, addresses, the anonymizer's additional
//! data, the two invoke-phase client actions, the extension's origin-bound scopes) and the three
//! shadow-account reads in `fixtures/extension-strk20-jobs.jsonl` (the lines with a `kind`).

use bramble_core::strk20::{shadow_account_scope, shadow_dapp_name, SHADOW_SCAN_LIMITS};
use serde_json::Value;
use starknet_wallet_core::privacy::client_actions::{compile_actions_calldata, ClientAction};
use starknet_wallet_core::privacy::discovery::Walked;
use starknet_wallet_core::privacy::proof::CollectPolicy;
use starknet_wallet_core::privacy::shadow::{
    address, commitment, compute_additional_data, invoke_additional_data, partial_commitment,
    OpenNoteRef, PRIMER_CLASS_HASH,
};
use starknet_wallet_core::privacy::shadow_reads::{
    get_shadow_accounts_body, judge_shadow_accounts, nonce_windows, scope_commitment,
    ShadowCommitmentRead,
};
use starknet_wallet_core::send::calls::Call;
use starknet_wallet_core::{encode_short_string, felt, Felt, SecretFelt};

use crate::strk20_support::lines;

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/extension-strk20-shadow.json")).unwrap()
}

/// The rows of `key`, each a compact JSON string.
fn rows(key: &str) -> Vec<Value> {
    let rows: Vec<Value> = fixture()[key]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| serde_json::from_str(row.as_str().unwrap()).unwrap())
        .collect();
    assert!(!rows.is_empty(), "{key}");
    rows
}

fn hex(value: &Value) -> Felt {
    felt::parse(value.as_str().unwrap()).unwrap()
}

fn hexes(value: &Value) -> Vec<Felt> {
    value.as_array().unwrap().iter().map(hex).collect()
}

/// `encodeDappName`: a felt passes through (the generator spelled a `bigint` name as its hex), a
/// string is a short string.
fn dapp_name(row: &Value) -> Felt {
    if row["dappName"] == row["dappNameFelt"] {
        hex(&row["dappName"])
    } else {
        encode_short_string(row["dappName"].as_str().unwrap()).unwrap()
    }
}

#[test]
fn primer_class_hash_is_the_sdks() {
    assert_eq!(PRIMER_CLASS_HASH, hex(&fixture()["primerClassHash"]));
}

#[test]
fn shadow_accounts_match_the_sdk() {
    let rows = rows("shadowAccounts");
    let (mut strings, mut numbers) = (0, 0);
    for row in &rows {
        let name = dapp_name(row);
        assert_eq!(name, hex(&row["dappNameFelt"]), "{row}");
        if row["dappName"] == row["dappNameFelt"] {
            numbers += 1;
        } else {
            strings += 1;
        }
        let key = SecretFelt::new(hex(&row["viewingKey"]));
        let anonymizer = hex(&row["anonymizer"]);
        let partial = partial_commitment(&hex(&row["user"]), &key, &anonymizer, &name);
        assert_eq!(partial, hex(&row["partialCommitment"]), "{row}");
        let commitment = commitment(&partial, &hex(&row["nonce"]));
        assert_eq!(commitment, hex(&row["commitment"]), "{row}");
        assert_eq!(
            address(&commitment, &anonymizer),
            Some(hex(&row["address"])),
            "{row}"
        );
    }
    // Two accounts, three names (two short strings, one number), three nonces each.
    assert_eq!((strings, numbers), (12, 6));
}

fn policy(value: &Value) -> CollectPolicy {
    match value["type"].as_str() {
        None | Some("all") => CollectPolicy::All,
        Some("diff") => CollectPolicy::Diff,
        Some("exact") => {
            let amount = value["amount"]
                .as_str()
                .unwrap()
                .strip_prefix("0x")
                .unwrap();
            CollectPolicy::Exact(u128::from_str_radix(amount, 16).unwrap())
        }
        Some(other) => panic!("unknown policy {other}"),
    }
}

#[test]
fn invoke_additional_data_is_what_call_data_compile_writes() {
    let rows = rows("invokeAdditionalData");
    for row in &rows {
        let calls: Vec<Call> = row["calls"]
            .as_array()
            .unwrap()
            .iter()
            .map(|call| Call {
                contract_address: hex(&call["contractAddress"]),
                entrypoint: call["entrypoint"].as_str().unwrap().to_owned(),
                calldata: hexes(&call["calldata"]),
            })
            .collect();
        let notes: Vec<OpenNoteRef> = row["openNotes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|note| OpenNoteRef {
                note_id: hex(&note["noteId"]),
                token: hex(&note["token"]),
            })
            .collect();
        // `null`: no policy given, the SDK's default (all).
        let collect = policy(&row["collectPolicy"]);
        let result = &row["result"];
        assert_eq!(
            invoke_additional_data(&calls, &notes, collect),
            hexes(&result["invokeAdditionalData"]),
            "{row}"
        );
        let name = encode_short_string(row["dappName"].as_str().unwrap()).unwrap();
        assert_eq!(
            compute_additional_data(&name, &hex(&row["nonce"])),
            hexes(&result["computeAdditionalData"]),
            "{row}"
        );
    }
    assert_eq!(rows.len(), 8);
}

fn action(value: &Value) -> ClientAction {
    let input = &value["input"];
    match value["type"].as_str().unwrap() {
        "InvokeExternal" => ClientAction::InvokeExternal {
            contract_address: hex(&input["contract_address"]),
            calldata: hexes(&input["calldata"]),
        },
        "ComputeAndInvoke" => ClientAction::ComputeAndInvoke {
            contract_address: hex(&input["contract_address"]),
            compute_additional_data: hexes(&input["compute_additional_data"]),
            invoke_additional_data: hexes(&input["invoke_additional_data"]),
        },
        "Deposit" => ClientAction::Deposit {
            token: hex(&input["token"]),
            amount: u128::try_from(hex(&input["amount"])).unwrap(),
        },
        "CreateOpenNote" => ClientAction::CreateOpenNote {
            recipient_addr: hex(&input["recipient_addr"]),
            recipient_public_key: hex(&input["recipient_public_key"]),
            token: hex(&input["token"]),
            index: u32::try_from(hex(&input["index"])).unwrap(),
            random: hex(&input["random"]),
        },
        "Withdraw" => ClientAction::Withdraw {
            to_addr: hex(&input["to_addr"]),
            token: hex(&input["token"]),
            amount: u128::try_from(hex(&input["amount"])).unwrap(),
            random: hex(&input["random"]),
        },
        other => panic!("unknown client action {other}"),
    }
}

#[test]
fn invoke_phase_client_actions_encode_as_the_sdk() {
    for row in rows("clientActions") {
        let actions: Vec<ClientAction> = row["actions"]
            .as_array()
            .unwrap()
            .iter()
            .map(action)
            .collect();
        let key = SecretFelt::new(hex(&row["viewingKey"]));
        let calldata = compile_actions_calldata(&hex(&row["user"]), &key, &actions).unwrap();
        let spelled: Vec<String> = calldata
            .iter()
            .map(|felt| felt.expose_secret_hex().to_string())
            .collect();
        let expected: Vec<String> = serde_json::from_value(row["calldata"].clone()).unwrap();
        assert_eq!(spelled, expected, "{row}");
    }
}

#[test]
fn the_extensions_scopes_are_bramble_cores() {
    for row in rows("extensionScopes") {
        let (origin, label) = (
            row["origin"].as_str().unwrap(),
            row["label"].as_str().unwrap(),
        );
        assert_eq!(
            shadow_account_scope(origin, label),
            row["dappName"],
            "{row}"
        );
        assert_eq!(
            shadow_dapp_name(origin, label),
            encode_short_string(row["dappName"].as_str().unwrap()),
            "{row}"
        );
    }
    // The recorded shadow-invoke jobs carry the scope of their origin and label as `dapp_name`.
    for job in lines() {
        let inputs = &job["inputs"];
        for action in inputs["actions"].as_array().into_iter().flatten() {
            if action["type"] == "shadow_account_invoke" {
                let scope = shadow_account_scope(
                    inputs["origin"].as_str().unwrap(),
                    inputs["label"].as_str().unwrap(),
                );
                assert_eq!(action["dapp_name"], scope.as_str(), "{}", job["id"]);
            }
        }
    }
}

/// The read's inputs: the account, key, anonymizer and the scope's dapp name.
struct Read {
    account: Felt,
    key: SecretFelt,
    anonymizer: Felt,
    name: Felt,
}

fn read(job: &Value) -> Read {
    let inputs = &job["inputs"];
    let scope = shadow_account_scope(
        inputs["origin"].as_str().unwrap(),
        inputs["label"].as_str().unwrap(),
    );
    assert_eq!(inputs["dappName"], scope.as_str(), "{}", job["id"]);
    Read {
        account: hex(&inputs["account"]),
        key: SecretFelt::new(hex(&inputs["viewingKey"])),
        anonymizer: hex(&inputs["anonymizer"]),
        name: encode_short_string(&scope).unwrap(),
    }
}

/// `shadowAccountCommitment`: the unfiltered discovery, byte for byte, then the commitment.
fn commitment_read(job: &Value) {
    let id = job["id"].as_str().unwrap();
    let inputs = &job["inputs"];
    let read = read(job);
    let mut walk = Some(ShadowCommitmentRead::new(
        hex(&inputs["pool"]),
        read.account,
        64,
    ));
    for request in job["requests"].as_array().unwrap() {
        assert_eq!(request["route"], "incoming", "{id}");
        assert_eq!(
            request["url"],
            format!(
                "{}/v1/sync/incoming_state",
                inputs["discoveryUrl"].as_str().unwrap()
            ),
            "{id}"
        );
        let current = walk.take().expect("a request after the walk ended");
        assert_eq!(
            std::str::from_utf8(&current.body(&read.key)).unwrap(),
            request["body"].as_str().unwrap(),
            "{id}: body"
        );
        let reply = &request["reply"];
        let status = u16::try_from(reply["status"].as_u64().unwrap()).unwrap();
        match current
            .on_reply(status, reply["body"].as_str().unwrap().as_bytes())
            .unwrap()
        {
            Walked::More(next) => walk = Some(next),
            Walked::Done(()) => {}
        }
    }
    assert!(walk.is_none(), "{id}: the walk completed");
    let nonce = inputs.get("nonce").map(hex);
    let result = scope_commitment(
        &read.account,
        &read.key,
        &read.anonymizer,
        &read.name,
        nonce.as_ref(),
    );
    assert_eq!(result, hex(&job["outcome"]["result"]), "{id}");
}

/// `shadowAccounts`: one `get_shadow_accounts` per window (each from a fresh provider, so the
/// SDK's JSON-RPC id is always 1; ours is substituted), every reply judged.
fn accounts_read(job: &Value) {
    let id = job["id"].as_str().unwrap();
    let inputs = &job["inputs"];
    let read = read(job);
    let partial = scope_commitment(&read.account, &read.key, &read.anonymizer, &read.name, None);
    let windows = nonce_windows(&hexes(&inputs["nonces"]), &SHADOW_SCAN_LIMITS).unwrap();
    let requests = job["requests"].as_array().unwrap();
    assert_eq!(windows.len(), requests.len(), "{id}: one call per window");
    let mut infos = Vec::new();
    for (at, (window, request)) in windows.iter().zip(requests).enumerate() {
        let rpc_id = u64::try_from(at).unwrap() + 7;
        assert_eq!(request["route"], "call", "{id}");
        assert_eq!(request["url"], inputs["rpcUrl"], "{id}");
        let ours: Value = serde_json::from_str(&get_shadow_accounts_body(
            rpc_id,
            &read.anonymizer,
            &partial,
            *window,
        ))
        .unwrap();
        let mut theirs: Value = serde_json::from_str(request["body"].as_str().unwrap()).unwrap();
        theirs["id"] = rpc_id.into();
        assert_eq!(ours, theirs, "{id}: body");
        let mut reply: Value =
            serde_json::from_str(request["reply"]["body"].as_str().unwrap()).unwrap();
        reply["id"] = rpc_id.into();
        infos.extend(judge_shadow_accounts(&reply.to_string(), rpc_id, *window).unwrap());
    }
    let recorded = job["outcome"]["result"].as_array().unwrap();
    assert_eq!(infos.len(), recorded.len(), "{id}");
    for (info, recorded) in infos.iter().zip(recorded) {
        assert_eq!(Felt::from(info.nonce), hex(&recorded["nonce"]), "{id}");
        assert_eq!(info.address, hex(&recorded["address"]), "{id}");
        assert_eq!(Some(info.deployed), recorded["deployed"].as_bool(), "{id}");
        // The canned addresses are the SDK's own derivation.
        let commitment = commitment(&partial, &Felt::from(info.nonce));
        assert_eq!(
            address(&commitment, &read.anonymizer),
            Some(info.address),
            "{id}"
        );
    }
}

#[test]
fn the_recorded_shadow_account_reads_are_the_cores() {
    let (mut commitments, mut accounts) = (0, 0);
    for job in lines() {
        match job["kind"].as_str() {
            Some("shadow-commitment-read") => {
                commitment_read(&job);
                commitments += 1;
            }
            Some("shadow-accounts-read") => {
                accounts_read(&job);
                accounts += 1;
            }
            Some("multisig-review" | "multisig-refusal") => {} // `strk20_multisig_parity`
            Some(other) => panic!("unknown read {other}"),
            None => {}
        }
    }
    // Tracks the fixture: the partial commitment, the commitment at a nonce, and one scan over
    // two windows.
    assert_eq!((commitments, accounts), (2, 1));
}
