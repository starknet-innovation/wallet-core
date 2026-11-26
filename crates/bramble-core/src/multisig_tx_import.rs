//! Importing a co-signer's package past its parse (`importIntent`), shared by
//! `import_multisig_intent` and a coordination sync, with its refusal kept typed so the sync can
//! tell a final refusal from a passing one; and importing one co-signer's pasted signature
//! (`importSignature`, `import_multisig_signature`).

use std::sync::Arc;

use wallet_core::accounts::multisig::signer_guid;
use wallet_core::send::call_decode::Known;
use wallet_core::send::multisig_import::{import_intent, ImportContext, ImportError};
use wallet_core::send::multisig_intent::{IntentPackage, SigningIntent};
use wallet_core::send::multisig_invoke::MultisigSignature;

use crate::allowances::{token_infos, trusted_tokens};
use crate::multisig_rpc::MultisigStateRead;
use crate::multisig_tx::MultisigIntent;
use crate::multisig_tx_ops::{class, refused, summary_length};
use crate::networks::NetworkId;
use crate::portfolio::tokens::TokenPreferences;
use crate::wallet::WalletCore;
use crate::{felt, multisig_intents, multisig_store, CoreError};

/// `readImportIntentSignatureInput`'s refusal: an empty id or a felt not canonical and non-zero.
const INVALID_SIGNATURE_IMPORT: &str = "invalid Argent multisig signature import";

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

#[uniffi::export]
impl WalletCore {
    /// Adds a co-signer's pasted signature to the collecting intent `intent_id`
    /// (`importSignature`), the alternative to a whole package. `public_key`, `r` and `s` are
    /// canonical non-zero felts. `state` (`multisig_intent_read` of this intent, driven to its
    /// end) must show the snapshot and the prepared nonce, as for a local sign. The signature must
    /// be a snapshot member's over the hash recomputed from the prepared transaction, by a signer
    /// that has not signed, below the threshold (`SigningIntent::with_signature`); the intent is
    /// stored with it in GUID order, ready at the threshold. Nothing here signs or holds a key.
    ///
    /// Unlike the extension, which takes no claim here, the import takes the signer's signing
    /// claim (`claim_signing`), so a paste for a signer is refused while that signer's local or
    /// Ledger signing (a live `MultisigDeviceSigning` included) is under way.
    pub fn import_multisig_signature(
        &self,
        intent_id: String,
        public_key: String,
        r: String,
        s: String,
        state: Arc<MultisigStateRead>,
    ) -> Result<MultisigIntent, CoreError> {
        let canonical = |value: &str| felt::parse_canonical_non_zero(value).is_some();
        if intent_id.is_empty() || ![&public_key, &r, &s].iter().all(|value| canonical(value)) {
            return Err(refused(INVALID_SIGNATURE_IMPORT));
        }
        let (intent, _) = self.collecting_intent(&intent_id, &state)?;
        let signature = MultisigSignature { public_key, r, s };
        // Refuse before claiming; the store checks the intent as it stands then again.
        intent
            .with_signature(signature.clone())
            .map_err(|error| refused(error.message()))?;
        let guid = felt::parse(&signature.public_key)
            .and_then(|key| signer_guid(&key).ok())
            .ok_or_else(|| refused(INVALID_SIGNATURE_IMPORT))?;
        self.require_unlocked()?;
        let _claim = self.claim_signing(&intent.id, &guid)?;
        self.store_signature(intent, signature)
    }
}
