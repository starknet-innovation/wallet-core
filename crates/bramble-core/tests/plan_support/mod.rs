//! Shared by the plan parity tests: the captures, Bramble's catalog values, a test `PlanHost` on
//! the published E2E phrase, and the driver that holds the plan to the recorded wire order.
#![allow(dead_code)]

pub mod node;

use std::cell::RefCell;

use bramble_core::catalogs::accounts::{
    catalog, send_authorization, AccountFamily, ARGENT_SIGNER_ARRAY,
};
use bramble_core::SEND_POLICY;
use serde_json::{json, Value};
use starknet_wallet_core::send::calls::ReviewFlag;
use starknet_wallet_core::send::judge::{Observation, TxStatus};
use starknet_wallet_core::send::plan::{
    Account, Kind, PendingKind, PendingRecord, Plan, PlanHost, SignRefusal, Token, Transfer,
    TransferRefusal,
};
use starknet_wallet_core::send::sign::Deployment;
use starknet_wallet_core::send::types::{SendStep, StepReply};
use starknet_wallet_core::{
    derive_starknet_keypair, felt, sign_stark_hash, Felt, NetworkId, SecretFelt, StarkSignature,
};

pub const NOW: u64 = 1_790_000_000_000;
pub const STRK: &str = "0x4718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d";
pub const ACCOUNT_1: &str = "0x3fa78c2861131190760b151f3ee110f473940eca3fec8cc423ecc307b5241a3";
pub const ETH: &str = "0x49d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7";

// The OpenZeppelin class Bramble deploys (Krusty's at `OZ_ACCOUNT_VERSION`), as the node answers
// it; a wrong value fails the captures below. The other classes come from `bramble_core::catalogs`.
pub const OZ_CLASS: &str = "0x1d1777db36cdd06dd62cfde77b1b6ae06412af95d57a13dc40ac77b8a702381";

pub fn capture(name: &str) -> Value {
    let path = format!(
        "{}/tests/fixtures/extension-transactions-{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

pub fn phase(scenario: &Value, name: &str) -> Vec<Value> {
    scenario[name]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| serde_json::from_str(line.as_str().unwrap()).unwrap())
        .collect()
}

pub fn f(text: &str) -> Felt {
    felt::parse(&felt::canonical(&Felt::from_hex(text).unwrap())).unwrap()
}

pub fn network(scenario: &Value) -> NetworkId {
    match scenario["networkId"].as_str().unwrap() {
        "SN_MAIN" => NetworkId::Mainnet,
        _ => NetworkId::Sepolia,
    }
}

/// The wallet behind a plan: the published E2E phrase's account 0 key, and an activity log.
pub struct Host {
    key: SecretFelt,
    pub records: RefCell<Vec<PendingRecord>>,
    pub observed: RefCell<Vec<Observation>>,
    /// Every hash the plan asked to have signed, in order.
    pub signed: RefCell<Vec<Felt>>,
    /// What `session_current` answers: false once the wallet locked after the plan was made.
    pub session: std::cell::Cell<bool>,
}

impl Host {
    pub fn new() -> Self {
        let phrase = capture("wire")["mnemonic"].as_str().unwrap().to_string();
        let pair = derive_starknet_keypair(&phrase, 0, 0, None).unwrap();
        Self {
            key: pair.private_key,
            records: RefCell::default(),
            observed: RefCell::default(),
            signed: RefCell::default(),
            session: std::cell::Cell::new(true),
        }
    }

    pub fn public_key(&self) -> Felt {
        f(&sign_stark_hash(&self.key, "0x1").unwrap().public_key)
    }
}

impl PlanHost for Host {
    fn session_current(&self) -> bool {
        self.session.get()
    }
    fn sign(&self, hash: &Felt, _renew: bool) -> Result<StarkSignature, SignRefusal> {
        self.signed.borrow_mut().push(*hash);
        sign_stark_hash(&self.key, &felt::canonical(hash)).map_err(|_| SignRefusal::Failed)
    }
    fn record_pending(&self, record: PendingRecord) {
        self.records.borrow_mut().push(record);
    }
    fn observe(&self, _hash: &Felt, observation: Observation) -> Option<TxStatus> {
        self.observed.borrow_mut().push(observation);
        None
    }
    fn explorer_url(&self, hash: &str) -> Option<String> {
        Some(format!("https://explorer.test/tx/{hash}"))
    }
}

/// The account the extension signed for: its stored address, and the deployment the wallet
/// offers only when it reproduces that address (`accountDeployment`).
pub fn account(scenario: &Value, family: &str, public_key: Felt) -> Account {
    let exchanges: Vec<Value> = ["reviewInvoke", "reviewDeployment", "submit"]
        .iter()
        .flat_map(|name| phase(scenario, name))
        .collect();
    let address = scenario["account"]
        .get("address")
        .map(|a| f(a.as_str().unwrap()))
        .or_else(|| {
            exchanges.iter().find_map(|e| {
                let params = &e["params"];
                params
                    .get("contract_address")
                    .or_else(|| params.get("request")?.get("contract_address"))
                    .map(|a| f(a.as_str().unwrap()))
            })
        })
        .unwrap();
    let record_class = scenario["recordClass"].as_str().unwrap_or("");
    // Mobile deploys only the pinned Argent 0.4.0 class or Xverse's build of it.
    let argent_class = catalog()
        .classes
        .iter()
        .find(|entry| {
            entry.info.label == record_class
                && entry.info.authorization_shape == ARGENT_SIGNER_ARRAY
        })
        .map(|entry| entry.class_hash);
    let (family, candidate) = match family {
        "openzeppelin" => (
            AccountFamily::OpenZeppelin,
            Some((f(OZ_CLASS), Felt::ZERO, vec![public_key])),
        ),
        "argent" => (
            AccountFamily::Argent,
            // Krusty's Argent constructor: a Starknet signer and no guardian.
            argent_class.map(|class| (class, public_key, vec![Felt::ZERO, public_key, Felt::ONE])),
        ),
        _ => (
            AccountFamily::Braavos,
            Some((f(record_class), public_key, vec![public_key])),
        ),
    };
    let authorization = send_authorization(family);
    let deployment = candidate
        .map(|(class_hash, salt, constructor_calldata)| Deployment {
            class_hash,
            salt,
            constructor_calldata,
        })
        .filter(|d| d.address() == Some(address))
        .filter(|d| {
            family != AccountFamily::Braavos || d.class_hash == catalog().braavos_deployment
        });
    Account {
        network: network(scenario),
        address,
        public_key,
        authorization,
        deployment,
    }
}

/// Bramble's `prepare_transfer` for the scenario: 1.5 STRK to the phrase's account 1 (the
/// refusal captures send there without naming it).
pub fn plan(scenario: &Value, account: Account) -> Plan {
    if scenario["flow"] == "deployment" {
        return Plan::new(account, Kind::Deployment, SEND_POLICY);
    }
    let recipient = scenario["recipient"].as_str().unwrap_or(ACCOUNT_1);
    let transfer = transfer(&account, recipient, &[]).unwrap();
    assert!(!transfer.flags().contains(&ReviewFlag::AddressPoisoning));
    Plan::new(account, Kind::Transfer(Box::new(transfer)), SEND_POLICY)
}

/// Bramble's `prepare_transfer`: 1.5 STRK, checked against `trusted` and the ETH and STRK
/// contracts.
pub fn transfer(
    account: &Account,
    recipient: &str,
    trusted: &[Felt],
) -> Result<Transfer, TransferRefusal> {
    let token = Token {
        address: f(STRK),
        symbol: "STRK",
        decimals: 18,
    };
    Transfer::new(
        account,
        token,
        recipient,
        "1.5",
        trusted,
        &[f(STRK), f(ETH)],
        None,
    )
}

pub fn reply(id: u64, recorded: &Value) -> StepReply {
    let mut body = json!({ "jsonrpc": "2.0", "id": id });
    for (key, value) in recorded.as_object().unwrap() {
        body[key] = value.clone();
    }
    StepReply::Rpc {
        id,
        body: body.to_string(),
        now_ms: NOW,
    }
}

/// Drives one phase: every RPC the plan emits must be the next recorded exchange. The core proves
/// the chain where the extension's review does not, and the standalone deployment review reads
/// the class to show "already deployed"; those extra reads are answered from the scenario.
pub fn drive(
    plan: &mut Plan,
    host: &Host,
    mut step: SendStep,
    expected: &[Value],
    scenario: &Value,
) -> SendStep {
    let name = &scenario["name"];
    let mut next = 0;
    loop {
        match &step {
            SendStep::Rpc { id, body, .. } => {
                let sent: Value = serde_json::from_str(body).unwrap();
                let method = sent["method"].as_str().unwrap();
                let recorded = match expected.get(next) {
                    Some(exchange) if exchange["method"] == method => {
                        let params = match &exchange["params"] {
                            Value::Null => json!([]),
                            params => params.clone(),
                        };
                        assert_eq!(sent["params"], params, "{name}: {method}");
                        next += 1;
                        exchange["reply"].clone()
                    }
                    _ if method == "starknet_chainId" => {
                        json!({"result": felt::canonical(&network(scenario).chain_id())})
                    }
                    _ if method == "starknet_getClassHashAt"
                        && scenario["flow"] == "deployment" =>
                    {
                        match scenario["reviewState"]["deployed"].as_bool() {
                            Some(true) => json!({"result": OZ_CLASS}),
                            _ => json!({"error": {"code": 20, "message": "Contract not found"}}),
                        }
                    }
                    other => panic!("{name}: core sent {method}, extension sent {other:?}"),
                };
                step = plan.advance(host, reply(*id, &recorded)).unwrap();
            }
            SendStep::Wait { .. } => step = plan.advance(host, StepReply::WaitElapsed).unwrap(),
            _ => {
                assert_eq!(
                    next,
                    expected.len(),
                    "{name}: core stopped early at {step:?}"
                );
                return step;
            }
        }
    }
}

pub fn review_calls(scenario: &Value) -> Vec<Value> {
    ["reviewInvoke", "reviewDeployment"]
        .iter()
        .flat_map(|name| phase(scenario, name))
        .filter(|exchange| exchange["method"] != "starknet_chainId")
        .collect()
}

pub fn fee_matches(fee: Option<String>, expected: &Value, name: &str) {
    assert_eq!(
        fee,
        expected["overallFee"].as_str().map(String::from),
        "{name}"
    );
}

pub fn run(scenario: &Value, family: &str) -> (Plan, Host) {
    let name = scenario["name"].as_str().unwrap();
    let host = Host::new();
    let account = account(scenario, family, host.public_key());
    if let Some(expected) = scenario["account"].get("publicKey") {
        assert_eq!(account.public_key, f(expected.as_str().unwrap()), "{name}");
    }
    let mut plan = plan(scenario, account);
    let step = plan.advance(&host, StepReply::Start).unwrap();
    let step = drive(&mut plan, &host, step, &review_calls(scenario), scenario);
    assert_eq!(step, SendStep::Review, "{name}");
    let review = plan.current_review().unwrap().clone();
    assert!(review.can_confirm, "{name}: {review:?}");
    fee_matches(
        review.fee.map(|f| f.max_spend),
        &scenario["reviewedFee"],
        name,
    );
    fee_matches(
        review.deployment_fee.map(|f| f.max_spend),
        &scenario["reviewedDeploymentFee"],
        name,
    );

    let step = plan.advance(&host, StepReply::Confirm).unwrap();
    let step = drive(&mut plan, &host, step, &phase(scenario, "submit"), scenario);
    let hash = scenario["transactionHash"].as_str().unwrap();
    assert_eq!(step, SendStep::Broadcast { hash: hash.into() }, "{name}");
    let signed = scenario
        .get("signatures")
        .or_else(|| scenario.get("signed"));
    // The same hashes, signed in the extension's order (Braavos signs its deployment
    // parameters before the transaction hash).
    let expected: Vec<Felt> = signed
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|record| f(record["hash"].as_str().unwrap()))
        .collect();
    assert_eq!(*host.signed.borrow(), expected, "{name}");
    assert_eq!(plan.signatures_made() as usize, expected.len(), "{name}");
    // The pending record was handed over before the write left the core, keyed by its hash.
    let records = host.records.borrow();
    let record = records
        .iter()
        .find(|r| felt::canonical(&r.hash) == hash)
        .expect("pending");
    assert_eq!(record.submitted_at, NOW);
    match (&record.kind, scenario["flow"].as_str().unwrap()) {
        (
            PendingKind::Transfer {
                amount,
                symbol,
                recipient,
                ..
            },
            "transfer",
        ) => {
            assert_eq!(record.summary, "Send 1.5 STRK to 0x3fa7…41a3");
            assert_eq!((amount.as_str(), symbol.as_str()), ("1.5", "STRK"));
            assert_eq!(*recipient, f(scenario["recipient"].as_str().unwrap()));
        }
        (PendingKind::Deployment, "deployment") => {
            assert_eq!(record.summary, "Deploy account contract");
        }
        other => panic!("{name}: {other:?}"),
    }
    drop(records);
    (plan, host)
}
