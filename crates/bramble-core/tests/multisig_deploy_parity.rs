//! Deploying an Argent multisig must put on the wire exactly what the extension's deploy path
//! puts there. `extension-multisig-transactions.json`'s `deploy` records, per network,
//! `argentMultisigDeployment` and the platform's `estimateAccountDeploymentFee` /
//! `submitAccountDeployment` for the fixture multisig, signed by one member, against a canned
//! node; this drives `send::multisig_deploy::deployment_plan` under Bramble's pinned class and
//! `SEND_POLICY` with the same replies and requires the same request bodies in the same order,
//! the same reviewed fee, transaction hash and signature.
//!
//! One reorder is deliberate: the plan proves the chain before its first read, where the
//! extension's estimate reads the chain id only when starknet.js hashes the query. The recorded
//! `starknet_chainId` exchange is still required, once.

mod plan_support;

use std::cell::RefCell;

use bramble_core::catalogs::multisig::argent_multisig_class;
use bramble_core::SEND_POLICY;
use plan_support::{f, network, reply, NOW};
use serde_json::{json, Value};
use starknet_wallet_core::accounts::multisig::{predict, signer_guid_hex, ArgentMultisigConfig};
use starknet_wallet_core::accounts::Ladder;
use starknet_wallet_core::kms::account_key;
use starknet_wallet_core::send::judge::{Observation, TxStatus};
use starknet_wallet_core::send::multisig_deploy::{
    deployment, deployment_plan, MultisigDeployRefusal,
};
use starknet_wallet_core::send::plan::{PendingKind, PendingRecord, Plan, PlanHost, SignRefusal};
use starknet_wallet_core::send::types::{SendFailureCode, SendStep, StepReply};
use starknet_wallet_core::{felt, sign_stark_hash, Felt, SecretFelt, StarkSignature};

fn fixture() -> Value {
    let path = format!(
        "{}/tests/fixtures/extension-multisig-transactions.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// The fixture's networks: Sepolia and Mainnet.
fn networks(fixture: &Value) -> &[Value] {
    let networks = fixture["networks"].as_array().unwrap();
    assert_eq!(networks.len(), 2);
    networks
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect()
}

fn config(value: &Value) -> ArgentMultisigConfig {
    ArgentMultisigConfig {
        threshold: value["threshold"].as_u64().unwrap() as u32,
        signer_public_keys: strings(&value["signerPublicKeys"]),
    }
}

fn decoded(lines: &Value) -> Vec<Value> {
    lines
        .as_array()
        .unwrap()
        .iter()
        .map(|line| serde_json::from_str(line.as_str().unwrap()).unwrap())
        .collect()
}

/// One member of the fixture phrase's OpenZeppelin ladder, signing as the plan asks.
struct Member {
    key: SecretFelt,
    signed: RefCell<Vec<(Felt, StarkSignature)>>,
    records: RefCell<Vec<PendingRecord>>,
}

impl Member {
    fn new(fixture: &Value, index: u32) -> Self {
        let phrase = fixture["mnemonic"].as_str().unwrap();
        Self {
            key: account_key(phrase, None, Ladder::Account, index).unwrap(),
            signed: RefCell::default(),
            records: RefCell::default(),
        }
    }

    fn public_key(&self) -> Felt {
        f(&sign_stark_hash(&self.key, "0x1").unwrap().public_key)
    }
}

impl PlanHost for Member {
    fn session_current(&self) -> bool {
        true
    }
    fn sign(&self, hash: &Felt, _renew: bool) -> Result<StarkSignature, SignRefusal> {
        let signature =
            sign_stark_hash(&self.key, &felt::canonical(hash)).map_err(|_| SignRefusal::Failed)?;
        self.signed.borrow_mut().push((*hash, signature.clone()));
        Ok(signature)
    }
    fn record_pending(&self, record: PendingRecord) {
        self.records.borrow_mut().push(record);
    }
    fn observe(&self, _hash: &Felt, _observation: Observation) -> Option<TxStatus> {
        None
    }
    fn explorer_url(&self, _hash: &str) -> Option<String> {
        None
    }
}

/// Drives one phase: every RPC the plan emits must be the next recorded exchange, byte for byte,
/// except that the plan's chain proof may come first and take the recorded `starknet_chainId`
/// from later in the phase. Every recorded exchange is used exactly once.
fn drive(plan: &mut Plan, host: &Member, mut step: SendStep, recorded: &[Value]) -> SendStep {
    let mut pending: Vec<&Value> = recorded.iter().collect();
    loop {
        let SendStep::Rpc { id, body, .. } = &step else {
            assert!(pending.is_empty(), "stopped early at {step:?}: {pending:?}");
            return step;
        };
        let sent: Value = serde_json::from_str(body).unwrap();
        let method = sent["method"].as_str().unwrap();
        let at = match pending.first() {
            Some(next) if next["method"] == method => 0,
            _ if method == "starknet_chainId" => pending
                .iter()
                .position(|e| e["method"] == method)
                .expect("a recorded chain id"),
            other => panic!("core sent {method}, extension sent {other:?}"),
        };
        let exchange = pending.remove(at);
        let params = match &exchange["params"] {
            Value::Null => json!([]),
            params => params.clone(),
        };
        assert_eq!(sent["params"], params, "{method}");
        step = plan.advance(host, reply(*id, &exchange["reply"])).unwrap();
    }
}

fn deployer(net: &Value, fixture: &Value) -> Member {
    let index = net["deploy"]["deployerAccountIndex"].as_u64().unwrap() as u32;
    Member::new(fixture, index)
}

fn plan_for(net: &Value, member: &Member) -> Plan {
    let input = &net["deploy"]["input"];
    deployment_plan(
        network(net),
        &config(input),
        input["salt"].as_str().unwrap(),
        argent_multisig_class(),
        &member.public_key(),
        SEND_POLICY,
    )
    .unwrap()
}

#[test]
fn the_signers_are_the_fixture_phrases_openzeppelin_accounts() {
    let fixture = fixture();
    for net in networks(&fixture) {
        for signer in net["signers"].as_array().unwrap() {
            let member = Member::new(&fixture, signer["accountIndex"].as_u64().unwrap() as u32);
            let key = felt::canonical(&member.public_key());
            assert_eq!(key, signer["publicKey"].as_str().unwrap());
            assert_eq!(
                signer_guid_hex(&key).unwrap(),
                signer["guid"].as_str().unwrap()
            );
        }
    }
}

#[test]
fn the_deployment_matches_the_extension() {
    let fixture = fixture();
    let class = argent_multisig_class();
    for net in networks(&fixture) {
        let deploy = &net["deploy"];
        let input = &deploy["input"];
        let prediction = predict(&config(input), input["salt"].as_str().unwrap(), class).unwrap();
        let canonical = &deploy["canonical"];
        assert_eq!(prediction.config, config(canonical));
        assert_eq!(prediction.salt, canonical["salt"].as_str().unwrap());
        let expected = &deploy["deployment"];
        assert_eq!(
            prediction.class_hash,
            expected["classHash"].as_str().unwrap()
        );
        assert_eq!(prediction.salt, expected["addressSalt"].as_str().unwrap());
        assert_eq!(
            prediction.constructor_calldata,
            strings(&expected["constructorCalldata"])
        );
        assert_eq!(
            prediction.address,
            expected["contractAddress"].as_str().unwrap()
        );
        assert_eq!(
            prediction.address,
            deploy["predictedAddress"].as_str().unwrap()
        );
        // The record the extension stores for it.
        let account = &net["account"];
        assert_eq!(prediction.address, account["address"].as_str().unwrap());
        assert_eq!(
            prediction.class_hash,
            account["classHash"].as_str().unwrap()
        );
        assert_eq!(prediction.config, config(&account["deployment"]));
        assert_eq!(
            prediction.salt,
            account["deployment"]["salt"].as_str().unwrap()
        );
        let deployment = deployment(&prediction).unwrap();
        assert_eq!(deployment.address(), Some(f(&prediction.address)));
    }
}

#[test]
fn the_deployment_plan_matches_the_extension_wire() {
    let fixture = fixture();
    for net in networks(&fixture) {
        let deploy = &net["deploy"];
        let member = deployer(net, &fixture);
        let mut plan = plan_for(net, &member);

        let step = plan.advance(&member, StepReply::Start).unwrap();
        let step = drive(&mut plan, &member, step, &decoded(&deploy["review"]));
        assert_eq!(step, SendStep::Review);
        let review = plan.current_review().unwrap().clone();
        assert!(review.can_confirm && !review.already_deployed, "{review:?}");
        let fee = review.deployment_fee.unwrap();
        assert_eq!(
            fee.max_spend,
            deploy["reviewedFee"]["overallFee"].as_str().unwrap()
        );
        assert_eq!(format!("{:?}", fee.unit).to_uppercase(), "FRI");
        assert!(
            member.signed.borrow().is_empty(),
            "the review signs nothing"
        );

        let step = plan.advance(&member, StepReply::Confirm).unwrap();
        let step = drive(&mut plan, &member, step, &decoded(&deploy["submit"]));
        let hash = deploy["transactionHash"].as_str().unwrap();
        assert_eq!(step, SendStep::Broadcast { hash: hash.into() });

        let signed = member.signed.borrow();
        let recorded = deploy["signatures"].as_array().unwrap();
        assert_eq!(signed.len(), recorded.len());
        for ((hash, signature), expected) in signed.iter().zip(recorded) {
            assert_eq!(felt::canonical(hash), expected["hash"].as_str().unwrap());
            assert_eq!(signature.r, expected["r"].as_str().unwrap());
            assert_eq!(signature.s, expected["s"].as_str().unwrap());
        }
        let records = member.records.borrow();
        assert_eq!(records.len(), 1);
        assert_eq!(felt::canonical(&records[0].hash), hash);
        assert_eq!(records[0].kind, PendingKind::Deployment);
        assert_eq!(records[0].submitted_at, NOW);
    }
}

/// The Sepolia review, then the submit answered from `submit`; nothing is signed or recorded.
fn submit_with(submit: &[Value]) -> SendStep {
    let fixture = fixture();
    let net = &fixture["networks"][0];
    let member = deployer(net, &fixture);
    let mut plan = plan_for(net, &member);
    let step = plan.advance(&member, StepReply::Start).unwrap();
    drive(&mut plan, &member, step, &decoded(&net["deploy"]["review"]));
    let step = plan.advance(&member, StepReply::Confirm).unwrap();
    let step = drive(&mut plan, &member, step, submit);
    assert!(member.signed.borrow().is_empty() && member.records.borrow().is_empty());
    step
}

/// The review, then the submit's chain proof and class read answered with `class`.
fn submit_with_class(class: &str) -> SendStep {
    let fixture = fixture();
    let mut submit = decoded(&fixture["networks"][0]["deploy"]["submit"]);
    submit.truncate(2);
    submit[1]["reply"] = json!({ "result": class });
    submit_with(&submit)
}

/// `extension-multisig-transactions-refusals.json`'s `deploySubmit`: the extension's
/// `submitAccountDeployment` refusals for the same Sepolia multisig, each mapped to the plan's.
#[test]
fn deployments_the_extension_refuses_are_refused() {
    let path = format!(
        "{}/tests/fixtures/extension-multisig-transactions-refusals.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let refusals: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let fixture = fixture();
    let cases = refusals["deploySubmit"].as_array().unwrap();
    assert_eq!(cases.len(), 2);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        match (name, case["refusal"]["message"].as_str().unwrap()) {
            ("alreadyDeployed", "Account is already deployed") => assert_eq!(
                failure(submit_with(&decoded(&case["wire"]))),
                SendFailureCode::AlreadyDeployed
            ),
            // The extension estimates first (the four reads in `wire`) and refuses when the
            // outsider's signature is serialised; the plan refuses before it reads anything.
            ("nonMemberDeployer", "Argent multisig signature is not from a configured signer") => {
                let index = case["signerAccountIndex"].as_u64().unwrap() as u32;
                let outsider = Member::new(&fixture, index);
                let input = &fixture["networks"][0]["deploy"]["input"];
                let refused = deployment_plan(
                    network(&fixture["networks"][0]),
                    &config(input),
                    input["salt"].as_str().unwrap(),
                    argent_multisig_class(),
                    &outsider.public_key(),
                    SEND_POLICY,
                );
                assert_eq!(
                    refused.err(),
                    Some(MultisigDeployRefusal::DeployerNotMember)
                );
            }
            other => panic!("unmapped extension refusal {other:?}"),
        }
    }
}

fn failure(step: SendStep) -> SendFailureCode {
    match step {
        SendStep::Failed { failure } => failure.code,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_deployed_multisig_is_not_deployed_again() {
    let class = argent_multisig_class().class_hash().to_string();
    assert_eq!(
        failure(submit_with_class(&class)),
        SendFailureCode::AlreadyDeployed
    );
}

#[test]
fn a_multisig_running_another_class_is_refused() {
    assert_eq!(
        failure(submit_with_class("0x1234")),
        SendFailureCode::AccountClassUnrecognised
    );
}

/// Reviews the deployment with the address already running `class`.
fn review_deployed_as(class: &str) -> (Plan, SendStep) {
    let fixture = fixture();
    let net = &fixture["networks"][0];
    let member = deployer(net, &fixture);
    let mut plan = plan_for(net, &member);
    let step = plan.advance(&member, StepReply::Start).unwrap();
    let mut review = decoded(&net["deploy"]["review"]);
    review.retain(|e| e["method"] != "starknet_getNonce" && e["method"] != "starknet_estimateFee");
    review[0]["reply"] = json!({ "result": class });
    let step = drive(&mut plan, &member, step, &review);
    (plan, step)
}

#[test]
fn the_review_of_a_deployed_multisig_cannot_be_confirmed() {
    let (plan, step) = review_deployed_as(argent_multisig_class().class_hash());
    assert_eq!(step, SendStep::Review);
    let review = plan.current_review().unwrap();
    assert!(review.already_deployed && !review.can_confirm);
}

#[test]
fn the_review_names_a_multisig_running_another_class() {
    // The extension's review reports any class as already deployed; this one refuses it.
    let (_, step) = review_deployed_as("0x1234");
    assert_eq!(failure(step), SendFailureCode::AccountClassUnrecognised);
}

#[test]
fn a_multisig_never_sends_on_one_key() {
    use starknet_wallet_core::send::plan::{Account, Authorization, Kind};
    let fixture = fixture();
    let net = &fixture["networks"][0];
    let member = deployer(net, &fixture);
    let account = Account {
        network: network(net),
        address: f(net["account"]["address"].as_str().unwrap()),
        public_key: member.public_key(),
        authorization: Authorization::ArgentMultisig {
            class_hash: argent_multisig_class().hash(),
            query_signer: member.public_key(),
        },
        deployment: None,
    };
    let transfer = plan_support::transfer(&account, plan_support::ACCOUNT_1, &[]).unwrap();
    let mut plan = Plan::new(account, Kind::Transfer(Box::new(transfer)), SEND_POLICY);
    assert_eq!(
        failure(plan.advance(&member, StepReply::Start).unwrap()),
        SendFailureCode::AccountClassUnsupported
    );
}
