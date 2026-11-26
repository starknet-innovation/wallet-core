//! The `WalletCore` entry points for Argent multisig transactions (P1), as the extension's
//! `multisig-tx-ops.ts` and `multisig-deploy-ops.ts`: deploy, propose (a signer change is a
//! proposal of `argent_multisig_config_change_call`), co-sign, submit and import. Every input is
//! untrusted. The host drives every read (`MultisigAuthorityRead`, `MultisigStateRead`, the
//! plans) and the core judges it; a co-signer signs with its own sealed key inside the core, by
//! vault record, and no key crosses the FFI.
//!
//! Before an intent is signed, submitted or imported the chain must still hold its snapshot and
//! nonce (`multisig_import::require_current`); a local operation that finds either moved marks
//! the intent stale, as `currentArgentIntentContext` does. An import's summary is always this
//! wallet's own review of the calls (`import_intent`).

use std::sync::Arc;

use wallet_core::accounts::multisig::{signer_guid, ArgentMultisigClass};
use wallet_core::accounts::multisig_change::config_change_call;
use wallet_core::accounts::multisig_records::{self, Record};
use wallet_core::send::call_decode::Known;
use wallet_core::send::calls::{self, SendAmountError};
use wallet_core::send::multisig_deploy::{deployment_plan, MultisigDeployRefusal};
use wallet_core::send::multisig_import::{held_config, import_intent, ImportContext, ImportError};
use wallet_core::send::multisig_intent::{
    ConfigSnapshot, IntentPackage, IntentState, SigningIntent,
};
use wallet_core::send::multisig_invoke::{normalize_calls, MultisigSignature};
use wallet_core::send::multisig_invoke_plan::{ProposePlan, SubmitPlan};
use wallet_core::send::multisig_review::review_calls;
use wallet_core::Felt;

use crate::account_store::{StoredAccount, VaultPayload};
use crate::allowances::{token_infos, trusted_tokens};
use crate::argent_multisig::ArgentMultisigConfig;
use crate::multisig_guard::{settle, OperationClaim};
use crate::multisig_rpc::{MultisigAuthorityRead, MultisigStateRead};
use crate::multisig_tx::{
    intent_view, MultisigCall, MultisigCallReview, MultisigIntent, MultisigProposal,
    MultisigProposalRequest, MultisigSubmission,
};
use crate::networks::NetworkId;
use crate::portfolio::model::TokenConfig;
use crate::portfolio::tokens::TokenPreferences;
use crate::send::plan::TransferPlan;
use crate::send::types::SendError;
use crate::send::wallet_ext::Use;
use crate::wallet::WalletCore;
use crate::wallet_accounts::{sign_for, signing_record};
use crate::{felt, multisig_constants, multisig_intents, multisig_store, CoreError};
use wallet_core::allowances::retention::Ranking;

fn refused(reason: &str) -> CoreError {
    CoreError::InvalidInput {
        reason: reason.into(),
    }
}

const NOT_FOUND: &str = "Argent multisig not found";
const INTENT_NOT_FOUND: &str = "intent not found";
const INVALID_PACKAGE: &str = "invalid Argent multisig intent package";

fn class() -> &'static ArgentMultisigClass {
    multisig_constants::argent_multisig_class()
}

fn summary_length() -> usize {
    multisig_constants::get().limits.summary_length
}

fn guid_of(public_key: &str) -> Option<Felt> {
    felt::parse(public_key).and_then(|key| signer_guid(&key).ok())
}

/// The vault record `signer_account_id`, refused unless it may co-sign `intent`: on the
/// multisig's network, its key a member of the snapshot that has not signed yet. Returns it with
/// its signer GUID.
pub(crate) fn cosigner<'a>(
    payload: &'a VaultPayload,
    intent: &SigningIntent,
    record: &Record,
    signer_account_id: &str,
) -> Result<(&'a StoredAccount, Felt), CoreError> {
    let signer = signing_record(payload, signer_account_id)
        .ok()
        .filter(|signer| signer.network().catalog_id() == record.network_id)
        .ok_or_else(|| refused("selected signer account is unavailable on this network"))?;
    let not_member = || refused("selected account is not a current Argent multisig signer");
    let guid = guid_of(&signer.public_key).ok_or_else(not_member)?;
    if !intent
        .config_snapshot
        .signer_guids
        .contains(&felt::canonical(&guid))
    {
        return Err(not_member());
    }
    if intent
        .signatures
        .iter()
        .any(|signed| guid_of(&signed.public_key) == Some(guid))
    {
        return Err(refused(
            "this Argent multisig signer already signed the intent",
        ));
    }
    Ok((signer, guid))
}

/// The one call `request` names, built here from `record` (the multisig that sends it).
fn proposal_call(
    record: &Record,
    trusted: &[TokenConfig],
    request: &MultisigProposalRequest,
) -> Result<MultisigCall, CoreError> {
    let (token_id, recipient, amount) = match request {
        MultisigProposalRequest::ChangeSigners { change } => {
            return config_change_call(&record.address, change, class())
                .map_err(|error| refused(error.message()));
        }
        MultisigProposalRequest::Transfer {
            token_id,
            recipient,
            amount,
        } => (token_id, recipient, amount),
    };
    let token = trusted
        .iter()
        .find(|token| token.id == *token_id)
        .and_then(|token| Some((felt::parse(&token.address)?, token.decimals)))
        .ok_or_else(|| refused("unknown token"))?;
    let recipient = calls::validate_recipient(recipient)
        .filter(|recipient| *recipient != Felt::ZERO)
        .ok_or_else(|| refused("Invalid transfer address"))?;
    let amount = calls::parse_token_amount(amount, token.1).map_err(|error| {
        refused(match error {
            SendAmountError::TooManyDecimals => "Too many decimal places",
            SendAmountError::NotPositive => "Transfer amount must be greater than zero",
            SendAmountError::Invalid | SendAmountError::TooLarge => "Invalid transfer amount",
        })
    })?;
    let calldata = [recipient, Felt::from(amount.low), Felt::from(amount.high)];
    Ok(MultisigCall {
        calldata: calldata.iter().map(felt::canonical).collect(),
        contract_address: felt::canonical(&token.0),
        entrypoint: "transfer".into(),
    })
}

impl WalletCore {
    pub(crate) fn multisig_record(&self, id: &str) -> Result<Record, CoreError> {
        multisig_store::load(self.data_dir())?
            .records
            .into_iter()
            .find(|record| record.id == id)
            .ok_or_else(|| refused(NOT_FOUND))
    }

    /// Every address the person recognises on `network` (`trusted_recipients`), for the review.
    fn review_trusted(&self, network: NetworkId) -> Result<Vec<Felt>, CoreError> {
        self.trusted_recipients(network)
            .map_err(|error| match error {
                SendError::Locked => CoreError::Locked,
                _ => CoreError::Storage {
                    reason: "trusted addresses unreadable".into(),
                },
            })
    }

    pub(crate) fn stored_intent(&self, id: &str) -> Result<SigningIntent, CoreError> {
        multisig_intents::get(self.data_dir(), id)?.ok_or_else(|| refused(INTENT_NOT_FOUND))
    }

    /// The collecting intent `intent_id` and its multisig, once `state` shows the chain still
    /// holds the intent's snapshot and nonce (`signIntent`'s `resolveLocalSigning`).
    pub(crate) fn collecting_intent(
        &self,
        intent_id: &str,
        state: &MultisigStateRead,
    ) -> Result<(SigningIntent, Record), CoreError> {
        let intent = self.stored_intent(intent_id)?;
        if intent.state != IntentState::Collecting {
            return Err(refused(
                "Argent multisig intent is not collecting signatures",
            ));
        }
        let record = self.multisig_record(&intent.account_id)?;
        self.require_chain(&intent, &record, state)?;
        Ok((intent, record))
    }

    /// Claims `guid`'s signature of `intent_id` for the live session (`claimOperation("sign",
    /// ..)`), so one signer never signs one intent twice at once. Call inside the session.
    pub(crate) fn claim_signing(
        &self,
        intent_id: &str,
        guid: &Felt,
    ) -> Result<OperationClaim, CoreError> {
        let generation = self.lock_epoch.load(std::sync::atomic::Ordering::SeqCst);
        OperationClaim::sign(self.data_dir(), intent_id, guid, generation)
            .ok_or_else(|| refused("this signer already signed or is signing the intent"))
    }

    /// Stores `intent` with `signature` added in GUID order, ready at the threshold
    /// (`persistSignature`). The store validates the result, every signature over the hash
    /// recomputed from the prepared transaction.
    pub(crate) fn store_signature(
        &self,
        intent: SigningIntent,
        signature: MultisigSignature,
    ) -> Result<MultisigIntent, CoreError> {
        let mut signatures = intent.signatures.clone();
        signatures.push(signature);
        signatures.sort_by_cached_key(|signed| guid_of(&signed.public_key));
        let ready = signatures.len() == intent.config_snapshot.threshold as usize;
        let next = SigningIntent {
            signatures,
            state: if ready {
                IntentState::Ready
            } else {
                IntentState::Collecting
            },
            ..intent
        };
        self.require_unlocked()?;
        multisig_intents::upsert(self.data_dir(), &next)?;
        Ok(intent_view(&next))
    }
}

#[uniffi::export]
impl WalletCore {
    /// The signing intents stored for one multisig, newest first. A submitted intent whose
    /// activity row reached a terminal status is settled accepted or rejected first.
    pub fn multisig_intents(&self, multisig_id: String) -> Result<Vec<MultisigIntent>, CoreError> {
        self.require_unlocked()?;
        settle(self.data_dir(), &multisig_id)?;
        Ok(multisig_intents::load(self.data_dir())?
            .iter()
            .rev()
            .filter(|intent| intent.account_id == multisig_id)
            .map(intent_view)
            .collect())
    }

    /// Deploys a saved multisig setup (`save_multisig_setup`), signed by the vault record
    /// `deployer_account_id`, which must be on the multisig's network and a member of the
    /// reviewed constructor. The plan is the standalone deployment, recorded under the deployer.
    /// The host checks the class is declared on the network first (`ClassDeclaredRead`).
    pub fn prepare_multisig_deployment(
        self: Arc<Self>,
        multisig_id: String,
        deployer_account_id: String,
    ) -> Result<Arc<TransferPlan>, CoreError> {
        let record = self.multisig_record(&multisig_id)?;
        let network =
            NetworkId::from_catalog_id(&record.network_id).ok_or_else(|| refused(NOT_FOUND))?;
        let setup = record
            .deployment
            .as_ref()
            .ok_or_else(|| refused("Argent multisig deployment is unavailable"))?;
        let acting = self
            .plan_account_for(&deployer_account_id, Use::Sign)
            .ok()
            .filter(|acting| acting.network == network)
            .ok_or_else(|| refused("unknown account"))?;
        let config = ArgentMultisigConfig {
            threshold: setup.threshold,
            signer_public_keys: setup.signer_public_keys.clone(),
        };
        let plan = deployment_plan(
            network,
            &config,
            &setup.salt,
            class(),
            &acting.account.public_key,
            crate::SEND_POLICY,
        )
        .map_err(|refusal| {
            refused(match refusal {
                MultisigDeployRefusal::InvalidConfig => "Invalid Argent multisig deployment",
                MultisigDeployRefusal::DeployerNotMember => {
                    "selected signer is not a member of the reviewed constructor"
                }
            })
        })?;
        Ok(TransferPlan::with_plan(
            &self,
            acting,
            plan,
            Ranking::default(),
        ))
    }

    /// Proposes `request` from the saved multisig `read` describes (`prepareIntent`). `read` is
    /// a fresh authority read of it, driven to `Available`: the stored public keys must still be
    /// the chain's signers, and its threshold and GUIDs become the intent's snapshot. The call is
    /// built and reviewed here (`review()`), over the person's trusted tracked `tokens` on the
    /// multisig's network (a transfer's token is one of them), so nothing unsafe or poisoned is
    /// ever frozen. Before it is stored, every stale version of the same calls is looked up:
    /// one on chain refuses it (`AlreadySent`), and one at another nonce that may have sent them
    /// refuses it unless `confirm_resend` (`ResendUnconfirmed`, the extension's `confirmResend`).
    pub fn propose_multisig_transaction(
        self: Arc<Self>,
        read: Arc<MultisigAuthorityRead>,
        request: MultisigProposalRequest,
        tokens: Arc<TokenPreferences>,
        confirm_resend: bool,
    ) -> Result<Arc<MultisigProposal>, CoreError> {
        let unavailable = || refused(ImportError::AuthorityUnavailable.message());
        let network = read.network();
        let record = self.multisig_record(&multisig_store::record_id(network, &read.address())?)?;
        // Only an `Available` read holds an authority.
        let (threshold, guids) = read.authority().ok_or_else(unavailable)?;
        let snapshot = ConfigSnapshot::new(class(), threshold, &guids);
        if multisig_records::authority(&record, class()).is_none() {
            return Err(refused(
                "Argent multisig public signer configuration is unavailable",
            ));
        }
        let current = held_config(&record, &snapshot, class())
            .ok_or_else(|| refused("Argent multisig signer configuration changed on-chain"))?;
        let trusted = trusted_tokens(&tokens, network);
        let call = proposal_call(&record, &trusted, &request)?;
        let calls = normalize_calls(&[call]).map_err(|error| refused(&error.to_string()))?;
        let infos = token_infos(&trusted);
        let known = Known {
            tokens: &infos,
            nft_contracts: &[],
        };
        let review = review_calls(
            &record.address,
            &calls,
            Some(&current),
            class(),
            &known,
            &self.review_trusted(network)?,
            summary_length(),
        )
        .map_err(|error| refused(error.message()))?;
        let config = ArgentMultisigConfig {
            threshold: current.threshold,
            signer_public_keys: current.signer_public_keys,
        };
        let plan = ProposePlan::new(
            &record,
            &calls,
            &config,
            class(),
            crate::MULTISIG_FEE_OVERHEAD,
        )
        .map_err(|error| refused(&error.to_string()))?;
        let generation = self.session_generation().ok_or(CoreError::Locked)?;
        let call_review = MultisigCallReview {
            summary: review.summary,
            change: review.config.as_ref().map(|config| config.change.clone()),
            next_config: review.config.map(|config| ArgentMultisigConfig {
                threshold: config.next_config.threshold,
                signer_public_keys: config.next_config.signer_public_keys,
            }),
        };
        Ok(MultisigProposal::new(
            &self,
            generation,
            plan,
            record,
            snapshot,
            call_review,
            confirm_resend,
        ))
    }

    /// Co-signs a collecting intent with the vault record `signer_account_id` (`signIntent`):
    /// the record must be on the multisig's network, its key a member of the snapshot that has
    /// not signed yet, and `state` (`multisig_intent_read` of this intent, driven to its end) must
    /// show the snapshot and the prepared nonce, and no stale version of the calls on chain. The key
    /// signs the frozen hash inside the core; the intent is stored with the signature in GUID
    /// order, ready at the threshold. A Ledger record is refused here: it co-signs on its device
    /// (`sign_multisig_intent_on_device`).
    pub fn sign_multisig_intent(
        &self,
        intent_id: String,
        signer_account_id: String,
        state: Arc<MultisigStateRead>,
    ) -> Result<MultisigIntent, CoreError> {
        let (intent, record) = self.collecting_intent(&intent_id, &state)?;
        let (signature, _claim) = self.with_session(true, |session| {
            let (signer, guid) = cosigner(&session.payload, &intent, &record, &signer_account_id)?;
            let claim = self.claim_signing(&intent.id, &guid)?;
            let signature = sign_for(&session.payload, signer, &intent.prepared.transaction_hash)?;
            Ok((signature, claim))
        })?;
        self.store_signature(intent, MultisigSignature::from(signature))
    }

    /// Submits a ready (or fee-stale) intent with its threshold signature (`submitIntent`).
    /// `state` (`multisig_intent_read` of this intent) must show the snapshot and the prepared
    /// nonce; a nonce moved past it means it may have been sent, and its hash found on chain
    /// refuses it as already sent. An intent already broadcast is refused: its outcome is read
    /// from the chain, never sent again. One submission of an intent runs at a time.
    pub fn submit_multisig_intent(
        self: Arc<Self>,
        intent_id: String,
        state: Arc<MultisigStateRead>,
    ) -> Result<Arc<MultisigSubmission>, CoreError> {
        let intent = self.stored_intent(&intent_id)?;
        if intent.broadcast_transaction_hash.is_some() {
            return Err(refused("Argent multisig intent was already submitted"));
        }
        if !matches!(intent.state, IntentState::Ready | IntentState::Stale) {
            return Err(refused("Argent multisig intent is not ready"));
        }
        let record = self.multisig_record(&intent.account_id)?;
        self.require_chain(&intent, &record, &state)?;
        if intent.signatures.len() != intent.config_snapshot.threshold as usize {
            return Err(refused("Argent multisig threshold is not satisfied"));
        }
        let config = ArgentMultisigConfig {
            threshold: intent.config_snapshot.threshold,
            signer_public_keys: intent
                .signatures
                .iter()
                .map(|signed| signed.public_key.clone())
                .collect(),
        };
        let plan = SubmitPlan::new(&intent, &config, class(), summary_length())
            .map_err(|error| refused(&error.to_string()))?;
        let generation = self.session_generation().ok_or(CoreError::Locked)?;
        let claim = OperationClaim::submit(self.data_dir(), &intent.id)
            .ok_or_else(|| refused("Argent multisig submission claim exists"))?;
        Ok(MultisigSubmission::new(
            &self, generation, plan, intent, record, claim,
        ))
    }

    /// Imports a co-signer's package (`importIntent`) for a saved multisig: `state` is a fresh
    /// read of that multisig. The package's own lifecycle claims and summary are dropped; the
    /// summary is this wallet's review of the calls over its trusted tracked `tokens`, and a
    /// review refusal refuses the import. A
    /// stored intent under the same id is merged into, never replaced by another transaction.
    pub fn import_multisig_intent(
        &self,
        package: String,
        state: Arc<MultisigStateRead>,
        tokens: Arc<TokenPreferences>,
    ) -> Result<MultisigIntent, CoreError> {
        if package.len() > multisig_constants::get().nostr.max_application_data_bytes {
            return Err(refused(INVALID_PACKAGE));
        }
        let parsed = IntentPackage::parse(&package, class(), summary_length())
            .ok_or_else(|| refused(INVALID_PACKAGE))?;
        let next = self.import_package(&parsed, &state, &tokens)?;
        Ok(intent_view(&next))
    }
}

/// Why `import_package` refused: the shared import rule's refusal, kept typed so a coordination
/// sync can tell a final refusal from a passing one, or the wallet's own (lock, storage, the
/// intent store's "invalid", "conflict" and "store full").
pub(crate) enum ImportRefusal {
    Import(ImportError),
    Wallet(CoreError),
}

impl From<ImportRefusal> for CoreError {
    fn from(refusal: ImportRefusal) -> Self {
        match refusal {
            ImportRefusal::Import(error) => refused(error.message()),
            ImportRefusal::Wallet(error) => error,
        }
    }
}

impl From<CoreError> for ImportRefusal {
    fn from(error: CoreError) -> Self {
        Self::Wallet(error)
    }
}

impl WalletCore {
    /// `import_multisig_intent` past the parse, which a coordination sync shares: stores the
    /// merged intent and returns it.
    pub(crate) fn import_package(
        &self,
        parsed: &IntentPackage,
        state: &MultisigStateRead,
        tokens: &TokenPreferences,
    ) -> Result<SigningIntent, ImportRefusal> {
        // Only a record that is not there is `UnknownAccount` (final); an unreadable store is the
        // wallet's error, which a sync tries again.
        let record = multisig_store::load(self.data_dir())?
            .records
            .into_iter()
            .find(|record| record.id == parsed.intent.account_id)
            .ok_or(ImportRefusal::Import(ImportError::UnknownAccount))?;
        let network = NetworkId::from_catalog_id(&record.network_id)
            .ok_or(ImportRefusal::Import(ImportError::UnknownAccount))?;
        let (read, nonce) = state
            .finished()
            .ok_or(ImportRefusal::Import(ImportError::AuthorityUnavailable))?;
        // ponytail: `get` and the `upsert` below are not one critical section, so a local sign
        // landing between them makes this merge fail as "conflict", which a sync settles as
        // final. Serialise intent mutations (the extension's `runActiveStateMutation`) if that
        // race shows up; the local operations take no shared lock today.
        let stored = multisig_intents::get(self.data_dir(), &parsed.intent.id)?;
        let trusted = trusted_tokens(tokens, network);
        let infos = token_infos(&trusted);
        let known = Known {
            tokens: &infos,
            nft_contracts: &[],
        };
        let trusted = self.review_trusted(network)?;
        let wallet = ImportContext {
            record: &record,
            stored: stored.as_ref(),
            class: class(),
            known: &known,
            trusted: &trusted,
            summary_length: summary_length(),
        };
        let next = import_intent(parsed, &read, &nonce, &wallet).map_err(ImportRefusal::Import)?;
        self.require_unlocked()?;
        multisig_intents::upsert(self.data_dir(), &next)?;
        Ok(next)
    }
}
