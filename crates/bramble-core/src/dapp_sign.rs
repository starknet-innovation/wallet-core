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
//! `signing_record` and `sign_for` under the same session hold as every check before it. A
//! Ledger account's hash is computed under that hold too and then signed on its device
//! (`dapp_sign_device.rs`). Every refusal but the document's is UNKNOWN_ERROR (163), as the
//! extension's strings map.
//!
//! Accounts it signs for: any whose key the vault holds (phrase-derived or imported), and a
//! Ledger account (`accountHashSigner`). OpenZeppelin signs at once; Ready (Argent) and Braavos
//! sign only once the live account accepts the signature (`dapp_sign_read.rs`,
//! `requireLiveAuthorizableClass`), whatever class is stored. Refused: a multisig, which is never
//! a vault record and so never the granted account here.

use std::sync::atomic::Ordering;

use starknet_wallet_core::allowances::record::AllowanceRecord;
use starknet_wallet_core::catalog::TokenInfo;
use starknet_wallet_core::dapp::{
    describe_typed_data, review_outside_execution, sign_rows, CheckedTypedData,
    OutsideExecutionReview, Site, Stage, WalletRpcError,
};
use starknet_wallet_core::js::json::JsValue;
use starknet_wallet_core::send::call_decode::Known;
use starknet_wallet_core::send::review_flags::ReviewContext;
use starknet_wallet_core::Felt;

use crate::account_catalog::AccountFamily;
use crate::account_store::{KeySource, StoredAccount, VaultPayload};
use crate::allowances::{token_infos, trusted_tokens, with_allowance_log};
use crate::dapp::DAPP_POLICY;
use crate::dapp_session::{
    error, result, settled, Card, DappCard, DappReply, DappSession, Shape, Waiter,
};
use crate::dapp_sign_device::DeviceStart;
use crate::dapp_sign_read::DappSignStep;
use crate::felt;
use crate::networks::NetworkId;
use crate::send::types::RiskFlag;
use crate::send::wallet_ext::known_token_addresses;
use crate::wallet_accounts::{sign_for, signing_record};

/// One decoded row of a sign card: the dotted path a value sits at and the text the card prints
/// for it (the extension's `signValueText`). Both are display-safe: names outside the SNIP-12
/// grammar are sketched in `«»`, and bidi, zero-width and control characters spelled `\uXXXX`.
/// `signed` is false for a value the site sent that the hash does not cover, which the extension
/// shows apart, as "shown by the site but NOT signed", capped at 48 rows.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SignRow {
    pub key: String,
    pub value: String,
    pub signed: bool,
}

/// A block's signed rows in the order the card shows them, then its unsigned ones.
fn sign_rows(block: sign_rows::SignBlock) -> Vec<SignRow> {
    let tagged = |rows: Vec<sign_rows::SignRow>, signed| {
        rows.into_iter().map(move |row| SignRow {
            key: row.key,
            value: row.value,
            signed,
        })
    };
    tagged(block.signed, true)
        .chain(tagged(block.unsigned, false))
        .collect()
}

/// A typed-data card: the account it was reviewed for and the document as the page sent it.
pub(crate) struct SignCard {
    pub(crate) account_id: String,
    address: String,
    pub(crate) network: NetworkId,
    summary: String,
    typed_data: JsValue,
    domain: Vec<SignRow>,
    message: Vec<SignRow>,
    outside_execution: bool,
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
            domain: self.domain.clone(),
            message: self.message.clone(),
            outside_execution: self.outside_execution,
            call_summaries: self.call_summaries.clone(),
            flags: self.flags.clone(),
        }
    }
}

pub(crate) fn refused() -> WalletRpcError {
    WalletRpcError::unknown_error()
}

/// Whether this wallet signs typed data for `record`: a key the vault holds, or a Ledger's. Every
/// family but OpenZeppelin also needs the live account read first (`sign`'s `authorized`).
fn signs_typed_data(record: &StoredAccount) -> bool {
    matches!(
        record.key,
        KeySource::Phrase(_) | KeySource::Imported | KeySource::Ledger(_)
    )
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
    pub(crate) fn context(&self) -> ReviewContext<'_> {
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
        self.review_inputs_for(network.filter(|_| is_outside_execution(typed_data))?)
    }

    /// What the invoke route's review reads on `network`; `None` when any of it cannot be read.
    pub(crate) fn review_inputs_for(&self, network: NetworkId) -> Option<ReviewInputs> {
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

/// The record an approved card still signs for: the site not blocked, the card reviewed for it,
/// and it still the granted active account of a kind this wallet signs typed data for.
pub(crate) fn still_granted<'a>(
    site: &Site<'_>,
    payload: Option<&'a VaultPayload>,
    card: &SignCard,
    reviewed_account_id: &str,
) -> Result<&'a StoredAccount, WalletRpcError> {
    if site.blocked || reviewed_account_id != card.account_id {
        return Err(refused());
    }
    let record = granted_signer(site, payload)?;
    (record.id == card.account_id)
        .then_some(record)
        .ok_or_else(refused)
}

/// `serializeAccountSignature` as the page reads it (`signaturePayload`): OpenZeppelin and
/// Braavos `[r, s]`, Ready `["1", "0", public_key, r, s]`, each felt canonical and non-zero.
pub(crate) fn signature_json(
    record: &StoredAccount,
    r: &str,
    s: &str,
) -> Result<JsValue, WalletRpcError> {
    let canonical = |value: &str| {
        felt::parse_non_zero(value)
            .map(|felt| JsValue::String(felt::canonical(&felt)))
            .ok_or_else(refused)
    };
    let (r, s) = (canonical(r)?, canonical(s)?);
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
            Ok(card) => self.queue(
                origin,
                Card::Sign(Box::new(card)),
                request_id,
                Shape::Signature,
            ),
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
    // A value the card could not place withholds Sign in the extension; here it is never queued.
    let rows = CheckedTypedData::check(typed_data, &DAPP_POLICY.typed_data)?
        .sign_rows()
        .ok_or_else(WalletRpcError::invalid_request_payload)?;
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
        domain: sign_rows(rows.domain),
        message: sign_rows(rows.message),
        outside_execution: is_outside_execution(typed_data),
        call_summaries,
        flags,
    })
}

/// What an approved card's checks end in: the signature array, or a Ledger's device to sign the
/// hash on.
enum Signed {
    Json(JsValue),
    Device(DeviceStart),
}

/// The approved card's signature array (or a Ledger's hash to sign), if every check still holds.
fn sign(
    site: &Site<'_>,
    payload: Option<&VaultPayload>,
    card: &SignCard,
    reviewed_account_id: &str,
    inputs: Option<&ReviewInputs>,
    authorized: bool,
) -> Result<Signed, WalletRpcError> {
    let record = still_granted(site, payload, card, reviewed_account_id)?;
    // Only OpenZeppelin's signature does not depend on the class the account runs.
    if !authorized && record.family() != AccountFamily::OpenZeppelin {
        return Err(refused());
    }
    let payload = payload.ok_or_else(refused)?;
    let (hash, _) = reviewed_hash(record, &card.typed_data, inputs, Stage::Sign)?;
    if let Some(device) = crate::ledger::record_device(record) {
        let public_key = felt::parse(&record.public_key).ok_or_else(refused)?;
        return Ok(Signed::Device(DeviceStart {
            device,
            hash,
            public_key,
        }));
    }
    let signature = sign_for(payload, record, &felt::canonical(&hash)).map_err(|_| refused())?;
    signature_json(record, &signature.r, &signature.s).map(Signed::Json)
}

/// An approved typed-data card: its site, the card, the requests waiting on it, and the account
/// the person reviewed it for.
pub(crate) struct Approved {
    pub(crate) origin: String,
    pub(crate) card: Box<SignCard>,
    pub(crate) waiters: Vec<Waiter>,
    pub(crate) reviewed_account_id: String,
}

impl Approved {
    /// Answers every request waiting on the card with `reply`.
    pub(crate) fn answer(self, reply: Result<JsValue, WalletRpcError>) -> DappSignStep {
        let reply = match reply {
            Ok(json) => result(&json),
            Err(refusal) => error(refusal),
        };
        DappSignStep::Settled {
            settled: settled(self.waiters, |_| reply.clone()),
        }
    }
}

impl DappSession {
    /// Signs an approved card and answers its requests, if every check still holds; a Ledger
    /// account's hash goes to its device instead. `authorized`: the live account read
    /// (`dapp_sign_read.rs`) accepted the signature.
    pub(crate) fn finish_sign(
        &self,
        approval_id: String,
        approved: Approved,
        authorized: bool,
    ) -> DappSignStep {
        let card = &approved.card;
        // Read fresh, so a contact or payment made since queueing is checked against.
        let inputs = self.review_inputs(&card.typed_data, Some(card.network));
        // Every check and the signature (or the hash a device signs) run under one hold of the
        // session: no lock, switch or revocation can land between them.
        let signed = self.with_site(&approved.origin, card.network, true, |site, session| {
            let generation = self.wallet.lock_epoch.load(Ordering::SeqCst);
            let payload = session.map(|session| &session.payload);
            let signed = sign(
                site,
                payload,
                card,
                &approved.reviewed_account_id,
                inputs.as_ref(),
                authorized,
            );
            Ok(signed.map(|signed| (signed, generation)))
        });
        match signed.unwrap_or_else(|_| Err(refused())) {
            Ok((Signed::Json(json), _)) => approved.answer(Ok(json)),
            Ok((Signed::Device(start), generation)) => {
                self.start_device(approval_id, approved, start, generation)
            }
            Err(refusal) => approved.answer(Err(refusal)),
        }
    }
}
