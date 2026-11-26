//! Deleting one stored intent (`removeIntent` in `multisig-account-ops.ts`, the intent card's
//! "Delete"). The removal is recorded in `coordination.json` first, so a relay serving the
//! intent's package again cannot bring it back on the next sync.

use crate::multisig_guard::OperationClaim;
use crate::multisig_tx_ops::refused;
use crate::wallet::WalletCore;
use crate::{coordination, multisig_intents, CoreError};

/// `readRemoveIntentInput`'s longest id.
const MAX_INTENT_ID_LENGTH: usize = 256;

#[uniffi::export]
impl WalletCore {
    /// Removes the stored intent `intent_id` and keeps it out of later coordination syncs. Any
    /// state may be removed, as in the extension. Unlike the extension, an intent whose
    /// submission is under way is refused: the submission stores the intent again when it
    /// records its broadcast, which would bring a removed intent back.
    pub fn remove_multisig_intent(&self, intent_id: String) -> Result<(), CoreError> {
        if intent_id.is_empty() || intent_id.len() > MAX_INTENT_ID_LENGTH {
            return Err(refused("invalid Argent multisig intent removal request"));
        }
        self.require_unlocked()?;
        let _claim = OperationClaim::submit(self.data_dir(), &intent_id)
            .ok_or_else(|| refused("Argent multisig intent is being submitted"))?;
        let intent = multisig_intents::get(self.data_dir(), &intent_id)?
            .ok_or_else(|| refused("Argent multisig intent not found"))?;
        // Recorded first: a record for an intent still stored changes nothing, while a removal
        // with no record lets a sync bring it back.
        coordination::update(self.data_dir(), |file| {
            coordination::record_removed_intent(file, &intent.account_id, &intent.id)
        })?;
        self.require_unlocked()?;
        if !multisig_intents::remove(self.data_dir(), &intent_id)? {
            return Err(refused("Argent multisig intent not found"));
        }
        Ok(())
    }
}
