//! The unlocked vault in memory: one record per account mobile operates (family, class, key
//! source, index, salt, address, public key), one active account, the phrase, and the keys of
//! imported accounts; beside them the wallet's labels, groups, hidden accounts, RPC overrides and
//! backup flag, which live in `public_store`'s preferences file rather than the vault.
//! `vault_v13` reads and writes the sealed form, the extension's schema v13, and keeps what mobile
//! does not operate (`kept`) out of `accounts`, so nothing that lists, selects or signs reaches it.
//!
//! Nothing stored is trusted blindly. `validate` checks every field, and re-derives every record
//! from its key (the phrase's, or an imported account's sealed one) and compares it with what was
//! stored, so a tampered or corrupted record fails closed rather than redirecting Receive or Copy
//! to another address.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use wallet_core::Felt;
use wallet_core::{SecretFelt, StarkXOnlyKeypair};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::account_address::family_address;
use crate::account_catalog::{catalog, AccountFamily};
pub(crate) use crate::account_validate::{check_consistent, validate};
use crate::files::storage;
use crate::groups::StoredGroup;
use crate::kms::Ladder;
use crate::networks::NetworkId;
use crate::vault_wire::Kept;
use crate::{felt, kms, CoreError};

/// Records one vault may hold across networks (mobile policy; bounds vault size and unlock work).
pub(crate) const MAX_ACCOUNT_RECORDS: usize = 128;
/// Highest account index (`RECOVERY_SCAN_LIMITS.maxIndex`, Krusty's unhardened bound).
pub(crate) const MAX_ACCOUNT_INDEX: u32 = 0x7fff_ffff;

/// An index this wallet may derive, restore or create at (the extension's
/// `isRecoveryAccountIndex`): in range, and never strkd's reserved agent index.
pub(crate) fn is_usable_account_index(index: u32) -> bool {
    index <= MAX_ACCOUNT_INDEX && index != catalog().reserved_agent_index
}

/// Where a record's signing key comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeySource {
    /// Derived from the phrase at the record's index on this ladder, in each call that needs it.
    Phrase(Ladder),
    /// A raw key the user imported (the extension's `imported-raw-key`): sealed in the vault and
    /// held in the session (`VaultPayload::imported_keys`). The record's index is 0 and unused.
    Imported,
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
    /// The key tree `account_index` walks, or an imported key.
    pub key: KeySource,
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

    /// The phrase coordinate the record's key is derived at; `None` for an imported key.
    pub(crate) fn phrase_path(&self) -> Option<(Ladder, u32)> {
        match self.key {
            KeySource::Phrase(ladder) => Some((ladder, self.account_index)),
            KeySource::Imported => None,
        }
    }
}

/// One account key as the vault needs it: the signing key (zeroized on drop), its public x, and
/// the y-coordinate a `starknet-bip44` record carries (`None` for Ready's x-only scheme and for
/// an imported key). Resolved inside the call that needs it and dropped there.
pub(crate) type DerivedKey = (StarkXOnlyKeypair, Option<String>);

/// The unlocked wallet. The phrase, the imported keys and the kept secrets are secret; all are
/// zeroized on drop.
#[derive(Zeroize, ZeroizeOnDrop)]
pub(crate) struct VaultPayload {
    /// The records mobile operates. Every list, selection and signature reads these only.
    #[zeroize(skip)]
    pub accounts: Vec<StoredAccount>,
    /// The vault's active account. It may name a kept record (an extension backup whose active
    /// account is a Ledger one), which `record` does not find: then no account is active here.
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
    /// Each imported account's key, by account id, as the vault's `signingKeys` holds them.
    /// Boxed so the map moves pointers, never a key; `SecretFelt` zeroizes itself on drop.
    #[zeroize(skip)]
    pub imported_keys: BTreeMap<String, Box<SecretFelt>>,
    /// What the vault holds that mobile does not operate, written back unchanged. Its secret
    /// records zeroize themselves.
    #[zeroize(skip)]
    pub kept: Kept,
    #[zeroize(skip)]
    pub labels: BTreeMap<String, String>,
    pub mnemonic: String,
    /// Each network's custom RPC endpoint, keyed by catalog id (the extension's `rpcOverrides`, a
    /// preference). May carry an API key: never logged or put in an error.
    #[zeroize(skip)]
    pub rpc_overrides: BTreeMap<String, String>,
    /// Catalog ids whose refused override was dropped on load (`public_store::load_preferences`):
    /// their explorer fallback stays off by default. Set only on load.
    #[zeroize(skip)]
    pub explorer_fallback_off: BTreeSet<String>,
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
            imported_keys: BTreeMap::new(),
            kept: Kept::default(),
            labels: BTreeMap::new(),
            mnemonic,
            rpc_overrides: BTreeMap::new(),
            explorer_fallback_off: BTreeSet::new(),
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

    /// `record`'s key, for the caller to use and drop: derived from the phrase, or a copy of the
    /// imported key, whose public key Krusty computes here. Every account key goes through here.
    pub(crate) fn signing_key(&self, record: &StoredAccount) -> Result<DerivedKey, CoreError> {
        match record.key {
            KeySource::Phrase(ladder) => Ok(wallet_core::account_keypair(
                &self.mnemonic,
                None,
                ladder,
                record.account_index,
            )?),
            KeySource::Imported => {
                let key = self
                    .imported_keys
                    .get(&record.id)
                    .ok_or_else(invalid_payload)?;
                let pair = StarkXOnlyKeypair {
                    public_key_x: wallet_core::stark_public_key(key)?,
                    private_key: SecretFelt::clone(key),
                };
                Ok((pair, None))
            }
        }
    }

    /// `source`'s key placed on `network` (the extension's `deriveNetworkAccount`): the same
    /// family, class and salt, and for an imported account a copy of its key to seal under the
    /// new record.
    pub(crate) fn project(
        &self,
        source: &StoredAccount,
        network: NetworkId,
    ) -> Result<(StoredAccount, Option<Box<SecretFelt>>), CoreError> {
        let (pair, _) = self.signing_key(source)?;
        let public_key = felt::parse_non_zero(&pair.public_key_x).ok_or_else(invalid_payload)?;
        let family = source.family();
        let class_hash = (family != AccountFamily::OpenZeppelin).then(|| source.class_felt());
        let record = record_at(
            family,
            source.key,
            source.account_index,
            class_hash.as_ref(),
            source.salt_felt().as_ref(),
            &public_key,
            network,
        )?;
        let sealed = (source.key == KeySource::Imported).then(|| Box::new(pair.private_key));
        Ok((record, sealed))
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
            KeySource::Phrase(path),
            account_index,
            class_hash,
            salt,
            &public_key,
            network,
        )
    }

    /// A record mobile operates; `None` for a kept one.
    pub(crate) fn record(&self, account_id: &str) -> Option<&StoredAccount> {
        self.accounts.iter().find(|record| record.id == account_id)
    }

    /// Whether the vault holds `account_id` at all, operated or kept: a new record must not
    /// take an id the vault already holds.
    pub(crate) fn holds(&self, account_id: &str) -> bool {
        self.record(account_id).is_some() || self.kept.account_ids().any(|id| id == account_id)
    }

    /// Every account record the vault holds, operated or kept.
    pub(crate) fn record_count(&self) -> usize {
        self.accounts.len() + self.kept.accounts.len()
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
        // A key sealed for a record the rollback removed goes with it.
        let accounts = &self.accounts;
        self.imported_keys
            .retain(|id, _| accounts.iter().any(|record| &record.id == id));
        self.rpc_overrides = state.rpc_overrides;
    }
}

/// Places an already-derived public key under a family, class, and salt on one network.
pub(crate) fn record_at(
    family: AccountFamily,
    key: KeySource,
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
    let key = match (key, account_index) {
        (KeySource::Phrase(Ladder::Address), 0) => KeySource::Phrase(Ladder::Account),
        _ => key,
    };
    let placed = family_address(family, class_hash, salt, public_key, network)?;
    let address = felt::canonical(&placed.address);
    Ok(StoredAccount {
        account_index,
        id: format!("{}:{address}", network.catalog_id()),
        address,
        key,
        class_hash: felt::canonical(&placed.class_hash),
        deployment_salt: placed.deployment_salt.as_ref().map(felt::canonical),
        family: family.wire().into(),
        network: network.catalog_id().into(),
        public_key: felt::canonical(public_key),
    })
}

pub(crate) fn invalid_payload() -> CoreError {
    storage("vault payload is invalid")
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

// Lowest unused hardened account index on `network` (`nextAccountIndex`), past those in `taken`.
// Only this wallet's BIP-44 tree is counted: an own-ladder record holds its index, a Braavos
// vendor-ladder record holds account 0 (it sits at `0'/0/index`), and a Ready record, on its own
// ETH-seeded tree, and an imported key hold none.
pub(crate) fn next_account_index(
    payload: &VaultPayload,
    network: NetworkId,
    taken: &HashSet<u32>,
) -> Result<u32, CoreError> {
    let used: HashSet<u32> = payload
        .accounts
        .iter()
        .filter(|record| record.network() == network)
        .filter_map(|record| match record.phrase_path()? {
            (Ladder::Account, index) => Some(index),
            (Ladder::Address, _) => Some(0),
            (Ladder::ArgentX, _) => None,
        })
        .collect();
    (0..=MAX_ACCOUNT_INDEX)
        .filter(|index| is_usable_account_index(*index))
        .find(|index| !used.contains(index) && !taken.contains(index))
        .ok_or_else(|| CoreError::InvalidInput {
            reason: "account limit reached".into(),
        })
}
