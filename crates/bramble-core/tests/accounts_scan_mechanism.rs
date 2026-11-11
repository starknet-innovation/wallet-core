//! The discovery state machine end to end against mobile's scripted node (its `tests/recovery.rs`
//! and `tests/recovery_notes.rs` scan cases, without Bramble's row wording): batching, the keep
//! rule, pause and retry, exact ids, manual indices, a reset for another network or phrase, and
//! the advisory verdicts a deployed candidate carries. The plan's canned node (`plan_support`)
//! answers one account; this one answers the forty addresses a batch probes.

mod accounts_support;
mod plan_support;

use std::collections::{HashMap, HashSet};

use accounts_support::{scan_config, PHRASE};
use plan_support::{f, ETH, STRK};
use serde_json::{json, Value};
use starknet_wallet_core::accounts::advisory::{own_viewing_public_key, Restriction};
use starknet_wallet_core::accounts::scan::{Found, Scan, ScanConfig, ScanStep};
use starknet_wallet_core::accounts::{AccountFamily, Ladder};
use starknet_wallet_core::rpc::{Request, Response};
use starknet_wallet_core::send::plan_probe::{
    GET_DEFERRED_REMOVE_SIGNERS, GET_GUARDIAN, GET_OWNER, GET_OWNER_TYPE, GET_PUBLIC_KEY,
    GET_SIGNERS,
};
use starknet_wallet_core::{account_public_key, felt, Error, NetworkId};

const OZ_CLASS: &str = "0x1d1777db36cdd06dd62cfde77b1b6ae06412af95d57a13dc40ac77b8a702381";
const ARGENT_0_3_1: &str = "0x29927c8af6bccf3f6fda035981e765a7bdbf18a2dc0d630494f8758aa908e2b";
const ARGENT_0_4_0: &str = "0x36078334509b514626504edc9fb252328d1a240e4e948bef8d0c08dff45927f";
const BRAAVOS_1_0_0: &str = "0x3d16c7a9a60b0593bd202f660a28c5d76e0403601d9ccc7e4fa253b6a70c201";
const BRAAVOS_BASE: &str = "0x13bfe114fb1cf405bfc3a7f8dbe2d91db146c17521d40dcf57e16d6b59fa8e6";
const BRAAVOS_CURRENT: &str = "0x3957f9f5a1cbfe918cedc2015c85200ca51a5f7506ecb6de98a5207b759bf8a";
/// A pool for the advisory test; which address is Bramble's routing, not the scan's.
const POOL: &str = "0x5c0be7f9d2e7b8d1c2a3f4e5d6c7b8a9f0e1d2c3b4a5968778695a4b3c2d1e0";

fn sepolia() -> ScanConfig {
    scan_config(NetworkId::Sepolia, vec![f(STRK), f(ETH)], None)
}

/// A node that knows which addresses are deployed and which hold a token balance.
#[derive(Default)]
struct Node {
    deployed: HashSet<String>,
    funded: HashSet<String>,
    /// Answer class-hash probes with this raw error instead (e.g. a rate limit).
    failure: Option<Value>,
    /// A deployed address's live class, when not OpenZeppelin's.
    classes: HashMap<String, String>,
    /// `starknet_call` results keyed `contract:selector:first-calldata`, for the advisory reads;
    /// any other call is a balance read.
    views: HashMap<String, Vec<String>>,
    block_time: u64,
}

fn view_key(contract: &str, selector: &str, argument: Option<&str>) -> String {
    format!("{contract}:{selector}:{}", argument.unwrap_or(""))
}

impl Node {
    fn view(&mut self, contract: &str, selector: &str, argument: Option<&str>, result: &[&str]) {
        let result = result.iter().map(|felt| felt.to_string()).collect();
        self.views
            .insert(view_key(contract, selector, argument), result);
    }

    fn answer(&self, request: &Request) -> Response {
        let body: Value = serde_json::from_str(&request.body).unwrap();
        let id = body["id"].clone();
        let call = &body["params"]["request"];
        let key = view_key(
            call["contract_address"].as_str().unwrap_or(""),
            call["entry_point_selector"].as_str().unwrap_or(""),
            call["calldata"][0].as_str(),
        );
        let reply = match body["method"].as_str().unwrap() {
            "starknet_getClassHashAt" => {
                let address = body["params"]["contract_address"].as_str().unwrap();
                if let Some(error) = &self.failure {
                    json!({ "jsonrpc": "2.0", "id": id, "error": error })
                } else if self.deployed.contains(address) {
                    let class = self.classes.get(address).map_or(OZ_CLASS, String::as_str);
                    json!({ "jsonrpc": "2.0", "id": id, "result": class })
                } else {
                    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": 20, "message": "Contract not found" } })
                }
            }
            "starknet_call" if self.views.contains_key(&key) => {
                json!({ "jsonrpc": "2.0", "id": id, "result": self.views[&key] })
            }
            "starknet_getBlockWithTxHashes" => {
                json!({ "jsonrpc": "2.0", "id": id, "result": { "timestamp": self.block_time, "transactions": [] } })
            }
            "starknet_call" => {
                let account = call["calldata"][0].as_str().unwrap();
                let low = if self.funded.contains(account) {
                    "0x5"
                } else {
                    "0x0"
                };
                json!({ "jsonrpc": "2.0", "id": id, "result": [low, "0x0"] })
            }
            other => panic!("unexpected method {other}"),
        };
        Response {
            id: request.id,
            body: reply.to_string(),
        }
    }

    fn answer_all(&self, step: &ScanStep) -> Vec<Response> {
        step.requests
            .iter()
            .map(|request| self.answer(request))
            .collect()
    }

    /// Runs one batch to completion (class-hash round, then balances and reads if any).
    fn run_batch(&self, scan: &mut Scan, config: &ScanConfig, manual: Option<u32>) -> ScanStep {
        let mut step = scan.next(PHRASE, None, config, manual).unwrap();
        while !step.requests.is_empty() {
            step = scan
                .submit(PHRASE, None, config, &self.answer_all(&step))
                .unwrap();
        }
        step
    }

    /// Runs the whole default scan.
    fn run_scan(&self, scan: &mut Scan, config: &ScanConfig) -> ScanStep {
        loop {
            let step = self.run_batch(scan, config, None);
            if step.complete {
                return step;
            }
        }
    }
}

/// The address the first batch probes at `position` (ten shapes per index).
fn address_of(config: &ScanConfig, position: usize) -> String {
    let first = Scan::new().next(PHRASE, None, config, None).unwrap();
    let body: Value = serde_json::from_str(&first.requests[position].body).unwrap();
    body["params"]["contract_address"]
        .as_str()
        .unwrap()
        .to_string()
}

fn row<'a>(step: &'a ScanStep, address: &str) -> &'a Found {
    step.candidates
        .iter()
        .find(|found| felt::canonical(&found.address) == address)
        .expect("the row is kept")
}

#[test]
fn a_quiet_wallet_offers_only_the_creatable_index_zero_accounts() {
    let config = sepolia();
    let mut scan = Scan::new();
    let first = scan.next(PHRASE, None, &config, None).unwrap();
    assert_eq!(
        first.requests.len(),
        4 * 10,
        "four indices, ten shapes each"
    );
    assert_eq!(
        (first.checked, first.target, first.complete),
        (0, 20, false)
    );
    let step = Node::default().run_scan(&mut scan, &config);
    assert!(step.complete);
    assert_eq!(step.checked, 20);
    let rows: Vec<_> = step
        .candidates
        .iter()
        .map(|c| {
            (
                c.family,
                c.ladder,
                felt::canonical(&c.class_hash),
                c.account_index,
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            (
                AccountFamily::OpenZeppelin,
                Ladder::Account,
                OZ_CLASS.into(),
                0
            ),
            (
                AccountFamily::Argent,
                Ladder::Account,
                ARGENT_0_4_0.into(),
                0
            ),
            (
                AccountFamily::Braavos,
                Ladder::Address,
                BRAAVOS_1_0_0.into(),
                0
            ),
        ],
        "OZ, Argent 0.4.0 and Braavos 1.0.0 at index 0 only (issue 718)"
    );
    assert!(step.candidates.iter().all(|c| !c.deployed && !c.funded));
    // A finished scan answers its last step again (the extension restarts from index 0).
    let again = scan.next(PHRASE, None, &config, None).unwrap();
    assert_eq!(again, step);
}

#[test]
fn activity_is_found_across_families() {
    let config = sepolia();
    // Each index: OZ, then argent 0.3.1, 0.3.0, 0.4.0 and Xverse's 0.4.0 on this wallet's ladder,
    // then argent 0.3.1, 0.3.0, 0.4.0 on Ready's, then braavos 1.0.0 and base on Braavos's
    // (issues 505, 734).
    let (ready, braavos) = (address_of(&config, 10 + 5), address_of(&config, 10 * 3 + 9));
    let mut node = Node::default();
    node.deployed.insert(ready.clone()); // Ready argent 0.3.1 at index 1
    node.funded.insert(braavos.clone()); // braavos base at index 3
    let step = node.run_scan(&mut Scan::new(), &config);
    let argent = row(&step, &ready);
    assert_eq!(
        (argent.family, argent.ladder, argent.account_index),
        (AccountFamily::Argent, Ladder::ArgentX, 1)
    );
    assert_eq!(argent.class_hash, f(ARGENT_0_3_1));
    assert!(argent.deployed && !argent.funded);
    // The scripted node answers every deployed account with OpenZeppelin's class, which no
    // Argent authorization this wallet produces accepts.
    assert_eq!(argent.restriction, Some(Restriction::Release));
    let braavos = row(&step, &braavos);
    assert_eq!(
        (braavos.family, braavos.account_index),
        (AccountFamily::Braavos, 3)
    );
    assert_eq!(braavos.class_hash, f(BRAAVOS_BASE));
    assert!(braavos.funded && !braavos.deployed);
    // Activity at index 3 moves the target to 24.
    assert_eq!((step.checked, step.target), (24, 24));
}

#[test]
fn an_unexpected_error_pauses_and_the_batch_is_retried() {
    let config = sepolia();
    let mut scan = Scan::new();
    let failing = Node {
        failure: Some(json!({ "code": 24, "message": "Block not found" })),
        ..Node::default()
    };
    let step = scan.next(PHRASE, None, &config, None).unwrap();
    let error = scan
        .submit(PHRASE, None, &config, &failing.answer_all(&step))
        .unwrap_err();
    assert!(matches!(error, Error::Rpc { ref reason } if reason == "account search paused"));
    // Nothing advanced: the retry re-issues indices 0-3 with fresh ids.
    let retry = scan.next(PHRASE, None, &config, None).unwrap();
    assert_eq!(retry.checked, 0);
    assert_eq!(retry.requests.len(), 40);
    assert!(retry.requests[0].id > step.requests.last().unwrap().id);
    let done = Node::default().run_batch(&mut scan, &config, None);
    assert_eq!(done.checked, 4);
}

#[test]
fn responses_must_answer_exactly_the_outstanding_ids() {
    let config = sepolia();
    let mut scan = Scan::new();
    let node = Node::default();
    let mut submit = |change: &dyn Fn(&mut Vec<Response>)| {
        let step = scan.next(PHRASE, None, &config, None).unwrap();
        let mut answers = node.answer_all(&step);
        change(&mut answers);
        scan.submit(PHRASE, None, &config, &answers)
    };
    assert!(
        submit(&|answers| drop(answers.pop())).is_err(),
        "a missing answer"
    );
    assert!(
        submit(&|answers| answers[1] = answers[0].clone()).is_err(),
        "a duplicate id"
    );
    assert!(
        submit(&|answers| answers[0].id = 999_999).is_err(),
        "a foreign id"
    );
    let swap = |answers: &mut Vec<Response>| {
        let (a, b) = (answers[0].id, answers[1].id);
        answers[0].id = b;
        answers[1].id = a;
    };
    assert!(submit(&swap).is_err(), "a body under the wrong id");
    assert!(submit(&|_| ()).is_ok(), "every id once");
}

#[test]
fn a_manual_index_keeps_the_scan_position_and_lists_creatable_accounts() {
    let config = sepolia();
    let mut scan = Scan::new();
    let node = Node::default();
    node.run_batch(&mut scan, &config, None);
    let manual = node.run_batch(&mut scan, &config, Some(57));
    assert_eq!(manual.checked, 4, "the default scan does not move");
    assert!(manual
        .candidates
        .iter()
        .any(|c| c.account_index == 57 && c.family == AccountFamily::OpenZeppelin));
    assert!(matches!(
        scan.next(PHRASE, None, &config, Some(0x8000_0000)),
        Err(Error::InvalidInput { .. })
    ));
}

#[test]
fn the_scan_resets_for_another_network_or_phrase() {
    let config = sepolia();
    let mut scan = Scan::new();
    Node::default().run_batch(&mut scan, &config, None);
    let mainnet = scan_config(NetworkId::Mainnet, vec![f(STRK)], None);
    assert_eq!(
        scan.next(PHRASE, None, &mainnet, None).unwrap().checked,
        0,
        "a different network starts over"
    );
    Node::default().run_batch(&mut scan, &mainnet, None);
    let mut passphrase = mainnet.clone();
    passphrase
        .shapes
        .retain(|shape| shape.ladder != Ladder::ArgentX);
    let other = scan.next(PHRASE, Some("p"), &passphrase, None).unwrap();
    assert_eq!(other.checked, 0, "another passphrase is another wallet");
    // A dropped scan (the host's lock) has nothing in progress.
    assert!(Scan::new().submit(PHRASE, None, &config, &[]).is_err());
}

#[test]
fn deployed_candidates_carry_the_extensions_advisory_verdicts() {
    let network = NetworkId::Mainnet;
    let config = scan_config(network, vec![f(STRK)], Some(f(POOL)));
    // Index 0: OZ (0), …, Argent 0.4.0 on Ready's ladder (7), Braavos 1.0.0 (8); OZ at index 1.
    let at = |position| address_of(&config, position);
    let (oz, ready, braavos, oz_one) = (at(0), at(7), at(8), at(10));
    let own_view =
        own_viewing_public_key(PHRASE, None, Ladder::Account, 0, network, &f(POOL)).unwrap();
    let key = |ladder| felt::canonical(&account_public_key(PHRASE, None, ladder, 0).unwrap());
    let (ready_key, braavos_key) = (key(Ladder::ArgentX), key(Ladder::Address));
    let mut node = Node {
        block_time: 100,
        ..Node::default()
    };
    for address in [&oz, &ready, &braavos, &oz_one] {
        node.deployed.insert(address.clone());
    }
    node.classes.insert(ready.clone(), ARGENT_0_4_0.into());
    node.classes.insert(braavos.clone(), BRAAVOS_CURRENT.into());
    // Ready: this wallet's owner, a guardian, and another wallet's key in the pool.
    node.view(&ready, GET_OWNER, None, &[&ready_key]);
    node.view(&ready, GET_OWNER_TYPE, None, &["0x0"]);
    node.view(&ready, GET_GUARDIAN, None, &["0x9"]);
    node.view(POOL, GET_PUBLIC_KEY, Some(&ready), &["0x123"]);
    // Braavos: a 2FA signer whose removal expires at the block's own timestamp, so not yet.
    node.view(
        &braavos,
        GET_SIGNERS,
        None,
        &["0x1", &braavos_key, "0x1", "0x9", "0x0"],
    );
    node.view(&braavos, GET_DEFERRED_REMOVE_SIGNERS, None, &["0x64"]);
    // OZ: this wallet's own registration, then another wallet's.
    node.view(
        POOL,
        GET_PUBLIC_KEY,
        Some(&oz),
        &[&felt::canonical(&own_view)],
    );
    node.view(POOL, GET_PUBLIC_KEY, Some(&oz_one), &["0x456"]);

    let step = node.run_scan(&mut Scan::new(), &config);
    let ready = row(&step, &ready);
    assert_eq!(ready.restriction, Some(Restriction::Guardian));
    assert!(ready.privacy_elsewhere);
    let braavos = row(&step, &braavos);
    assert_eq!(braavos.restriction, Some(Restriction::StrongSigner));
    assert!(
        !braavos.privacy_elsewhere,
        "an unreadable pool key is no note"
    );
    let oz = row(&step, &oz);
    assert_eq!((oz.restriction, oz.privacy_elsewhere), (None, false));
    assert!(row(&step, &oz_one).privacy_elsewhere);
}

#[test]
fn a_network_without_a_pool_reads_no_pool_key() {
    let config = sepolia();
    let oz = address_of(&config, 0);
    let mut node = Node::default();
    node.deployed.insert(oz);
    let mut scan = Scan::new();
    let step = scan.next(PHRASE, None, &config, None).unwrap();
    let detail = scan
        .submit(PHRASE, None, &config, &node.answer_all(&step))
        .unwrap();
    assert!(detail
        .requests
        .iter()
        .all(|request| !request.body.contains(GET_PUBLIC_KEY)));
    assert!(
        step.requests.iter().all(|request| !request.optional),
        "a probe is never optional"
    );
}
