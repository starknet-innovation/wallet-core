//! `wallet_signTypedData` for `DappSession`: a site's SNIP-12 typed data, queued as its own card
//! and signed only when the person approves it. The extension's admission
//! (`background/transactions/sign-typed-data-admission.ts`, `chain-ops.ts`'s
//! `handleSignTypedData`) and its signing at approval (`approvals/approval-decisions.ts`,
//! `approval-claim-context.ts`, `transactions/sign-grant.ts`, `executeSignApproval`).
//!
//! Gate order at request: blocklist, queue room, lock, the granted active account, an account
//! kind this wallet signs typed data for, the document's gate and chain binding (114), and the
//! SNIP-9 refusal; then one card per request, never deduplicated. At approval: claim once,
//! record the site, then the blocklist, lock, the card's account still granted and active (so the
//! network has not moved either), the account kind, the hash again, and the signature through
//! `signing_record` and `sign_for` under the same session hold as every check before it. Every
//! refusal but the document's is UNKNOWN_ERROR (163), as the extension's strings map.
//!
//! Accounts it signs for: OpenZeppelin, and Ready (Argent) whose stored class takes the signer
//! array, each holding its key in the vault (phrase-derived or imported). Refused: Ledger (the
//! device path is not wired for typed data), Braavos (signing needs the live class and signer
//! read), a Ready account stored at an Argent 0.3.x class (it cannot verify the 0.4 signature),
//! and a multisig, which is never a vault record and so never the granted account here.

use starknet_wallet_core::dapp::{describe_typed_data, CheckedTypedData, Site, WalletRpcError};
use starknet_wallet_core::js::json::JsValue;
use starknet_wallet_core::Felt;

use crate::account_catalog::{class_entry, AccountFamily, ARGENT_SIGNER_ARRAY};
use crate::account_store::{KeySource, StoredAccount, VaultPayload};
use crate::dapp::DAPP_POLICY;
use crate::dapp_session::{
    error, result, settled, Card, DappCard, DappReply, DappSession, DappSettled, Shape,
};
use crate::felt;
use crate::networks::NetworkId;
use crate::wallet_accounts::{sign_for, signing_record};

/// A typed-data card: the account it was reviewed for and the document as the page sent it.
pub(crate) struct SignCard {
    account_id: String,
    address: String,
    network: NetworkId,
    summary: String,
    typed_data: JsValue,
}

impl SignCard {
    pub(crate) fn card(&self) -> DappCard {
        DappCard::SignTypedData {
            account_id: self.account_id.clone(),
            address: self.address.clone(),
            network: self.network,
            summary: self.summary.clone(),
            typed_data_json: self.typed_data.to_json(),
        }
    }
}

fn refused() -> WalletRpcError {
    WalletRpcError::unknown_error()
}

/// Whether this wallet signs typed data for `record`: a key the vault holds, and a family whose
/// signature it can spell without reading the chain.
// ponytail: Braavos and an upgraded Ready account need `requireLiveAuthorizableClass` (the live
// class and signers at an accepted block) before signing; wire that read here to admit them.
fn signs_typed_data(record: &StoredAccount) -> bool {
    if !matches!(record.key, KeySource::Phrase(_) | KeySource::Imported) {
        return false;
    }
    match record.family() {
        AccountFamily::OpenZeppelin => true,
        AccountFamily::Argent => class_entry(&record.class_felt()).is_some_and(|entry| {
            entry.family == AccountFamily::Argent
                && entry.info.authorization_shape == ARGENT_SIGNER_ARRAY
        }),
        AccountFamily::Braavos => false,
    }
}

/// A SNIP-9 OutsideExecution authorises calls the site can submit later without a prompt, so it
/// needs the invoke route's review before it can be signed.
// ponytail: refused until the OutsideExecution review lands (its own PR); wire
// `reviewOutsideExecution` in here, at both admission and approval.
fn outside_execution_refusal(checked: &CheckedTypedData<'_>) -> Option<WalletRpcError> {
    let primary = checked.value().get("primaryType");
    matches!(primary, Some(JsValue::String(name)) if name == "OutsideExecution").then(refused)
}

/// The record's message hash for `typed_data`, bound to the record's network: the document's
/// gate and chain binding refuse as INVALID_REQUEST_PAYLOAD, an OutsideExecution as refused.
fn message_hash(record: &StoredAccount, typed_data: &JsValue) -> Result<Felt, WalletRpcError> {
    let checked = CheckedTypedData::check(typed_data, &DAPP_POLICY.typed_data)?;
    let address = felt::parse_non_zero(&record.address).ok_or_else(refused)?;
    let hash = checked.message_hash_for_chain(&address, &record.network().chain_id())?;
    outside_execution_refusal(&checked).map_or(Ok(hash), Err)
}

/// The record the site may sign with now: the granted active account, kept by the vault and of a
/// kind this wallet signs typed data for; refused while locked or blocked.
fn granted_signer<'a>(
    site: &Site<'_>,
    payload: Option<&'a VaultPayload>,
) -> Result<&'a StoredAccount, WalletRpcError> {
    let payload = payload.ok_or_else(refused)?;
    let account = site.granted().ok_or_else(refused)?;
    let record = signing_record(payload, &account.id).map_err(|_| refused())?;
    signs_typed_data(record)
        .then_some(record)
        .ok_or_else(refused)
}

/// `serializeAccountSignature` as the page reads it (`signaturePayload`): OpenZeppelin `[r, s]`,
/// Ready `["1", "0", public_key, r, s]`, each felt canonical and non-zero.
fn signature_json(
    record: &StoredAccount,
    signature: &crate::kms::Signature,
) -> Result<JsValue, WalletRpcError> {
    let canonical = |value: &str| {
        felt::parse_non_zero(value)
            .map(|felt| JsValue::String(felt::canonical(&felt)))
            .ok_or_else(refused)
    };
    let (r, s) = (canonical(&signature.r)?, canonical(&signature.s)?);
    Ok(JsValue::Array(match record.family() {
        AccountFamily::Argent => vec![
            JsValue::String("1".into()),
            JsValue::String("0".into()),
            canonical(&record.public_key)?,
            r,
            s,
        ],
        _ => vec![r, s],
    }))
}

impl DappSession {
    /// `handleSignTypedData`: a card for the granted account, or the refusal.
    pub(crate) fn request_sign(
        &self,
        origin: &str,
        typed_data: &JsValue,
        request_id: u64,
    ) -> DappReply {
        // The blocklist and the queue's room come before any vault read or hashing.
        if self.blocked(origin) || !self.can_admit(origin) {
            return error(refused());
        }
        // The shown network answers nothing here: only an unlocked session signs.
        let card = self.with_site(origin, NetworkId::Mainnet, false, |site, session| {
            Ok(admit(
                site,
                session.map(|session| &session.payload),
                typed_data,
            ))
        });
        match card.unwrap_or_else(|_| Err(refused())) {
            Ok(card) => self.queue(origin, Card::Sign(card), request_id, Shape::Signature),
            Err(refusal) => error(refusal),
        }
    }
}

/// The card a granted site's typed data queues as, or why it is refused.
fn admit(
    site: &Site<'_>,
    payload: Option<&VaultPayload>,
    typed_data: &JsValue,
) -> Result<SignCard, WalletRpcError> {
    let record = granted_signer(site, payload)?;
    message_hash(record, typed_data)?;
    Ok(SignCard {
        account_id: record.id.clone(),
        address: record.address.clone(),
        network: record.network(),
        summary: describe_typed_data(typed_data),
        typed_data: typed_data.clone(),
    })
}

/// The approved card's signature array, if every check still holds.
fn sign(
    site: &Site<'_>,
    payload: Option<&VaultPayload>,
    card: &SignCard,
    reviewed_account_id: &str,
) -> Result<JsValue, WalletRpcError> {
    if site.blocked || reviewed_account_id != card.account_id {
        return Err(refused());
    }
    let record = granted_signer(site, payload)?;
    let payload = payload.ok_or_else(refused)?;
    if record.id != card.account_id {
        return Err(refused());
    }
    let hash = message_hash(record, &card.typed_data)?;
    let signature = sign_for(payload, record, &felt::canonical(&hash)).map_err(|_| refused())?;
    signature_json(record, &signature)
}

#[uniffi::export]
impl DappSession {
    /// The person approved `approval_id`'s typed-data card, reviewed for `reviewed_account_id`
    /// (the card's `account_id`, echoed). Signs only while that account is still the active one
    /// granted to the site, on the network it was reviewed on, with the site not blocked and the
    /// wallet unlocked, and answers the request with the signature array; otherwise the request
    /// is refused. Either way the card is spent. A connect card, or a card already settled or
    /// expired, answers nothing. Renews. `now_ms` is the host's wall clock.
    pub fn approve_sign_typed_data(
        &self,
        approval_id: String,
        reviewed_account_id: String,
        now_ms: f64,
    ) -> Vec<DappSettled> {
        let Some((origin, Card::Sign(card), waiters)) =
            self.claim(&approval_id, |card| matches!(card, Card::Sign(_)))
        else {
            return Vec::new();
        };
        self.record_seen(&origin, now_ms);
        // Every check and the signature run under one hold of the session: no lock, switch or
        // revocation can land between them.
        let signed = self.with_site(&origin, card.network, true, |site, session| {
            let payload = session.map(|session| &session.payload);
            Ok(sign(site, payload, &card, &reviewed_account_id))
        });
        let reply = match signed.unwrap_or_else(|_| Err(refused())) {
            Ok(json) => result(&json),
            Err(refusal) => error(refusal),
        };
        settled(waiters, |_| reply.clone())
    }
}
