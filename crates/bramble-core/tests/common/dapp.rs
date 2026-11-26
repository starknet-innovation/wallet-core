//! The dapp sign tests' helpers (`tests/dapp_sign*.rs`): a session with `SITE` connected, a
//! typed-data request, and checking the signature the page receives.

use std::sync::Arc;

use bramble_core::ledger::LedgerScheme;
use bramble_core::{
    AccountFamily, DappReply, DappSession, DappSignStep, LedgerAccountRequest, NetworkId,
    TokenPreferences, WalletCore,
};
use serde_json::{json, Value};
use starknet_wallet_core::{kms, sign_stark_hash, Felt, SecretFelt};

use super::account_id;
use super::ledger_device::Device;

pub const SITE: &str = "https://app.example";
pub const NOW: f64 = 1_800_000_000_000.0;

/// A revision 1 document Krusty hashes, bound to `chain`.
pub fn typed_data(chain: &str, primary: &str) -> String {
    format!(
        r#"{{"domain":{{"chainId":"{chain}","name":"Bramble Test","revision":"1","version":"1"}},"message":{{"content":"hello"}},"primaryType":"{primary}","types":{{"{primary}":[{{"name":"content","type":"felt"}}],"StarknetDomain":[{{"name":"name","type":"shortstring"}},{{"name":"version","type":"shortstring"}},{{"name":"chainId","type":"shortstring"}},{{"name":"revision","type":"shortstring"}}]}}}}"#
    )
}

pub fn sepolia_mail() -> String {
    typed_data("SN_SEPOLIA", "Mail")
}

pub fn sign_request(
    dapp: &DappSession,
    origin: &str,
    document: &str,
    request_id: u64,
) -> DappReply {
    dapp.request(
        origin.into(),
        "wallet_signTypedData".into(),
        Some(format!(r#"{{"typed_data":{document}}}"#)),
        NetworkId::Sepolia,
        request_id,
    )
    .unwrap()
}

pub fn code(reply: &DappReply) -> i64 {
    match reply {
        DappReply::Error { code, .. } => *code,
        other => panic!("not an error: {other:?}"),
    }
}

pub fn pending_id(reply: DappReply) -> String {
    match reply {
        DappReply::Pending { approval_id } => approval_id,
        other => panic!("not pending: {other:?}"),
    }
}

/// A session over `core` with `SITE` connected to the active account.
pub fn connected(core: &Arc<WalletCore>) -> Arc<DappSession> {
    let dapp = DappSession::new(core.clone(), TokenPreferences::new(None));
    let reply = dapp
        .request(
            SITE.into(),
            "wallet_requestAccounts".into(),
            None,
            NetworkId::Sepolia,
            0,
        )
        .unwrap();
    let card = pending_id(reply);
    assert_eq!(dapp.approve_connect(card, account_id(core), NOW).len(), 1);
    dapp
}

/// The one reply an approval settles, for an account that needs no chain read (OpenZeppelin).
pub fn approved(dapp: &DappSession, card: String, account: String) -> DappReply {
    approved_on(dapp, card, account, &Chain::none())
}

/// The one reply an approval settles, answering each live account read from `chain`.
pub fn approved_on(dapp: &DappSession, card: String, account: String, chain: &Chain) -> DappReply {
    let step = dapp.approve_sign_typed_data(card.clone(), account, NOW);
    settled_reply(drive(dapp, &card, step, chain, None))
}

/// The one reply a step settles.
pub fn settled_reply(step: DappSignStep) -> DappReply {
    match step {
        DappSignStep::Settled { settled } => {
            assert_eq!(settled.len(), 1);
            settled[0].reply.clone()
        }
        other => panic!("not settled: {other:?}"),
    }
}

/// Answers `card`'s reads from `chain` and its APDUs from `device` until it settles or parks
/// (with no device, at its first APDU).
pub fn drive(
    dapp: &DappSession,
    card: &str,
    mut step: DappSignStep,
    chain: &Chain,
    mut device: Option<&mut Device>,
) -> DappSignStep {
    loop {
        step = match step {
            DappSignStep::Read { id, body, .. } => {
                dapp.answer_sign_read(card.into(), id, chain.reply(id, &body))
            }
            DappSignStep::Device { apdu } if device.is_some() => {
                let device = device.as_deref_mut().unwrap();
                dapp.answer_sign_device(card.into(), device.answer(&apdu))
            }
            other => return other,
        }
    }
}

/// A Ledger key the published phrase does not derive.
pub fn ledger_key() -> SecretFelt {
    SecretFelt::new(Felt::from(0x1ed6e5_u64))
}

/// Adds `key` as a Ready (Argent) Ledger account on Sepolia, which becomes the active one.
pub fn add_ledger(core: &WalletCore, key: &SecretFelt) {
    let public_key_x = sign_stark_hash(key, "0x1").unwrap().public_key;
    let added = core
        .add_ledger_account(LedgerAccountRequest {
            network: NetworkId::Sepolia,
            family: AccountFamily::Argent,
            scheme: LedgerScheme::Ready,
            account_index: 0,
            public_key_x,
        })
        .unwrap();
    assert!(added.ledger);
}

pub fn class(label: &str) -> String {
    bramble_core::account_classes()
        .into_iter()
        .find(|class| class.label == label)
        .unwrap()
        .class_hash
}

/// The live reads of a Ready account running `class_label`, with `owner` its only signer.
pub fn ready_chain(class_label: &str, owner: &str) -> Chain {
    use starknet_wallet_core::send::plan_probe::{GET_GUARDIAN, GET_OWNER, GET_OWNER_TYPE};
    Chain {
        class: Some(class(class_label)),
        views: vec![
            (GET_OWNER, vec![owner.to_owned()]),
            (GET_OWNER_TYPE, vec!["0x0".into()]),
            (GET_GUARDIAN, vec!["0x0".into()]),
        ],
    }
}

/// Whether an approval answered nothing (a card already decided, expired or of another kind).
pub fn answers_nothing(step: DappSignStep) -> bool {
    matches!(step, DappSignStep::Settled { settled } if settled.is_empty())
}

/// A scripted node for one account's live reads: its class (`None`: not deployed) and its views'
/// results by selector. A read nothing scripts panics.
pub struct Chain {
    pub class: Option<String>,
    pub views: Vec<(&'static str, Vec<String>)>,
}

impl Chain {
    pub fn none() -> Self {
        Self {
            class: None,
            views: Vec::new(),
        }
    }

    fn reply(&self, id: u64, body: &str) -> String {
        let request: Value = serde_json::from_str(body).unwrap();
        let answer = match request["method"].as_str().unwrap() {
            "starknet_chainId" => json!({ "result": "0x534e5f5345504f4c4941" }),
            "starknet_blockHashAndNumber" => {
                json!({ "result": { "block_hash": "0xb10c", "block_number": 7 } })
            }
            "starknet_getClassHashAt" => match &self.class {
                Some(class) => json!({ "result": class }),
                None => json!({ "error": { "code": 20, "message": "Contract not found" } }),
            },
            "starknet_call" => {
                let wanted = felt(
                    request["params"]["request"]["entry_point_selector"]
                        .as_str()
                        .unwrap(),
                );
                let (_, result) = self
                    .views
                    .iter()
                    .find(|(view, _)| felt(view) == wanted)
                    .expect("a scripted view");
                json!({ "result": result })
            }
            other => panic!("unexpected read {other}"),
        };
        let mut reply = answer;
        reply["jsonrpc"] = json!("2.0");
        reply["id"] = json!(id);
        reply.to_string()
    }
}

/// The signature array the page receives, as strings.
pub fn signature(reply: &DappReply) -> Vec<String> {
    match reply {
        DappReply::Result { json } => serde_json::from_str(json).unwrap(),
        other => panic!("not a result: {other:?}"),
    }
}

pub fn felt(text: &str) -> Felt {
    Felt::from_hex(text).unwrap()
}

/// Whether `(r, s)` verifies under the active account's key over Krusty's hash of `document`
/// for its address.
pub fn verifies(core: &WalletCore, document: &str, r: &str, s: &str) -> bool {
    let account = core.active_account().unwrap().unwrap();
    let hash = kms::typed_data_message_hash(document, &felt(&account.address)).unwrap();
    kms::stark_signature_is_valid(&felt(&account.public_key), &hash, &felt(r), &felt(s))
}
