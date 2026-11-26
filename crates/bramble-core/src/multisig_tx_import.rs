//! Importing a co-signer's package past its parse (`importIntent`), shared by
//! `import_multisig_intent` and a coordination sync, with its refusal kept typed so the sync can
//! tell a final refusal from a passing one.

use wallet_core::send::call_decode::Known;
use wallet_core::send::multisig_import::{import_intent, ImportContext, ImportError};
use wallet_core::send::multisig_intent::{IntentPackage, SigningIntent};

use crate::allowances::{token_infos, trusted_tokens};
use crate::multisig_rpc::MultisigStateRead;
use crate::multisig_tx_ops::{class, refused, summary_length};
use crate::networks::NetworkId;
use crate::portfolio::tokens::TokenPreferences;
use crate::wallet::WalletCore;
use crate::{multisig_intents, multisig_store, CoreError};

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
