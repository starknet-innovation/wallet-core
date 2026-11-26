//! The dapp sign tests' helpers (`tests/dapp_sign*.rs`): a session with `SITE` connected, a
//! typed-data request, and checking the signature the page receives.

use std::sync::Arc;

use bramble_core::{DappReply, DappSession, DappSignStep, NetworkId, TokenPreferences, WalletCore};
use serde_json::{json, Value};
use starknet_wallet_core::{kms, Felt};

use super::account_id;

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
    let mut step = dapp.approve_sign_typed_data(card.clone(), account, NOW);
    loop {
        match step {
            DappSignStep::Settled { settled } => {
                assert_eq!(settled.len(), 1);
                return settled[0].reply.clone();
            }
            DappSignStep::Read { id, body, .. } => {
                step = dapp.answer_sign_read(card.clone(), id, chain.reply(id, &body));
            }
        }
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
