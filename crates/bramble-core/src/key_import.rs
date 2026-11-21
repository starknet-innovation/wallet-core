//! Raw private-key import (`docs/PLAN.md` P3): the extension's `wallet.importAccount`
//! (`handleImportAccount` in `background/routing/account-creation-handlers.ts`, over
//! `importRawKeyAccount` in `vault/src/service/service-creation.ts`). It is the one export through
//! which a private key enters mobile's build, and it goes one way: the key is sealed in the vault
//! and held in the session (`VaultPayload::imported_keys`), and only the account's public record
//! comes back. Nothing on the mobile surface returns it.
//!
//! Only what the extension does for an OpenZeppelin import is here: Bramble's class, no salt,
//! placed as `deriveImportedAccount` places it, made active. Its Argent import (releases picked
//! after discovery, or a pasted address, re-proved on chain), a deployment salt, and a wallet made
//! from a key alone (`createKeyVault`) are not.

use wallet_core::Zeroizing;

use crate::account_catalog::AccountFamily;
use crate::account_store::{push_active, record_at, KeySource};
use crate::networks::NetworkId;
use crate::vault_provenance::Origin;
use crate::wallet::WalletCore;
use crate::wallet_accounts::{render, AccountRecord};
use crate::{felt, CoreError};

fn invalid(reason: &str) -> CoreError {
    CoreError::InvalidInput {
        reason: reason.into(),
    }
}

#[uniffi::export]
impl WalletCore {
    /// Imports a pasted Stark private key into the unlocked wallet as an OpenZeppelin account on
    /// `network` (Bramble's class, no salt), seals the key in the vault and makes the account
    /// active. Returns the account's public record, with `imported` set; the key is never
    /// returned, by this or any other function.
    ///
    /// The key is read as the extension's import form reads it (`0x`, then hex digits in either
    /// case; surrounding whitespace is trimmed) and must be a usable signing scalar, `1 <= k < n`.
    /// Anything else is `InvalidInput("invalid private key")`, which says nothing about the text.
    /// Refuses an account the wallet already holds ("account already exists": the key of one of
    /// its phrase accounts, or a key imported before), a wallet still finding its accounts, and a
    /// locked wallet (`Locked`). The phrase does not recover this account; the user keeps the key.
    /// Renews the idle window.
    pub fn import_private_key(
        &self,
        network: NetworkId,
        private_key: String,
    ) -> Result<AccountRecord, CoreError> {
        // The bytes UniFFI handed over are wiped as `text` drops; the host's own string is not
        // reachable from here.
        let text = Zeroizing::new(private_key);
        let key =
            felt::parse_private_key_input(&text).ok_or_else(|| invalid("invalid private key"))?;
        drop(text);
        let key = Box::new(key);
        self.with_session(true, |session| {
            if session.payload.recovery_pending() {
                return Err(invalid("finish finding your accounts first"));
            }
            let public_key = felt::parse_non_zero(&wallet_core::stark_public_key(&key)?).ok_or(
                CoreError::Kms {
                    operation: "public key".into(),
                },
            )?;
            let record = record_at(
                AccountFamily::OpenZeppelin,
                KeySource::Imported,
                0,
                None,
                None,
                &public_key,
                network,
            )?;
            let id = record.id.clone();
            let imported = Some(Origin::Imported);
            self.commit(session, |payload| {
                push_active(payload, record, Some(key), imported)
            })?;
            let payload = &session.payload;
            payload
                .record(&id)
                .map(|record| render(payload, record))
                .ok_or_else(|| invalid("unknown account"))
        })
    }
}
