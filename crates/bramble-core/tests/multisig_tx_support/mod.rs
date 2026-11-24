//! Shared by the multisig transaction export suites: a wallet holding the fixture phrase's
//! OpenZeppelin signers and the fixture multisig, the reads the host drives before each
//! operation, and a driver that answers a plan's RPC steps from the recorded exchanges.
#![allow(dead_code)] // Each suite reads a different subset.

use std::sync::Arc;

use bramble_core::{
    ArgentMultisigConfig, MultisigAuthorityRead, MultisigAuthorityStatus, MultisigIntent,
    MultisigProposalRequest, MultisigStateRead, MultisigTxStep, NetworkId, SendStep, StepReply,
    TokenPreferences, TransferPlan, WalletCore,
};
use serde_json::{json, Value};

use super::common;
use super::multisig_support::{decode, fixture, rpc_reply};

pub struct Wallet {
    pub dir: tempfile::TempDir,
    pub core: Arc<WalletCore>,
    pub network: NetworkId,
    pub net: Value,
}

pub fn network_of(net: &Value) -> NetworkId {
    match net["networkId"].as_str().unwrap() {
        "SN_MAIN" => NetworkId::Mainnet,
        _ => NetworkId::Sepolia,
    }
}

fn chain_id(network: NetworkId) -> &'static str {
    match network {
        NetworkId::Mainnet => "0x534e5f4d41494e",
        NetworkId::Sepolia => "0x534e5f5345504f4c4941",
    }
}

fn strings(value: &Value) -> Vec<String> {
    serde_json::from_value(value.clone()).unwrap()
}

/// The fixture phrase imported, its OpenZeppelin accounts 0 to 3 restored on `net`'s network, and
/// the fixture multisig saved as the setup its deployer created.
pub fn wallet(net: &Value) -> Wallet {
    let (dir, _clock, core) = common::core();
    let phrase = fixture()["mnemonic"].as_str().unwrap().to_string();
    core.import_wallet(common::PASSWORD.into(), phrase).unwrap();
    let network = network_of(net);
    core.restore_recovered_accounts(
        network,
        vec![common::oz(0), common::oz(1), common::oz(2), common::oz(3)],
    )
    .unwrap();
    let wallet = Wallet {
        dir,
        core,
        network,
        net: net.clone(),
    };
    let input = &net["deploy"]["input"];
    let deployer = wallet.signer_id(net["deploy"]["deployerAccountIndex"].as_u64().unwrap());
    let saved = wallet
        .core
        .save_multisig_setup(
            network,
            None,
            ArgentMultisigConfig {
                threshold: input["threshold"].as_u64().unwrap() as u32,
                signer_public_keys: strings(&input["signerPublicKeys"]),
            },
            input["salt"].as_str().unwrap().into(),
            deployer,
        )
        .unwrap();
    assert_eq!(saved.id, net["account"]["id"].as_str().unwrap());
    wallet
}

impl Wallet {
    pub fn multisig_id(&self) -> String {
        self.net["account"]["id"].as_str().unwrap().into()
    }

    pub fn address(&self) -> String {
        self.net["account"]["address"].as_str().unwrap().into()
    }

    /// The vault record of the fixture signer at OpenZeppelin index `index`.
    pub fn signer_id(&self, index: u64) -> String {
        let signer = self.net["signers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|signer| signer["accountIndex"] == index)
            .unwrap();
        self.core
            .account_records(self.network)
            .unwrap()
            .into_iter()
            .find(|account| account.public_key == signer["publicKey"].as_str().unwrap())
            .unwrap()
            .id
    }

    /// The multisig's signer GUIDs, from the keys its deployment commits to (the chain sorts
    /// them; the read is answered in reverse).
    pub fn guids(&self) -> Vec<String> {
        let mut guids: Vec<String> =
            strings(&self.net["account"]["deployment"]["signerPublicKeys"])
                .iter()
                .map(|key| bramble_core::argent_signer_guid(key.clone()).unwrap())
                .collect();
        guids.sort_by_key(|guid| format!("{:0>64}", &guid[2..]));
        guids
    }

    /// A restored account whose key is not one of the multisig's signers.
    pub fn outsider(&self) -> String {
        let members = strings(&self.net["account"]["deployment"]["signerPublicKeys"]);
        self.core
            .account_records(self.network)
            .unwrap()
            .into_iter()
            .find(|account| !members.contains(&account.public_key))
            .expect("an account outside the signer set")
            .id
    }

    pub fn threshold(&self) -> u32 {
        self.net["account"]["deployment"]["threshold"]
            .as_u64()
            .unwrap() as u32
    }

    /// The person's tracked tokens: the defaults.
    pub fn tokens(&self) -> Arc<TokenPreferences> {
        TokenPreferences::new(None)
    }

    /// The fixture's proposal, one STRK transfer, as the request a screen sends: the amount
    /// is the call's u256 written as decimal STRK text.
    pub fn transfer(&self) -> MultisigProposalRequest {
        let calls = self.net["proposal"]["calls"].as_array().unwrap();
        assert_eq!(calls.len(), 1);
        let call = &calls[0];
        assert_eq!(call["entrypoint"], "transfer");
        let calldata = strings(&call["calldata"]);
        assert_eq!(calldata[2], "0x0");
        let low = u128::from_str_radix(&calldata[1][2..], 16).unwrap();
        let unit = 10u128.pow(18);
        let fraction = format!("{:018}", low % unit);
        let fraction = fraction.trim_end_matches('0');
        let whole = low / unit;
        let token = call["contractAddress"].as_str().unwrap();
        let tracked = self.tokens().tracked(self.network);
        let token_id = tracked
            .iter()
            .find(|tracked| {
                let digits = |hex: &str| hex[2..].trim_start_matches('0').to_lowercase();
                digits(&tracked.token.address) == digits(token)
            })
            .expect("the fixture's token is tracked")
            .token
            .id
            .clone();
        MultisigProposalRequest::Transfer {
            token_id,
            recipient: calldata[0].clone(),
            amount: if fraction.is_empty() {
                whole.to_string()
            } else {
                format!("{whole}.{fraction}")
            },
        }
    }

    /// An authority read of the multisig answered with `threshold` and `guids`.
    pub fn authority(&self, threshold: u32, guids: &[String]) -> Arc<MultisigAuthorityRead> {
        let read = MultisigAuthorityRead::new(self.network, self.address()).unwrap();
        for result in self.authority_replies(threshold, guids) {
            let request = read.next_request().unwrap();
            read.accept_response(request.id, body(request.id, result))
                .unwrap();
        }
        assert_eq!(read.status(), Some(MultisigAuthorityStatus::Available));
        read
    }

    /// The chain as the fixture leaves it: its signers and threshold.
    pub fn current_authority(&self) -> Arc<MultisigAuthorityRead> {
        self.authority(self.threshold(), &self.guids())
    }

    /// A state read answered with `threshold`, `guids` and `nonce`.
    pub fn state(&self, threshold: u32, guids: &[String], nonce: &str) -> Arc<MultisigStateRead> {
        let read = MultisigStateRead::new(self.network, self.address()).unwrap();
        let mut replies = self.authority_replies(threshold, guids);
        replies.push(json!(nonce));
        for result in replies {
            let request = read.next_request().unwrap();
            read.accept_response(request.id, body(request.id, result))
                .unwrap();
        }
        assert!(read.next_request().is_none());
        assert_eq!(read.status(), Some(MultisigAuthorityStatus::Available));
        read
    }

    /// The state an intent was frozen against: the fixture's authority and its nonce.
    pub fn current_state(&self, intent: &MultisigIntent) -> Arc<MultisigStateRead> {
        self.state(self.threshold(), &self.guids(), &intent.nonce)
    }

    pub fn authority_replies(&self, threshold: u32, guids: &[String]) -> Vec<Value> {
        let mut signers = vec![json!(format!("0x{:x}", guids.len()))];
        signers.extend(guids.iter().rev().map(|guid| json!(guid)));
        vec![
            json!(chain_id(self.network)),
            json!({"block_hash": "0x1", "block_number": 1}),
            json!(self.net["account"]["classHash"]),
            json!([format!("0x{threshold:x}")]),
            Value::Array(signers),
        ]
    }

    /// The fixture's proposal, driven to its stored collecting intent.
    pub fn propose(&self) -> MultisigIntent {
        let proposal = self
            .core
            .clone()
            .propose_multisig_transaction(self.current_authority(), self.transfer(), self.tokens())
            .unwrap();
        let step = proposal.advance(StepReply::Start).unwrap();
        match drive(step, &phase(&self.net["proposal"]["prepare"]), |reply| {
            proposal.advance(reply).unwrap()
        }) {
            MultisigTxStep::Proposed { intent_id } => {
                let intent = proposal.intent().unwrap();
                assert_eq!(intent.id, intent_id);
                intent
            }
            other => panic!("{other:?}"),
        }
    }

    pub fn sign(&self, intent: &MultisigIntent, index: u64) -> MultisigIntent {
        self.core
            .sign_multisig_intent(
                intent.id.clone(),
                self.signer_id(index),
                self.current_state(intent),
            )
            .unwrap()
    }

    /// The proposal, co-signed by every fixture co-signer.
    pub fn ready(&self) -> MultisigIntent {
        let mut intent = self.propose();
        for index in self.net["proposal"]["cosignerAccountIndexes"]
            .as_array()
            .unwrap()
        {
            intent = self.sign(&intent, index.as_u64().unwrap());
        }
        intent
    }

    /// Relocks and unlocks: a new session.
    pub fn relock(&self) {
        self.core.lock();
        self.core.unlock(common::PASSWORD.into()).unwrap();
    }
}

pub fn body(id: u64, result: Value) -> String {
    json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string()
}

/// A recorded phase's exchanges, decoded.
pub fn phase(lines: &Value) -> Vec<Value> {
    lines.as_array().unwrap().iter().map(decode).collect()
}

/// Answers every RPC step of a multisig plan from `exchanges`, in order, holding each body to the
/// recorded one; returns the first other step, after every exchange was used.
pub fn drive(
    mut step: MultisigTxStep,
    exchanges: &[Value],
    mut advance: impl FnMut(StepReply) -> MultisigTxStep,
) -> MultisigTxStep {
    let mut next = 0;
    while let MultisigTxStep::Rpc { id, body, .. } = &step {
        let sent: Value = serde_json::from_str(body).unwrap();
        let exchange = exchanges.get(next).expect("a recorded exchange");
        assert_eq!(sent["method"], exchange["method"]);
        let params = match &exchange["params"] {
            Value::Null => json!([]),
            params => params.clone(),
        };
        assert_eq!(sent["params"], params, "{}", exchange["method"]);
        next += 1;
        step = advance(rpc_reply(*id, &exchange["reply"]));
    }
    assert_eq!(next, exchanges.len(), "stopped early at {step:?}");
    step
}

/// Answers a deployment plan's RPC steps from `exchanges`; the plan proves the chain first, so a
/// `starknet_chainId` may be taken from later in the phase (`multisig_deploy_parity.rs`).
pub fn drive_deployment(plan: &TransferPlan, mut step: SendStep, exchanges: &[Value]) -> SendStep {
    let mut pending: Vec<&Value> = exchanges.iter().collect();
    loop {
        let SendStep::Rpc { id, body, .. } = &step else {
            assert!(pending.is_empty(), "stopped early at {step:?}");
            return step;
        };
        let sent: Value = serde_json::from_str(body).unwrap();
        let method = sent["method"].as_str().unwrap();
        let at = pending
            .iter()
            .position(|exchange| exchange["method"] == method)
            .expect("a recorded exchange");
        assert!(
            at == 0 || method == "starknet_chainId",
            "{method} out of order"
        );
        let exchange = pending.remove(at);
        let params = match &exchange["params"] {
            Value::Null => json!([]),
            params => params.clone(),
        };
        assert_eq!(sent["params"], params, "{method}");
        let StepReply::Rpc { body, .. } = rpc_reply(*id, &exchange["reply"]) else {
            unreachable!()
        };
        step = plan
            .advance(StepReply::Rpc {
                id: *id,
                body,
                now_ms: 1_790_000_000_000,
            })
            .unwrap();
    }
}
