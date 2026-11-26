//! The dapp sign tests' helpers (`tests/dapp_sign*.rs`): a session with `SITE` connected, a
//! typed-data request, and checking the signature the page receives.

use std::sync::Arc;

use bramble_core::{DappReply, DappSession, NetworkId, TokenPreferences, WalletCore};
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

/// The one reply an approval settles.
pub fn approved(dapp: &DappSession, card: String, account: String) -> DappReply {
    let settled = dapp.approve_sign_typed_data(card, account, NOW);
    assert_eq!(settled.len(), 1);
    settled[0].reply.clone()
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
