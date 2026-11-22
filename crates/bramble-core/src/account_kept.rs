//! The accounts a vault holds that mobile keeps unused (`vault_wire::Kept`), listed so the host
//! can show them as not usable on this device rather than as missing. Public data only: no key,
//! and nothing here makes a kept record selectable or able to sign.

use crate::account_names::short_label;
use crate::vault_rules::{is_ledger, seed_id};
use crate::vault_wire::Account;
use crate::wallet::WalletCore;
use crate::{CoreError, NetworkId};

/// Why mobile does not operate an account the vault holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum UnusableReason {
    /// A Ledger account: mobile has no Ledger support yet.
    Ledger,
    /// Derived from a phrase imported beside the wallet's own.
    ImportedPhrase,
    /// On a network mobile does not have (the extension's devnet).
    OtherNetwork,
    /// A record mobile cannot re-derive from a key it holds (an address picked at import, a
    /// class or index mobile does not derive, a key that does not match), or one past the
    /// number of accounts mobile operates.
    NotDerivedHere,
}

/// One account kept unused.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct UnusableAccount {
    pub id: String,
    /// The vault's network id (`SN_MAIN`, `SN_SEPOLIA`, `SN_DEVNET`, ...).
    pub network_id: String,
    pub address: String,
    /// `accountLabel(address)`, 6…4.
    pub short_address: String,
    pub reason: UnusableReason,
}

fn reason(account: &Account) -> UnusableReason {
    if is_ledger(account) {
        UnusableReason::Ledger
    } else if seed_id(account).is_some() {
        UnusableReason::ImportedPhrase
    } else if NetworkId::from_catalog_id(&account.network_id).is_none() {
        UnusableReason::OtherNetwork
    } else {
        UnusableReason::NotDerivedHere
    }
}

#[uniffi::export]
impl WalletCore {
    /// Every account the vault holds that mobile keeps unused, in the vault's order. Passive.
    pub fn unusable_accounts(&self) -> Result<Vec<UnusableAccount>, CoreError> {
        self.with_session(false, |session| {
            Ok(session
                .payload
                .kept
                .accounts
                .iter()
                .map(|(_, account)| UnusableAccount {
                    id: account.id.clone(),
                    network_id: account.network_id.clone(),
                    address: account.address.clone(),
                    short_address: short_label(&account.address),
                    reason: reason(account),
                })
                .collect())
        })
    }
}
