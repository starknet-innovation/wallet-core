//! The vault payload: the extension's schema v13 (`packages/platform/vault/src/schema/`) or v14
//! (`release/v0.10.11`: v13 and `accountProvenance`), sealed in mobile's DEK container
//! (`container.rs`), whose `version` is the schema version. A new vault is written at
//! `NEW_VAULT_SCHEMA`, 14; a v13 vault is raised to 14 on unlock and written back
//! (`WalletCore::install`), as the extension's migration does. A v14 vault's provenance is kept,
//! and maintained as the extension's service maintains it (`vault_provenance`).
//!
//! Reading accepts every vault the extension's v13 or v14 validator accepts (`vault_rules`,
//! `vault_budget`), with one exception: a vault with no phrase (a Ledger-only or key-only wallet)
//! is refused, since this wallet has nothing to run without one (until P8). Mobile operates the
//! records it can sign for: phrase-derived accounts (`starknet-bip44` on either of its two
//! ladders, Ready's `argentx-bip44`) and accounts imported from a raw key (`imported-raw-key`,
//! PLAN.md P3), on the networks it has, up to `MAX_ACCOUNT_RECORDS`, each only when it re-derives
//! from its key (`vault_admit`). A record that does not is kept, never a reason to refuse.
//!
//! Everything else the vault holds (a Ledger account, an account derived from an imported phrase,
//! one on the extension's devnet, a record mobile does not re-derive, their sealed keys, origin
//! grants, imported phrases, Ledger viewing keys, shadow-account scopes, multisig viewing keys and
//! conflict marks) is kept unused (`vault_wire::Kept`, listed by `unusable_accounts`) and written
//! back as it was read: the same records, in the same order. Only object key order and string
//! escapes can differ, which no reader of the vault sees.
//!
//! Signing still derives each phrase account's key per call; its sealed key is what the
//! extension's schema requires, and must equal the derived one. An imported account's sealed key,
//! and a pre-fix Ready account's, is the only copy, so it is held in the session
//! (`VaultPayload::held_keys`) and signs.
//!
//! The only migration is 13 to 14, a version bump. Mobile's earlier vault versions
//! 1 to 5 were developer builds holding no funds (`docs/PLAN.md`, Vault switch): any of them fails
//! closed with `outdated()`, after the password is proven, and the user resets the wallet.

use std::collections::BTreeMap;
use std::io;

use zeroize::Zeroizing;

use crate::account_catalog::AccountFamily;
use crate::account_store::{self, KeySource, StoredAccount, VaultPayload, MAX_ACCOUNT_RECORDS};
use crate::container::{Dek, StoredVault, VaultFile, V1_SCOPE};
use crate::files::storage;
use crate::kms::Ladder;
use crate::ledger::LedgerScheme;
use crate::vault_admit::{self, Admission};
use crate::vault_wire::{Account, Derivation, Kept, SigningKey, Vault};
use crate::{kms, vault, vault_budget, vault_rules, CoreError};

/// The schema a new vault is written at (Bramble's choice): the extension's
/// `CURRENT_SCHEMA_VERSION`, 14 since v0.10.11.
pub(crate) const NEW_VAULT_SCHEMA: u32 = PROVENANCE_SCHEMA;
/// The extension's schema 14, which adds `accountProvenance`; the newest one mobile reads.
pub(crate) const PROVENANCE_SCHEMA: u32 = 14;
/// The schemas mobile reads, as the payload's `schemaVersion` and the container's `version`.
const READABLE_SCHEMAS: [u32; 2] = [13, PROVENANCE_SCHEMA];

fn invalid() -> CoreError {
    storage("vault payload is invalid")
}

/// A vault an earlier mobile build wrote (vault versions 1 to 5).
fn outdated() -> CoreError {
    storage("this wallet was saved by an earlier test build; delete it and import your phrase")
}

/// The extension's `VAULT_FULL`: the write would put the vault past the extension's budget.
fn full() -> CoreError {
    storage("vault storage limit reached")
}

impl Derivation {
    fn of(key: KeySource, index: u32) -> Self {
        let ladder = match key {
            KeySource::Phrase(ladder) => ladder,
            KeySource::Imported => return Self::ImportedRawKey {},
            // An index past the scheme's has no path; `vault_rules` then refuses the write.
            KeySource::Ledger(scheme) => {
                return Self::Ledger {
                    account_index: index.into(),
                    eip2645_application: scheme.application().into(),
                    path: crate::ledger::ledger_account_path(scheme.application(), index.into())
                        .map(|path| path.to_string())
                        .unwrap_or_default(),
                }
            }
        };
        let index = u64::from(index);
        match ladder {
            Ladder::Account => Self::StarknetBip44 {
                account_index: index,
                address_index: 0,
                seed_id: None,
            },
            Ladder::Address => Self::StarknetBip44 {
                account_index: 0,
                address_index: index,
                seed_id: None,
            },
            Ladder::ArgentX => Self::ArgentxBip44 {
                address_index: index,
                seed_id: None,
            },
        }
    }

    /// Mobile's coordinate for a record it operates. `{0, 0}` is index 0 on either Starknet
    /// ladder (the same key); it reads as `Account`. A path stepping both fields, or past a `u32`,
    /// is not one mobile derives. An imported key has no coordinate; its record's index is 0.
    fn key(&self) -> Option<(KeySource, u32)> {
        let (ladder, index) = match *self {
            Self::StarknetBip44 {
                account_index,
                address_index: 0,
                seed_id: None,
            } => (Ladder::Account, account_index),
            Self::StarknetBip44 {
                account_index: 0,
                address_index,
                seed_id: None,
            } => (Ladder::Address, address_index),
            Self::ArgentxBip44 {
                address_index,
                seed_id: None,
            } => (Ladder::ArgentX, address_index),
            Self::ImportedRawKey {} => return Some((KeySource::Imported, 0)),
            // `vault_rules` already holds the path to its scheme's at its index.
            Self::Ledger {
                account_index,
                ref eip2645_application,
                ref path,
            } => {
                let scheme = LedgerScheme::from_application(eip2645_application)?;
                let index = u32::try_from(account_index).ok()?;
                let device = scheme.device(index)?;
                return (device.path.to_string() == *path)
                    .then_some((KeySource::Ledger(scheme), index));
            }
            _ => return None,
        };
        Some((KeySource::Phrase(ladder), u32::try_from(index).ok()?))
    }
}

/// Counts what `serde_json` writes, so `encode` can size its buffer exactly.
#[derive(Default)]
struct Counter(usize);

impl io::Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 += bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Serialises `payload` at its schema: mobile's records, each phrase account's signing key
/// derived for the call only and each imported key the session holds, with everything kept put
/// back where it was. Refuses a vault the extension would refuse, including one past its budget.
/// Run `account_store::check_consistent` first.
pub(crate) fn encode(payload: &VaultPayload) -> Result<Zeroizing<Vec<u8>>, CoreError> {
    let kept = &payload.kept;
    let mut accounts = Vec::with_capacity(payload.accounts.len() + kept.accounts.len());
    let mut signing_keys = Vec::with_capacity(payload.accounts.len() + kept.signing_keys.len());
    let mut kept_accounts = kept.accounts.iter().peekable();
    for (at, record) in payload.accounts.iter().enumerate() {
        while let Some((_, account)) = kept_accounts.next_if(|(before, _)| *before <= at) {
            accounts.push(account.clone());
        }
        // A Ledger record holds no key: written with neither a y-coordinate nor a sealed key.
        let (pair, public_key_y) = match record.key {
            KeySource::Ledger(_) => (None, None),
            _ => {
                let (pair, y) = payload.signing_key(record)?;
                (Some(pair), y)
            }
        };
        accounts.push(Account {
            account_contract_type: record.family.clone(),
            address: record.address.clone(),
            class_hash: record.class_hash.clone(),
            deployment_salt: record.deployment_salt.clone(),
            derivation: Derivation::of(record.key, record.account_index),
            id: record.id.clone(),
            network_id: record.network.clone(),
            public_key_x: record.public_key.clone(),
            public_key_y,
        });
        if let Some(pair) = pair {
            signing_keys.push(SigningKey {
                account_id: record.id.clone(),
                // Moved out of the wiped buffer, not copied; the field is zeroized with the vault.
                private_key: std::mem::take(&mut *pair.private_key.expose_secret_hex()),
            });
        }
    }
    accounts.extend(kept_accounts.map(|(_, account)| account.clone()));
    signing_keys.extend(kept.signing_keys.iter().cloned());
    // The vault's own key order; a key it did not list (a record added since) goes last.
    signing_keys.sort_by_key(|key| {
        kept.key_order
            .iter()
            .position(|id| *id == key.account_id)
            .unwrap_or(usize::MAX)
    });
    let vault = Vault {
        accounts,
        account_provenance: payload.account_provenance.clone(),
        account_secrets: kept.account_secrets.clone(),
        active_account_id: payload.active_account_id.clone(),
        imported_seeds: kept.imported_seeds.clone(),
        mnemonic: Some(payload.mnemonic.clone()),
        multisig_strk20_key_conflicts: kept.multisig_strk20_key_conflicts.clone(),
        multisig_strk20_viewing_keys: kept.multisig_strk20_viewing_keys.clone(),
        origin_grants: kept.origin_grants.clone(),
        schema_version: payload.schema_version,
        shadow_account_scopes: kept.shadow_account_scopes.clone(),
        signing_keys,
    };
    if !vault_rules::is_valid(&vault) {
        return Err(invalid());
    }
    // Sized first, so the buffer never reallocates and leaves no copy of a secret behind.
    let mut size = Counter::default();
    serde_json::to_writer(&mut size, &vault).map_err(|_| storage("serialise vault"))?;
    let mut json = Zeroizing::new(Vec::with_capacity(size.0));
    serde_json::to_writer(&mut *json, &vault).map_err(|_| storage("serialise vault"))?;
    if !vault_budget::within(&json) {
        return Err(full());
    }
    Ok(json)
}

/// Parses and validates a decrypted v13 or v14 payload in full. Labels, groups and the backup
/// flag are not in the vault; the caller loads them (`public_store::load_preferences`).
pub(crate) fn decode(plaintext: &[u8]) -> Result<VaultPayload, CoreError> {
    // The size budget first: it is linear, and it bounds what the rules then scan.
    if !vault_budget::within(plaintext) {
        return Err(invalid());
    }
    let mut vault: Vault = serde_json::from_slice(plaintext).map_err(|_| invalid())?;
    // The extension's rules over the whole vault, then mobile's: a phrase to run on.
    if !vault_rules::is_valid(&vault) {
        return Err(invalid());
    }
    let mnemonic = vault.mnemonic.take().ok_or_else(invalid)?;
    let mut kept = Kept {
        key_order: vault
            .signing_keys
            .iter()
            .map(|key| key.account_id.clone())
            .collect(),
        account_secrets: std::mem::take(&mut vault.account_secrets),
        imported_seeds: std::mem::take(&mut vault.imported_seeds),
        multisig_strk20_key_conflicts: std::mem::take(&mut vault.multisig_strk20_key_conflicts),
        multisig_strk20_viewing_keys: std::mem::take(&mut vault.multisig_strk20_viewing_keys),
        origin_grants: std::mem::take(&mut vault.origin_grants),
        shadow_account_scopes: std::mem::take(&mut vault.shadow_account_scopes),
        ..Kept::default()
    };
    if !kms::validate_mnemonic(&mnemonic) {
        return Err(invalid());
    }
    // Each record mobile could operate is admitted or kept (`vault_admit`); every other record
    // is kept. Only the first `MAX_ACCOUNT_RECORDS` are re-derived, which bounds unlock work.
    let mut accounts = Vec::with_capacity(vault.accounts.len().min(MAX_ACCOUNT_RECORDS));
    let mut held_keys = BTreeMap::new();
    for account in std::mem::take(&mut vault.accounts) {
        let candidate = (vault_rules::is_operable(&account)
            && accounts.len() < MAX_ACCOUNT_RECORDS)
            .then(|| stored(&account))
            .flatten();
        let admission = match &candidate {
            Some(record) => vault_admit::admit(
                &mnemonic,
                record,
                &vault.signing_keys,
                account.public_key_y.as_ref(),
            )?,
            None => Admission::Kept,
        };
        match (candidate, admission) {
            (Some(record), Admission::Derived | Admission::Device) => accounts.push(record),
            (Some(record), Admission::Held(key)) => {
                held_keys.insert(record.id.clone(), key);
                accounts.push(record);
            }
            _ => kept.accounts.push((accounts.len(), account)),
        }
    }
    // Each kept record's key goes with it; the extension's bindings already hold one key per
    // record but a Ledger one. An operated record's key is derived or held, never read again.
    kept.signing_keys = std::mem::take(&mut vault.signing_keys)
        .into_iter()
        .filter(|key| kept.account_ids().any(|id| id == key.account_id))
        .collect();
    let mut payload = VaultPayload::opened(mnemonic, accounts, vault.active_account_id.take());
    payload.kept = kept;
    payload.held_keys = held_keys;
    payload.schema_version = vault.schema_version;
    payload.account_provenance = vault.account_provenance.take();
    account_store::check_consistent(&payload)?;
    Ok(payload)
}

/// One v13 record in mobile's shape; `None` for a coordinate or family mobile does not hold.
fn stored(account: &Account) -> Option<StoredAccount> {
    let (key, account_index) = account.derivation.key()?;
    AccountFamily::from_wire(&account.account_contract_type)?;
    Some(StoredAccount {
        account_index,
        address: account.address.clone(),
        class_hash: account.class_hash.clone(),
        deployment_salt: account.deployment_salt.clone(),
        family: account.account_contract_type.clone(),
        id: account.id.clone(),
        key,
        network: account.network_id.clone(),
        public_key: account.public_key_x.clone(),
    })
}

/// A vault opened into memory.
pub(crate) struct Opened {
    pub payload: VaultPayload,
    pub dek: Dek,
    pub container: VaultFile,
}

/// Opens a container's payload with its (already proven) DEK. The payload's schema must be the
/// container's.
pub(crate) fn open_container(file: VaultFile, dek: Dek) -> Result<Opened, CoreError> {
    if !READABLE_SCHEMAS.contains(&file.version) {
        return Err(outdated());
    }
    let payload = decode(&file.open_payload(&dek)?)?;
    if payload.schema_version != file.version {
        return Err(invalid());
    }
    Ok(Opened {
        payload,
        dek,
        container: file,
    })
}

/// Proves `password` against whatever is on disk (one KDF), without reading the payload.
pub(crate) fn prove_password(stored: &StoredVault, password: &str) -> Result<(), CoreError> {
    match stored {
        StoredVault::Container(file) => file.open_dek(password).map(drop)?,
        StoredVault::Envelope(envelope) => vault::open(envelope, password, V1_SCOPE).map(drop)?,
    }
    Ok(())
}

/// Opens the vault on disk with the password (one KDF). An earlier build's vault is refused
/// once the password is proven, so a wrong password still reads as one.
pub(crate) fn open_stored(stored: StoredVault, password: &str) -> Result<Opened, CoreError> {
    match stored {
        StoredVault::Container(file) => {
            let (dek, _) = file.open_dek(password)?;
            open_container(*file, dek)
        }
        envelope @ StoredVault::Envelope(_) => {
            prove_password(&envelope, password)?;
            Err(outdated())
        }
    }
}

#[cfg(test)]
#[path = "vault_v13_tests.rs"]
mod tests;
