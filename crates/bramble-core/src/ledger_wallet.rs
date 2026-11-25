//! Ledger accounts a person adds (P8): the extension's `createLedgerVault` (a new wallet with no
//! phrase whose first account is a Ledger account) and `addLedgerAccount` (one more on any
//! wallet), in `vault/src/service/service-creation.ts` over `deriveLedgerAccount`.
//!
//! The host reads the account's public key from the device at the scheme's path for the index
//! (`ledger::public_key_request`) and hands it in. The core places the address from it, as the
//! extension does from the key its panel read; nothing is signed and no key is held. A key that
//! is not the device's places an address the device cannot sign for: every send checks the key
//! at the path before the person confirms (`wrong-device`) and verifies the signature after.
//! A Braavos request is refused. That is stricter than the extension, whose `ledgerStoreRefusal`
//! admits a Braavos Ledger account at Ready's scheme once it is proven deployed or funded on
//! chain (`ledger-activity.ts`); that read is not ported, so mobile refuses rather than add one
//! unproven. A vault's existing Braavos Ledger record is still operated (part 4). No origin is
//! recorded, as the extension writes none.

use wallet_core::Zeroizing;

use crate::account_catalog::AccountFamily;
use crate::account_store::{push_active, record_at, KeySource, StoredAccount, VaultPayload};
use crate::ledger::LedgerScheme;
use crate::networks::NetworkId;
use crate::wallet::WalletCore;
use crate::wallet_accounts::{render, AccountRecord};
use crate::{felt, vault, CoreError};

/// One Ledger account as the device reported it.
#[derive(Debug, Clone, uniffi::Record)]
pub struct LedgerAccountRequest {
    pub network: NetworkId,
    /// OpenZeppelin or Argent; Braavos is refused (see the module).
    pub family: AccountFamily,
    /// The scheme the key was read at.
    pub scheme: LedgerScheme,
    /// The index the key was read at (below 2^31).
    pub account_index: u32,
    /// The device's public key at that path (`getPubKey`), as hex.
    pub public_key_x: String,
}

fn invalid(reason: &str) -> CoreError {
    CoreError::InvalidInput {
        reason: reason.into(),
    }
}

/// `deriveLedgerAccount`: the record the request places, at Bramble's class for its family.
fn record(request: &LedgerAccountRequest) -> Result<StoredAccount, CoreError> {
    if request.family == AccountFamily::Braavos {
        return Err(invalid("a Ledger account cannot be a Braavos account here"));
    }
    request
        .scheme
        .device(request.account_index)
        .ok_or_else(|| invalid("Ledger account index out of range"))?;
    let public_key = felt::parse_non_zero(request.public_key_x.trim())
        .ok_or_else(|| invalid("invalid public key"))?;
    record_at(
        request.family,
        KeySource::Ledger(request.scheme),
        request.account_index,
        None,
        None,
        &public_key,
        request.network,
    )
}

#[uniffi::export]
impl WalletCore {
    /// Creates a wallet with no recovery phrase whose first account is the Ledger account
    /// `request` names, sealed under `password`, and leaves it unlocked. Refuses when a wallet
    /// exists (`WalletExists`). Such a wallet signs only on its device: everything that needs a
    /// phrase (another phrase account, discovery, revealing a phrase, Nostr coordination) is
    /// refused.
    pub fn create_ledger_wallet(
        &self,
        password: String,
        request: LedgerAccountRequest,
    ) -> Result<AccountRecord, CoreError> {
        let password = Zeroizing::new(password);
        vault::check_password(&password)?;
        let record = record(&request)?;
        let id = record.id.clone();
        let mut payload = VaultPayload::opened(None, Vec::new(), None);
        push_active(&mut payload, record, None, None)?;
        self.create_vault(&password, payload)?;
        self.with_session(false, |session| {
            let payload = &session.payload;
            payload
                .record(&id)
                .map(|record| render(payload, record))
                .ok_or_else(|| invalid("unknown account"))
        })
    }

    /// Adds the Ledger account `request` names to the unlocked wallet and makes it active.
    /// Refuses an account the wallet already holds ("account already exists") and a wallet still
    /// finding its accounts. Renews the idle window.
    pub fn add_ledger_account(
        &self,
        request: LedgerAccountRequest,
    ) -> Result<AccountRecord, CoreError> {
        let record = record(&request)?;
        let id = record.id.clone();
        self.with_session(true, |session| {
            if session.payload.recovery_pending() {
                return Err(invalid("finish finding your accounts first"));
            }
            self.commit(session, |payload| push_active(payload, record, None, None))?;
            let payload = &session.payload;
            payload
                .record(&id)
                .map(|record| render(payload, record))
                .ok_or_else(|| invalid("unknown account"))
        })
    }

    /// Whether the unlocked wallet has a recovery phrase: false for a Ledger-only wallet, whose
    /// phrase screens the host hides. Passive.
    pub fn has_recovery_phrase(&self) -> Result<bool, CoreError> {
        self.with_session(false, |session| Ok(session.payload.mnemonic.is_some()))
    }
}
