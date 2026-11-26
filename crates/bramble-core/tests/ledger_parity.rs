//! P8 golden: `extension-ledger.jsonl`, the extension's Ledger client over the real Starknet app
//! under Speculos (see the line `header`). Every APDU the core sends must be the recorded request,
//! in order; each recorded reply is fed back, and the core's outcome must be the extension's.

mod plan_support;

use bramble_core::ledger::{ledger_account_path, LEDGER_APP_MIN};
use plan_support::node::*;
use plan_support::*;
use serde_json::Value;
use starknet_wallet_core::ledger::{
    app_version_request, check_app_version, parse_app_version, parse_public_key,
    public_key_request, DeviceSigning, DeviceStep, EnumerationStep, KeyEnumeration,
    LedgerRejection,
};
use starknet_wallet_core::send::plan::{Account, Authorization, Kind, Plan};
use starknet_wallet_core::send::plan_device::DeviceKey;
use starknet_wallet_core::send::sign::Deployment;
use starknet_wallet_core::send::types::{SendStep, StepReply};
use starknet_wallet_core::{felt, Felt, NetworkId};

fn all_lines() -> Vec<Value> {
    let path = format!(
        "{}/tests/fixtures/extension-ledger.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect()
}

fn lines(block: &str) -> Vec<Value> {
    let mut lines = all_lines();
    lines.retain(|line| line["block"] == block);
    lines
}

/// Every line belongs to a block a test below replays, and the extension gated at Bramble's
/// minimum: a block added in Bramble fails here rather than going unread.
#[test]
fn every_line_is_read_and_gated_at_bramble_s_minimum() {
    let read = ["header", "version", "publicKey", "sign", "enumerate"];
    for line in all_lines() {
        let block = line["block"].as_str().unwrap();
        assert!(read.contains(&block), "unread block {block}");
    }
    let v = LEDGER_APP_MIN;
    assert_eq!(
        lines("header")[0]["appMinimum"].as_str().unwrap(),
        format!("{}.{}.{}", v.major, v.minor, v.patch)
    );
}

fn line(block: &str, name: &str) -> Value {
    lines(block)
        .into_iter()
        .find(|line| line["name"] == name)
        .unwrap()
}

fn bytes(text: &Value) -> Vec<u8> {
    let text = text.as_str().unwrap();
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

fn path(line: &Value) -> starknet_wallet_core::ledger::LedgerPath {
    let index = line["accountIndex"].as_u64().unwrap();
    let path = ledger_account_path(line["scheme"].as_str().unwrap(), index).unwrap();
    assert_eq!(path.to_string(), line["path"].as_str().unwrap());
    path
}

/// The recorded exchanges, as a device answering the core: each APDU must be the next request.
struct Replay {
    exchanges: Vec<Value>,
    next: usize,
    name: String,
}

impl Replay {
    fn of(line: &Value) -> Self {
        Self {
            exchanges: line["exchanges"].as_array().unwrap().clone(),
            next: 0,
            name: line["name"].as_str().unwrap().into(),
        }
    }

    fn answer(&mut self, apdu: &[u8]) -> Vec<u8> {
        let exchange = self
            .exchanges
            .get(self.next)
            .unwrap_or_else(|| panic!("{}: core sent an extra APDU", self.name));
        assert_eq!(
            apdu,
            bytes(&exchange["request"]),
            "{}: APDU {}",
            self.name,
            self.next
        );
        self.next += 1;
        bytes(&exchange["reply"])
    }

    fn done(&self) {
        assert_eq!(
            self.next,
            self.exchanges.len(),
            "{}: core stopped early",
            self.name
        );
    }
}

/// The extension's refusal: its code and, where it carries one, its status word.
fn assert_refused(rejection: &LedgerRejection, outcome: &Value, name: &str) {
    assert_eq!(outcome["status"], "refused", "{name}: {rejection:?}");
    assert_eq!(rejection.code.as_str(), outcome["code"], "{name}");
    let word = outcome["statusWord"]
        .as_str()
        .map(|w| u16::from_str_radix(w.trim_start_matches("0x"), 16).unwrap());
    assert_eq!(rejection.status_word, word, "{name}");
}

fn felt_of(value: &Value) -> Felt {
    f(value.as_str().unwrap())
}

#[test]
fn the_app_version_reads_and_gates_as_the_extension() {
    let versions = lines("version");
    assert_eq!(versions.len(), 2);
    for line in versions {
        let name = line["name"].as_str().unwrap();
        let mut replay = Replay::of(&line);
        let reply = replay.answer(&app_version_request());
        replay.done();
        let outcome = &line["outcome"];
        match parse_app_version(Ok(&reply))
            .and_then(|v| check_app_version(v, LEDGER_APP_MIN).map(|()| v))
        {
            Ok(v) => {
                assert_eq!(outcome["status"], "ok", "{name}");
                let text = format!("{}.{}.{}", v.major, v.minor, v.patch);
                assert_eq!(text, outcome["version"].as_str().unwrap(), "{name}");
            }
            Err(rejection) => assert_refused(&rejection, outcome, name),
        }
    }
}

#[test]
fn public_keys_read_at_both_schemes_as_the_extension() {
    let keys = lines("publicKey");
    assert_eq!(keys.len(), 4);
    for line in keys {
        let mut replay = Replay::of(&line);
        let reply = replay.answer(&public_key_request(&path(&line)));
        replay.done();
        let key = parse_public_key(Ok(&reply)).unwrap();
        assert_eq!(
            felt::canonical(&key),
            line["outcome"]["publicKeyX"].as_str().unwrap()
        );
    }
}

/// Runs `DeviceSigning` for a `sign` line over its transcript.
fn sign(line: &Value) -> DeviceStep {
    let mut replay = Replay::of(line);
    let (mut signing, mut step) = DeviceSigning::start(
        path(line),
        felt_of(&line["hash"]),
        felt_of(&line["storedPublicKeyX"]),
        LEDGER_APP_MIN,
    );
    while let DeviceStep::Send(apdu) = &step {
        let reply = replay.answer(apdu);
        step = signing.on_reply(Ok(&reply));
    }
    replay.done();
    step
}

#[test]
fn every_signature_ends_as_the_extensions() {
    let signs = lines("sign");
    assert!(signs.len() >= 4);
    for line in signs {
        let name = line["name"].as_str().unwrap();
        let outcome = &line["outcome"];
        match sign(&line) {
            DeviceStep::Signed(signature) => {
                assert_eq!(outcome["status"], "ok", "{name}");
                assert_eq!(outcome["verified"], true, "{name}");
                assert_eq!(signature.r(), felt_of(&outcome["r"]), "{name}");
                assert_eq!(signature.s(), felt_of(&outcome["s"]), "{name}");
            }
            DeviceStep::Refused(rejection) => assert_refused(&rejection, outcome, name),
            DeviceStep::Send(_) => unreachable!(),
        }
    }
}

#[test]
fn a_page_of_keys_reads_as_the_extensions() {
    let pages = lines("enumerate");
    assert_eq!(pages.len(), 2);
    for line in pages {
        let scheme = line["scheme"].as_str().unwrap();
        let start = line["startIndex"].as_u64().unwrap() as u32;
        let count = line["count"].as_u64().unwrap() as u32;
        let page = (start..start + count)
            .map(|i| (i, ledger_account_path(scheme, i.into()).unwrap()))
            .collect();
        let mut replay = Replay::of(&line);
        let (mut enumeration, mut step) = KeyEnumeration::start(page, LEDGER_APP_MIN);
        while let EnumerationStep::Send(apdu) = &step {
            let reply = replay.answer(apdu);
            step = enumeration.on_reply(Ok(&reply));
        }
        replay.done();
        let EnumerationStep::Keys(keys) = step else {
            panic!("{scheme}: {step:?}");
        };
        let expected = line["outcome"]["entries"].as_array().unwrap();
        assert_eq!(keys.len(), expected.len());
        for (key, entry) in keys.iter().zip(expected) {
            assert_eq!(
                u64::from(key.index),
                entry["accountIndex"].as_u64().unwrap()
            );
            assert_eq!(key.public_key_x, felt_of(&entry["publicKeyX"]));
        }
    }
}

#[cfg(feature = "mobile")]
#[test]
fn the_picker_places_each_key_where_the_extension_does() {
    use bramble_core::ledger::LedgerScheme;
    use bramble_core::{ledger_account_candidates, AccountFamily, LedgerKeyReader, LedgerReadStep};
    let pages = lines("enumerate");
    assert_eq!(pages.len(), 2);
    for line in pages {
        let (scheme, family) = match line["scheme"].as_str().unwrap() {
            "LedgerW" => (LedgerScheme::Bramble, AccountFamily::OpenZeppelin),
            "argentx" => (LedgerScheme::Ready, AccountFamily::Argent),
            other => panic!("unknown scheme {other}"),
        };
        assert_eq!(
            line["family"],
            if family == AccountFamily::Argent {
                "argent"
            } else {
                "openzeppelin"
            }
        );
        assert_eq!(line["networkId"], "SN_SEPOLIA");
        let reader = LedgerKeyReader::new(
            scheme,
            line["startIndex"].as_u64().unwrap() as u32,
            line["count"].as_u64().unwrap() as u32,
        )
        .unwrap();
        let mut replay = Replay::of(&line);
        let mut step = reader.start();
        while let LedgerReadStep::Send { apdu } = &step {
            step = reader.reply(replay.answer(apdu));
        }
        replay.done();
        let LedgerReadStep::Keys { keys } = step else {
            panic!("{step:?}");
        };
        let candidates = ledger_account_candidates(NetworkId::Sepolia, family, keys).unwrap();
        let expected = line["candidates"].as_array().unwrap();
        assert_eq!(candidates.len(), expected.len());
        for (candidate, entry) in candidates.iter().zip(expected) {
            assert_eq!(f(&candidate.address), felt_of(&entry["address"]));
            assert_eq!(
                f(&candidate.request.public_key_x),
                felt_of(&entry["publicKeyX"])
            );
            let index = u64::from(candidate.request.account_index);
            assert_eq!(index, entry["accountIndex"].as_u64().unwrap());
        }
    }
}

/// LedgerW account 0 as an OpenZeppelin account on Sepolia, the key the device reported.
fn ledger_account() -> Account {
    let public_key = felt_of(&line("publicKey", "LedgerW 0")["outcome"]["publicKeyX"]);
    let deployment = Deployment {
        class_hash: f(OZ_CLASS),
        salt: Felt::ZERO,
        constructor_calldata: vec![public_key],
    };
    let address = deployment.address().unwrap();
    // The extension's picker placed the same address.
    let page = line("enumerate", "LedgerW page 0");
    assert_eq!(address, felt_of(&page["candidates"][0]["address"]));
    Account {
        network: NetworkId::Sepolia,
        address,
        public_key,
        authorization: Authorization::StarkPair,
        deployment: Some(deployment),
    }
}

/// A plan for LedgerW account 0, signing on the device: a transfer (1.5 STRK to `ACCOUNT_1`)
/// from the deployed account, or the undeployed account's standalone deployment, against
/// wallet-core's canned Sepolia node.
fn ledger_plan(host: &Host, node: &mut Node, deploy: bool) -> Plan {
    let account = ledger_account();
    let kind = if deploy {
        Kind::Deployment
    } else {
        Kind::Transfer(Box::new(transfer(&account, ACCOUNT_1, &[]).unwrap()))
    };
    let device = DeviceKey {
        path: ledger_account_path("LedgerW", 0).unwrap(),
        app_minimum: LEDGER_APP_MIN,
    };
    let mut plan = Plan::new(account, kind, bramble_core::SEND_POLICY).with_device(device);
    let first = plan.advance(host, StepReply::Start).unwrap();
    assert_eq!(
        run_until(&mut plan, host, node, first, never),
        SendStep::Review
    );
    plan
}

/// Confirms the plan and answers its device from the recorded `sign` line `name`; the write
/// carries the device's signature and the plan broadcasts the hash the device signed.
fn signs_on_the_recorded_device(deploy: bool, name: &str) {
    let host = Host::new();
    let mut node = Node::sepolia(!deploy);
    let mut plan = ledger_plan(&host, &mut node, deploy);
    let line = line("sign", name);
    let mut replay = Replay::of(&line);
    let mut step = plan.advance(&host, StepReply::Confirm).unwrap();
    let write = loop {
        step = match run_until(&mut plan, &host, &mut node, step, |_, w| w) {
            SendStep::Device { apdu } => {
                let reply = replay.answer(&apdu);
                plan.advance(&host, StepReply::Device { reply }).unwrap()
            }
            write @ SendStep::Rpc { .. } => break write,
            other => panic!("{name}: {other:?}"),
        };
    };
    replay.done();
    assert!(
        host.signed.borrow().is_empty(),
        "the host's key never signs"
    );
    assert_eq!(latest_hash(&host), felt::canonical(&felt_of(&line["hash"])));
    let (_, _, params, _) = parts(&write);
    let tx = params
        .get("invoke_transaction")
        .or_else(|| params.get("deploy_account_transaction"))
        .unwrap();
    let outcome = &line["outcome"];
    assert_eq!(outcome["verified"], true);
    let signature: Vec<Felt> = tx["signature"]
        .as_array()
        .unwrap()
        .iter()
        .map(felt_of)
        .collect();
    assert_eq!(signature, [felt_of(&outcome["r"]), felt_of(&outcome["s"])]);
    let SendStep::Broadcast { hash } = run_until(&mut plan, &host, &mut node, write, never) else {
        panic!("{name}: no broadcast");
    };
    assert_eq!(hash, latest_hash(&host));
}

#[test]
fn a_ledger_transfer_signs_on_the_recorded_device() {
    signs_on_the_recorded_device(false, "plan transfer");
}

#[test]
fn a_ledger_deployment_signs_on_the_recorded_device() {
    signs_on_the_recorded_device(true, "plan deployment");
}

/// Not a test of the fixture: prints the hashes the plans above sign, the generator's
/// `PLAN_*_HASH` inputs.
#[test]
#[ignore]
fn print_plan_hashes() {
    for deploy in [true, false] {
        let host = Host::new();
        let mut node = Node::sepolia(!deploy);
        let mut plan = ledger_plan(&host, &mut node, deploy);
        let mut step = plan.advance(&host, StepReply::Confirm).unwrap();
        let key = line("publicKey", "LedgerW 0")["exchanges"][0]["reply"].clone();
        loop {
            let SendStep::Device { apdu } = run_until(&mut plan, &host, &mut node, step, never)
            else {
                panic!()
            };
            let reply = match (apdu[1], apdu[2]) {
                (0x00, _) => vec![2, 4, 3, 0x90, 0],
                (0x01, _) => bytes(&key),
                (0x02, 0) => vec![0x90, 0],
                _ => {
                    println!(
                        "deploy={deploy} {}",
                        felt::canonical(&Felt::from_bytes_be_slice(&apdu[5..37]))
                    );
                    break;
                }
            };
            step = plan.advance(&host, StepReply::Device { reply }).unwrap();
        }
    }
}
