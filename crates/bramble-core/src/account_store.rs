//! The unlocked vault in memory: one record per account (family, class, ladder, index, salt,
//! address, public key), one active account, and the phrase; beside them the wallet's labels,
//! groups, hidden accounts, RPC overrides and backup flag, which live in `public_store`'s
//! preferences file rather than the vault.
//! `vault_v13` reads and writes the sealed form, the extension's schema v13.
//!
//! Nothing stored is trusted blindly. `validate` checks every field, and re-derives every record
//! from the phrase and compares it with what was stored, so a tampered or corrupted record fails
//! closed rather than redirecting Receive or Copy to another address.

use std::collections::{BTreeMap, HashSet};

use wallet_core::Felt;
use wallet_core::StarkXOnlyKeypair;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::account_address::family_address;
use crate::account_catalog::{catalog, AccountFamily};
use crate::account_names::{is_label, js_trim};
use crate::files::storage;
use crate::groups::{self, StoredGroup};
use crate::kms::Ladder;
use crate::networks::NetworkId;
use crate::{felt, kms, rpc_override, CoreError};

/// Records one vault may hold across networks (mobile policy; bounds vault size and unlock work).
pub(crate) const MAX_ACCOUNT_RECORDS: usize = 128;
/// Highest account index (`RECOVERY_SCAN_LIMITS.maxIndex`, Krusty's unhardened bound).
pub(crate) const MAX_ACCOUNT_INDEX: u32 = 0x7fff_ffff;

/// An index this wallet may derive, restore or create at (the extension's
/// `isRecoveryAccountIndex`): in range, and never strkd's reserved agent index.
pub(crate) fn is_usable_account_index(index: u32) -> bool {
    index <= MAX_ACCOUNT_INDEX && index != catalog().reserved_agent_index
}

/// One account record. Public data only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StoredAccount {
    pub account_index: u32,
    pub address: String,
    pub class_hash: String,
    pub deployment_salt: Option<String>,
    pub family: String,
    pub id: String,
    /// The key tree `account_index` walks.
    pub ladder: Ladder,
    pub network: String,
    pub public_key: String,
}

#[expect(
    clippy::expect_used,
    reason = "opening the store refuses any record these would fail on"
)]
impl StoredAccount {
    pub(crate) fn family(&self) -> AccountFamily {
        AccountFamily::from_wire(&self.family).expect("validated on open")
    }

    pub(crate) fn network(&self) -> NetworkId {
        NetworkId::from_catalog_id(&self.network).expect("validated on open")
    }

    pub(crate) fn class_felt(&self) -> Felt {
        felt::parse(&self.class_hash).expect("validated on open")
    }

    pub(crate) fn salt_felt(&self) -> Option<Felt> {
        self.deployment_salt
            .as_deref()
            .map(|salt| felt::parse(salt).expect("validated on open"))
    }

    pub(crate) fn ladder(&self) -> Ladder {
        self.ladder
    }
}

/// One account key as the vault needs it: the signing key (zeroized on drop), its public x, and
/// the y-coordinate a `starknet-bip44` record carries (`None` for Ready's x-only scheme). Derived
/// inside the call that needs it and dropped there; never held in the session.
pub(crate) type DerivedKey = (StarkXOnlyKeypair, Option<String>);

/// The unlocked wallet. Only the phrase is secret; it is zeroized on drop.
#[derive(Zeroize, ZeroizeOnDrop)]
pub(crate) struct VaultPayload {
    #[zeroize(skip)]
    pub accounts: Vec<StoredAccount>,
    #[zeroize(skip)]
    pub active_account_id: Option<String>,
    /// Set when a wallet is created and cleared once its Confirm backup step succeeds
    /// (`backup.rs`). Kept in the preferences file.
    #[zeroize(skip)]
    pub backup_pending: bool,
    #[zeroize(skip)]
    pub groups: Vec<StoredGroup>,
    /// Accounts the host's lists leave out (the extension's `hiddenAccountIds`, a preference).
    #[zeroize(skip)]
    pub hidden_account_ids: Vec<String>,
    #[zeroize(skip)]
    pub labels: BTreeMap<String, String>,
    pub mnemonic: String,
    /// Each network's custom RPC endpoint, keyed by catalog id (the extension's `rpcOverrides`, a
    /// preference). May carry an API key: never logged or put in an error.
    #[zeroize(skip)]
    pub rpc_overrides: BTreeMap<String, String>,
}

/// The non-secret state a mutation may change, captured so a failed write can be rolled back.
pub(crate) struct PublicState {
    accounts: Vec<StoredAccount>,
    active_account_id: Option<String>,
    backup_pending: bool,
    groups: Vec<StoredGroup>,
    hidden_account_ids: Vec<String>,
    labels: BTreeMap<String, String>,
    rpc_overrides: BTreeMap<String, String>,
}

impl VaultPayload {
    /// A new vault: OZ index 0 on `network` for a created wallet, or nothing yet (discovery
    /// pending) for an imported one. A created wallet's backup is pending until confirmed; an
    /// imported phrase is one the user already holds.
    pub(crate) fn new(
        mnemonic: String,
        first_account: Option<NetworkId>,
    ) -> Result<Self, CoreError> {
        let mut payload = Self::opened(mnemonic, Vec::new(), None);
        payload.backup_pending = first_account.is_some();
        if let Some(network) = first_account {
            let record = payload.derive(
                AccountFamily::OpenZeppelin,
                Ladder::Account,
                0,
                None,
                None,
                network,
            )?;
            payload.active_account_id = Some(record.id.clone());
            payload.accounts.push(record);
        }
        Ok(payload)
    }

    /// A vault's records as read, before validation, with no preferences yet.
    pub(crate) fn opened(
        mnemonic: String,
        accounts: Vec<StoredAccount>,
        active_account_id: Option<String>,
    ) -> Self {
        Self {
            accounts,
            active_account_id,
            backup_pending: false,
            groups: Vec::new(),
            hidden_account_ids: Vec::new(),
            labels: BTreeMap::new(),
            mnemonic,
            rpc_overrides: BTreeMap::new(),
        }
    }

    /// An imported wallet waits for discovery until it holds an account (restore never writes
    /// none).
    pub(crate) fn recovery_pending(&self) -> bool {
        self.accounts.is_empty()
    }

    pub(crate) fn public_key(&self, path: Ladder, index: u32) -> Result<Felt, CoreError> {
        kms::public_key(&self.mnemonic, None, path, index)
    }

    /// Derives `record`'s key, for the caller to use and drop.
    pub(crate) fn derived_key(&self, record: &StoredAccount) -> Result<DerivedKey, CoreError> {
        Ok(wallet_core::account_keypair(
            &self.mnemonic,
            None,
            record.ladder,
            record.account_index,
        )?)
    }

    /// Derives one record at `index` on `path`. Nothing here comes from the host except
    /// coordinates. New accounts are only ever created on `Ladder::Account`.
    pub(crate) fn derive(
        &self,
        family: AccountFamily,
        path: Ladder,
        account_index: u32,
        class_hash: Option<&Felt>,
        salt: Option<&Felt>,
        network: NetworkId,
    ) -> Result<StoredAccount, CoreError> {
        let public_key = self.public_key(path, account_index)?;
        record_at(
            family,
            path,
            account_index,
            class_hash,
            salt,
            &public_key,
            network,
        )
    }

    pub(crate) fn record(&self, account_id: &str) -> Option<&StoredAccount> {
        self.accounts.iter().find(|record| record.id == account_id)
    }

    pub(crate) fn snapshot(&self) -> PublicState {
        PublicState {
            accounts: self.accounts.clone(),
            active_account_id: self.active_account_id.clone(),
            backup_pending: self.backup_pending,
            groups: self.groups.clone(),
            hidden_account_ids: self.hidden_account_ids.clone(),
            labels: self.labels.clone(),
            rpc_overrides: self.rpc_overrides.clone(),
        }
    }

    /// Whether the sealed part (accounts, active account) moved since `before`.
    pub(crate) fn vault_changed(&self, before: &PublicState) -> bool {
        self.accounts != before.accounts || self.active_account_id != before.active_account_id
    }

    /// Whether the preferences (labels, groups, hidden accounts, RPC overrides, backup flag) moved
    /// since `before`.
    pub(crate) fn preferences_changed(&self, before: &PublicState) -> bool {
        self.labels != before.labels
            || self.groups != before.groups
            || self.hidden_account_ids != before.hidden_account_ids
            || self.rpc_overrides != before.rpc_overrides
            || self.backup_pending != before.backup_pending
    }

    pub(crate) fn restore(&mut self, state: PublicState) {
        self.accounts = state.accounts;
        self.active_account_id = state.active_account_id;
        self.backup_pending = state.backup_pending;
        self.groups = state.groups;
        self.hidden_account_ids = state.hidden_account_ids;
        self.labels = state.labels;
        self.rpc_overrides = state.rpc_overrides;
    }
}

/// Places an already-derived public key under a family, class, and salt on one network.
pub(crate) fn record_at(
    family: AccountFamily,
    ladder: Ladder,
    account_index: u32,
    class_hash: Option<&Felt>,
    salt: Option<&Felt>,
    public_key: &Felt,
    network: NetworkId,
) -> Result<StoredAccount, CoreError> {
    if !is_usable_account_index(account_index) {
        return Err(CoreError::InvalidInput {
            reason: "account index out of range".into(),
        });
    }
    // Index 0 on Braavos's address ladder is index 0 on this wallet's own (`m/44'/9004'/0'/0/0`,
    // the same key); v13 has one spelling for both, so the record takes the one it reads back as.
    let ladder = match (ladder, account_index) {
        (Ladder::Address, 0) => Ladder::Account,
        _ => ladder,
    };
    let placed = family_address(family, class_hash, salt, public_key, network)?;
    let address = felt::canonical(&placed.address);
    Ok(StoredAccount {
        account_index,
        id: format!("{}:{address}", network.catalog_id()),
        address,
        ladder,
        class_hash: felt::canonical(&placed.class_hash),
        deployment_salt: placed.deployment_salt.as_ref().map(felt::canonical),
        family: family.wire().into(),
        network: network.catalog_id().into(),
        public_key: felt::canonical(public_key),
    })
}

fn invalid_payload() -> CoreError {
    storage("vault payload is invalid")
}

fn is_canonical_felt(value: &str, non_zero: bool) -> bool {
    felt::parse(value)
        .filter(|parsed| !non_zero || *parsed != Felt::ZERO)
        .is_some_and(|parsed| felt::canonical(&parsed) == value)
}

/// Which ladders a family's records may sit on. Every family may be on this wallet's own ladder,
/// where every account this wallet creates is derived; Braavos may also be on its vendor's
/// address ladder, and Argent on Ready's.
pub(crate) fn ladder_fits(family: AccountFamily, path: Ladder) -> bool {
    match path {
        Ladder::Account => true,
        Ladder::Address => family == AccountFamily::Braavos,
        Ladder::ArgentX => family == AccountFamily::Argent,
    }
}

// Lowest unused hardened account index on `network` (`nextAccountIndex`). Only this wallet's
// BIP-44 tree is counted: an own-ladder record holds its index, a Braavos vendor-ladder record
// holds account 0 (it sits at `0'/0/index`), and a Ready record, on its own ETH-seeded tree,
// holds none.
pub(crate) fn next_account_index(
    payload: &VaultPayload,
    network: NetworkId,
) -> Result<u32, CoreError> {
    let used: HashSet<u32> = payload
        .accounts
        .iter()
        .filter(|record| record.network() == network)
        .filter_map(|record| match record.ladder() {
            Ladder::Account => Some(record.account_index),
            Ladder::Address => Some(0),
            Ladder::ArgentX => None,
        })
        .collect();
    (0..=MAX_ACCOUNT_INDEX)
        .filter(|index| is_usable_account_index(*index))
        .find(|index| !used.contains(index))
        .ok_or_else(|| CoreError::InvalidInput {
            reason: "account limit reached".into(),
        })
}

// Field grammar of one record, before any derivation.
fn is_well_formed(record: &StoredAccount) -> bool {
    let (Some(family), Some(network)) = (
        AccountFamily::from_wire(&record.family),
        NetworkId::from_catalog_id(&record.network),
    ) else {
        return false;
    };
    let salt_ok = match (&family, &record.deployment_salt) {
        (AccountFamily::OpenZeppelin, None) => true,
        (AccountFamily::OpenZeppelin, Some(_)) | (_, None) => false,
        (_, Some(salt)) => is_canonical_felt(salt, false),
    };
    record.account_index <= MAX_ACCOUNT_INDEX
        && ladder_fits(family, record.ladder)
        && is_canonical_felt(&record.address, true)
        && is_canonical_felt(&record.public_key, true)
        && is_canonical_felt(&record.class_hash, true)
        && salt_ok
        && record.id == format!("{}:{}", network.catalog_id(), record.address)
}

// Every structural invariant, without derivation.
fn is_consistent(payload: &VaultPayload) -> bool {
    let ids: HashSet<&str> = payload.accounts.iter().map(|r| r.id.as_str()).collect();
    kms::validate_mnemonic(&payload.mnemonic)
        && payload.accounts.len() <= MAX_ACCOUNT_RECORDS
        && ids.len() == payload.accounts.len()
        && payload.accounts.iter().all(is_well_formed)
        && match &payload.active_account_id {
            None => payload.accounts.is_empty(),
            Some(id) => ids.contains(id.as_str()),
        }
        && payload.labels.len() <= MAX_ACCOUNT_RECORDS
        && payload.labels.iter().all(|(id, label)| {
            ids.contains(id.as_str()) && is_label(label) && js_trim(label) == label
        })
        && groups::is_valid(&payload.groups, &ids)
        && payload
            .hidden_account_ids
            .iter()
            .all(|id| ids.contains(id.as_str()))
        && payload
            .hidden_account_ids
            .iter()
            .collect::<HashSet<_>>()
            .len()
            == payload.hidden_account_ids.len()
        && payload.rpc_overrides.iter().all(|(network, url)| {
            NetworkId::from_catalog_id(network).is_some() && rpc_override::is_rpc_url(url)
        })
}

// Re-derives every record from the phrase, one key per record, each also shown to `sealed`;
// any difference is tampering or corruption.
fn rederives(
    payload: &VaultPayload,
    sealed: &mut dyn FnMut(&StoredAccount, &DerivedKey) -> bool,
) -> Result<bool, CoreError> {
    for record in &payload.accounts {
        let key = payload.derived_key(record)?;
        let Some(public_key) = felt::parse_non_zero(&key.0.public_key_x) else {
            return Ok(false);
        };
        let family = record.family();
        let class_hash = (family != AccountFamily::OpenZeppelin).then(|| record.class_felt());
        let expected = record_at(
            family,
            record.ladder,
            record.account_index,
            class_hash.as_ref(),
            record.salt_felt().as_ref(),
            &public_key,
            record.network(),
        );
        if expected.ok().as_ref() != Some(record) || !sealed(record, &key) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Checks a payload in full, re-deriving every record; `sealed` also checks what the vault
/// sealed beside each record against its derived key. Used on open.
pub(crate) fn validate(
    payload: &VaultPayload,
    mut sealed: impl FnMut(&StoredAccount, &DerivedKey) -> bool,
) -> Result<(), CoreError> {
    if !is_consistent(payload) || !rederives(payload, &mut sealed)? {
        return Err(invalid_payload());
    }
    Ok(())
}

/// Checks every structural invariant without re-deriving. Used before every write, whose new
/// records were derived by this core in the same call.
pub(crate) fn check_consistent(payload: &VaultPayload) -> Result<(), CoreError> {
    if is_consistent(payload) {
        Ok(())
    } else {
        Err(invalid_payload())
    }
}
