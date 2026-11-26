//! `wallet_signTypedData` for `DappSession`: a site's SNIP-12 typed data, queued as its own card
//! and signed only when the person approves it. The extension's admission
//! (`background/transactions/sign-typed-data-admission.ts`, `chain-ops.ts`'s
//! `handleSignTypedData`) and its signing at approval (`approvals/approval-decisions.ts`,
//! `approval-claim-context.ts`, `transactions/sign-grant.ts`, `executeSignApproval`).
//!
//! Gate order at request: blocklist, queue room, lock, the granted active account, an account
//! kind this wallet signs typed data for, the document's gate and chain binding (114), and a
//! SNIP-9 outside execution's review (`review_outside_execution`: its calls flagged as an invoke
//! card's are, a self-call refused); then one card per request, never deduplicated. At approval: claim once,
//! record the site, then the blocklist, lock, the card's account still granted and active (so the
//! network has not moved either), the account kind, the hash again, an outside execution's
//! review again (now refusing a lookalike payee or spender), and the signature through
//! `signing_record` and `sign_for` under the same session hold as every check before it. Every
//! refusal but the document's is UNKNOWN_ERROR (163), as the extension's strings map.
//!
//! Accounts it signs for: OpenZeppelin, and Ready (Argent) whose stored class takes the signer
//! array, each holding its key in the vault (phrase-derived or imported). Refused: Ledger (the
//! device path is not wired for typed data), Braavos (signing needs the live class and signer
//! read), a Ready account stored at an Argent 0.3.x class (it cannot verify the 0.4 signature),
//! and a multisig, which is never a vault record and so never the granted account here.

use starknet_wallet_core::allowances::record::AllowanceRecord;
use starknet_wallet_core::catalog::TokenInfo;
use starknet_wallet_core::dapp::{
    describe_typed_data, review_outside_execution, CheckedTypedData, OutsideExecutionReview, Site,
    Stage, WalletRpcError,
};
use starknet_wallet_core::js::json::JsValue;
use starknet_wallet_core::send::call_decode::Known;
use starknet_wallet_core::send::review_flags::ReviewContext;
use starknet_wallet_core::Felt;

use crate::account_catalog::{class_entry, AccountFamily, ARGENT_SIGNER_ARRAY};
use crate::account_store::{KeySource, StoredAccount, VaultPayload};
use crate::allowances::{token_infos, trusted_tokens, with_allowance_log};
use crate::dapp::DAPP_POLICY;
use crate::dapp_session::{
    error, result, settled, Card, DappCard, DappReply, DappSession, DappSettled, Shape,
};
use crate::felt;
use crate::networks::NetworkId;
use crate::send::types::RiskFlag;
use crate::send::wallet_ext::known_token_addresses;
use crate::wallet_accounts::{sign_for, signing_record};

/// A typed-data card: the account it was reviewed for and the document as the page sent it.
pub(crate) struct SignCard {
    account_id: String,
    address: String,
    network: NetworkId,
    summary: String,
    typed_data: JsValue,
    call_summaries: Vec<String>,
    flags: Vec<RiskFlag>,
}

impl SignCard {
    pub(crate) fn card(&self) -> DappCard {
        DappCard::SignTypedData {
            account_id: self.account_id.clone(),
            address: self.address.clone(),
            network: self.network,
            summary: self.summary.clone(),
            typed_data_json: self.typed_data.to_json(),
            call_summaries: self.call_summaries.clone(),
            flags: self.flags.clone(),
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

/// A SNIP-9 OutsideExecution authorises calls the site can submit later without a prompt
/// (`isOutsideExecution`), so its calls are reviewed as an invoke's are.
fn is_outside_execution(typed_data: &JsValue) -> bool {
    matches!(typed_data.get("primaryType"), Some(JsValue::String(name)) if name == "OutsideExecution")
}

/// What an outside execution's review reads besides the document, for one network: the tracked
/// tokens, every address the person recognises, the known token contracts and the allowance
/// log. Read before the session hold, since `trusted_recipients` takes the session itself.
pub(crate) struct ReviewInputs {
    network: NetworkId,
    tokens: Vec<TokenInfo>,
    trusted: Vec<Felt>,
    token_contracts: Vec<Felt>,
    standing: Vec<AllowanceRecord>,
}

impl ReviewInputs {
    fn context(&self) -> ReviewContext<'_> {
        ReviewContext {
            known: Known {
                tokens: &self.tokens,
                nft_contracts: &[],
            },
            trusted: &self.trusted,
            known_contracts: &[],
            token_contracts: &self.token_contracts,
            standing: &self.standing,
        }
    }
}

impl DappSession {
    /// The review inputs for `typed_data` on `network`, read only for an outside execution;
    /// `None` when any of them cannot be read, which refuses it.
    fn review_inputs(
        &self,
        typed_data: &JsValue,
        network: Option<NetworkId>,
    ) -> Option<ReviewInputs> {
        let network = network.filter(|_| is_outside_execution(typed_data))?;
        let tracked = trusted_tokens(&self.tokens, network);
        let trusted = self.wallet.trusted_recipients(network).ok()?;
        let token_contracts = known_token_addresses(network)
            .into_iter()
            .chain(tracked.iter().filter_map(|t| felt::parse(&t.address)))
            .collect();
        let standing = with_allowance_log(self.wallet.data_dir(), |log| log.list()).ok()?;
        Some(ReviewInputs {
            network,
            tokens: token_infos(&tracked),
            trusted,
            token_contracts,
            standing,
        })
    }
}

/// The record's message hash for `typed_data`, bound to the record's network, with an outside
/// execution's review at `stage`. The document's gate and chain binding refuse as
/// INVALID_REQUEST_PAYLOAD; an outside execution without review inputs for the record's network
/// (one unreadable, or the active network moved since they were read) is refused, never signed
/// unreviewed.
fn reviewed_hash(
    record: &StoredAccount,
    typed_data: &JsValue,
    inputs: Option<&ReviewInputs>,
    stage: Stage,
) -> Result<(Felt, Option<OutsideExecutionReview>), WalletRpcError> {
    let checked = CheckedTypedData::check(typed_data, &DAPP_POLICY.typed_data)?;
    let address = felt::parse_non_zero(&record.address).ok_or_else(refused)?;
    let network = record.network();
    let hash = checked.message_hash_for_chain(&address, &network.chain_id())?;
    let review = match inputs.filter(|inputs| inputs.network == network) {
        Some(inputs) => {
            review_outside_execution(&checked, network, &address, &inputs.context(), stage)?
        }
        None if is_outside_execution(checked.value()) => return Err(refused()),
        None => None,
    };
    Ok((hash, review))
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
        let network = self
            .wallet
            .active_account()
            .ok()
            .flatten()
            .map(|account| account.network);
        let inputs = self.review_inputs(typed_data, network);
        // The shown network answers nothing here: only an unlocked session signs.
        let card = self.with_site(origin, NetworkId::Mainnet, false, |site, session| {
            Ok(admit(
                site,
                session.map(|session| &session.payload),
                typed_data,
                inputs.as_ref(),
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
    inputs: Option<&ReviewInputs>,
) -> Result<SignCard, WalletRpcError> {
    let record = granted_signer(site, payload)?;
    let (_, review) = reviewed_hash(record, typed_data, inputs, Stage::Queue)?;
    // The review's first line is the summary; the rest are the calls'.
    let (call_summaries, flags) = review.map_or_else(Default::default, |review| {
        (
            review.summaries.into_iter().skip(1).collect(),
            review.flags.iter().map(RiskFlag::from).collect(),
        )
    });
    Ok(SignCard {
        account_id: record.id.clone(),
        address: record.address.clone(),
        network: record.network(),
        summary: describe_typed_data(typed_data),
        typed_data: typed_data.clone(),
        call_summaries,
        flags,
    })
}

/// The approved card's signature array, if every check still holds.
fn sign(
    site: &Site<'_>,
    payload: Option<&VaultPayload>,
    card: &SignCard,
    reviewed_account_id: &str,
    inputs: Option<&ReviewInputs>,
) -> Result<JsValue, WalletRpcError> {
    if site.blocked || reviewed_account_id != card.account_id {
        return Err(refused());
    }
    let record = granted_signer(site, payload)?;
    let payload = payload.ok_or_else(refused)?;
    if record.id != card.account_id {
        return Err(refused());
    }
    let (hash, _) = reviewed_hash(record, &card.typed_data, inputs, Stage::Sign)?;
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
        // Read fresh, so a contact or payment made since queueing is checked against.
        let inputs = self.review_inputs(&card.typed_data, Some(card.network));
        // Every check and the signature run under one hold of the session: no lock, switch or
        // revocation can land between them.
        let signed = self.with_site(&origin, card.network, true, |site, session| {
            let payload = session.map(|session| &session.payload);
            Ok(sign(
                site,
                payload,
                &card,
                &reviewed_account_id,
                inputs.as_ref(),
            ))
        });
        let reply = match signed.unwrap_or_else(|_| Err(refused())) {
            Ok(json) => result(&json),
            Err(refusal) => error(refusal),
        };
        settled(waiters, |_| reply.clone())
    }
}
