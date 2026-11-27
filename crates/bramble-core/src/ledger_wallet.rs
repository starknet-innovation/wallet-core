//! Ledger accounts a person adds (P8): the extension's `createLedgerVault` (a new wallet with no
//! phrase whose first account is a Ledger account) and `addLedgerAccount` (one more on any
//! wallet), in `vault/src/service/service-creation.ts` over `deriveLedgerAccount`.
//!
//! The host reads the account's public key from the device at the scheme's path for the index
//! (`ledger::public_key_request`) and hands it in. The core places the address from it, as the
//! extension does from the key its panel read; nothing is signed and no key is held. A key that
//! is not the device's places an address the device cannot sign for: every send checks the key
//! at the path before the person confirms (`wrong-device`) and verifies the signature after.
//! Bramble never creates a Braavos account, so a Braavos request is admitted only at Ready's
//! scheme (`isLedgerApplicationForFamily`), at the class Braavos deploys (`ledgerDeployClassHash`),
//! and only once its address is proven in use: deployed, or holding a known token
//! (`ledgerStoreRefusal`, `ledger-activity.ts`). The host makes the reads
//! `ledger_account_activity_requests` names and hands the replies in; other families ignore
//! them, as the extension leaves them unchecked. No origin is recorded, as the extension writes
//! none.

use wallet_core::accounts::activity::{self, Activity};
use wallet_core::Zeroizing;

use crate::account_catalog::catalog;
use crate::account_catalog::AccountFamily;
use crate::account_store::{push_active, record_at, KeySource, StoredAccount, VaultPayload};
use crate::ledger::LedgerScheme;
use crate::networks::NetworkId;
use crate::portfolio::registry::known_tokens;
use crate::rpc_request::{RpcRequest, RpcResponse};
use crate::wallet::WalletCore;
use crate::wallet_accounts::{render, AccountRecord};
use crate::{felt, vault, CoreError};

/// One Ledger account as the device reported it.
#[derive(Debug, Clone, uniffi::Record)]
pub struct LedgerAccountRequest {
    pub network: NetworkId,
    /// OpenZeppelin or Argent at either scheme; Braavos at Ready's, once proven in use (see the
    /// module).
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

/// `deriveLedgerAccount`: the record the request places, at Bramble's class for its family and
/// Braavos's deployment class for Braavos.
pub(crate) fn record(request: &LedgerAccountRequest) -> Result<StoredAccount, CoreError> {
    let braavos = request.family == AccountFamily::Braavos;
    if braavos && request.scheme != LedgerScheme::Ready {
        return Err(invalid("Invalid Ledger path application"));
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
        braavos.then_some(&catalog().braavos_deployment),
        None,
        &public_key,
        request.network,
    )
}

/// The known tokens' addresses on `network` (`knownTokensForNetwork`).
fn token_addresses(network: NetworkId) -> Result<Vec<wallet_core::Felt>, CoreError> {
    known_tokens(network)
        .iter()
        .map(|token| felt::parse_non_zero(&token.address).ok_or_else(|| invalid("invalid token")))
        .collect()
}

/// `readLedgerAccountState`'s reads for the address `request` places: its class hash, then its
/// balance of each known token on its network. For a Braavos request, the host answers them and
/// passes the replies to `create_ledger_wallet` or `add_ledger_account`.
#[uniffi::export]
pub fn ledger_account_activity_requests(
    request: LedgerAccountRequest,
) -> Result<Vec<RpcRequest>, CoreError> {
    let address = felt::parse_non_zero(&record(&request)?.address)
        .ok_or_else(|| invalid("invalid address"))?;
    Ok(activity::requests(
        &address,
        &token_addresses(request.network)?,
    )?)
}

/// `ledgerStoreRefusal`: a Braavos request passes only when `activity` proves its address in
/// use; every other family passes unchecked.
fn check_activity(
    request: &LedgerAccountRequest,
    activity: &[RpcResponse],
) -> Result<(), CoreError> {
    if request.family != AccountFamily::Braavos {
        return Ok(());
    }
    let tokens = token_addresses(request.network)?.len();
    match activity::judge(tokens, activity) {
        Activity::Active => Ok(()),
        Activity::Unused => Err(invalid("braavos ledger account not found")),
        Activity::Unknown => Err(CoreError::Rpc {
            reason: "braavos ledger account status unavailable".into(),
        }),
    }
}

#[uniffi::export]
impl WalletCore {
    /// Creates a wallet with no recovery phrase whose first account is the Ledger account
    /// `request` names, sealed under `password`, and leaves it unlocked. Refuses when a wallet
    /// exists (`WalletExists`). Such a wallet signs only on its device: everything that needs a
    /// phrase (another phrase account, discovery, revealing a phrase, Nostr coordination) is
    /// refused. A Braavos account needs `activity`, the replies to
    /// `ledger_account_activity_requests`, to prove it in use; other families pass none.
    pub fn create_ledger_wallet(
        &self,
        password: String,
        request: LedgerAccountRequest,
        activity: Vec<RpcResponse>,
    ) -> Result<AccountRecord, CoreError> {
        let password = Zeroizing::new(password);
        vault::check_password(&password)?;
        let record = record(&request)?;
        check_activity(&request, &activity)?;
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
    /// finding its accounts. A Braavos account needs `activity`, as `create_ledger_wallet` does.
    /// Renews the idle window.
    pub fn add_ledger_account(
        &self,
        request: LedgerAccountRequest,
        activity: Vec<RpcResponse>,
    ) -> Result<AccountRecord, CoreError> {
        let record = record(&request)?;
        check_activity(&request, &activity)?;
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
