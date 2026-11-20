//! The vault payload: the extension's schema v13 (`packages/platform/vault/src/schema/`), sealed
//! in mobile's DEK container (`container.rs`), whose `version` is the schema version.
//!
//! Mobile writes the subset of v13 it holds: phrase-derived accounts (`starknet-bip44` on either
//! of its two ladders, Ready's `argentx-bip44`), accounts imported from a raw key
//! (`imported-raw-key`, PLAN.md P3), one signing key per account, an active account, and the six
//! collections it has no use for written empty. Reading accepts exactly that subset and checks it
//! against both rule sets: the extension's (field grammar, canonical felts, the account ↔
//! signing-key bijection, the active account) and mobile's stronger one, which re-derives every
//! record from its key (`account_store`): a phrase account's record, public key and sealed key
//! from the phrase, an imported account's public key and address from the key it sealed. A vault
//! holding anything else (a Ledger account, an origin grant, an imported seed, a null phrase) is
//! refused rather than half-read.
//!
//! Signing still derives each phrase account's key per call; its sealed key is what the
//! extension's schema requires, and must equal the derived one. An imported account's sealed key
//! is the only copy, so it is held in the session (`VaultPayload::imported_keys`) and signs.
//!
//! There is no migration. Mobile's earlier vault versions 1 to 5 were developer builds holding
//! no funds (`docs/PLAN.md`, Vault switch): any of them fails closed with `outdated()`, after the
//! password is proven, and the user resets the wallet.

use serde::de::{Error as _, IgnoredAny};
use serde::ser::SerializeSeq;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::account_catalog::AccountFamily;
use crate::account_store::{self, KeySource, StoredAccount, VaultPayload};
use crate::container::{Dek, StoredVault, VaultFile, V1_SCOPE};
use crate::files::storage;
use crate::kms::Ladder;
use crate::{felt, vault, CoreError};

/// The extension's `CURRENT_SCHEMA_VERSION`: the payload's `schemaVersion` and the container's
/// `version`.
pub(crate) const SCHEMA_VERSION: u32 = 13;

fn invalid() -> CoreError {
    storage("vault payload is invalid")
}

/// A vault an earlier mobile build wrote (vault versions 1 to 5).
fn outdated() -> CoreError {
    storage("this wallet was saved by an earlier test build; delete it and import your phrase")
}

/// A v13 collection mobile never holds: written `[]`; a vault with any entry is refused. Entries
/// are skipped unread, so nothing in them (some are secrets) is kept in memory.
struct Unused;

impl Serialize for Unused {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_seq(Some(0))?.end()
    }
}

impl<'de> Deserialize<'de> for Unused {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let entries = Vec::<IgnoredAny>::deserialize(deserializer)?;
        if entries.is_empty() {
            Ok(Self)
        } else {
            Err(D::Error::custom("unsupported vault entry"))
        }
    }
}

/// `AccountDerivation`, the kinds mobile writes: two phrase-derived, and an imported key.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
enum Derivation {
    #[serde(rename = "starknet-bip44", rename_all = "camelCase")]
    StarknetBip44 {
        account_index: u32,
        address_index: u32,
    },
    #[serde(rename = "argentx-bip44", rename_all = "camelCase")]
    ArgentxBip44 { address_index: u32 },
    #[serde(rename = "imported-raw-key")]
    ImportedRawKey {},
}

impl Derivation {
    fn of(key: KeySource, index: u32) -> Self {
        let ladder = match key {
            KeySource::Phrase(ladder) => ladder,
            KeySource::Imported => return Self::ImportedRawKey {},
        };
        match ladder {
            Ladder::Account => Self::StarknetBip44 {
                account_index: index,
                address_index: 0,
            },
            Ladder::Address => Self::StarknetBip44 {
                account_index: 0,
                address_index: index,
            },
            Ladder::ArgentX => Self::ArgentxBip44 {
                address_index: index,
            },
        }
    }

    /// Mobile's coordinate. `{0, 0}` is index 0 on either Starknet ladder (the same key); it
    /// reads as `Account`. A path stepping both fields is not one mobile derives. An imported key
    /// has no coordinate; its record's index is 0.
    fn key(&self) -> Option<(KeySource, u32)> {
        let (ladder, index) = match *self {
            Self::StarknetBip44 {
                account_index,
                address_index: 0,
            } => (Ladder::Account, account_index),
            Self::StarknetBip44 {
                account_index: 0,
                address_index,
            } => (Ladder::Address, address_index),
            Self::StarknetBip44 { .. } => return None,
            Self::ArgentxBip44 { address_index } => (Ladder::ArgentX, address_index),
            Self::ImportedRawKey {} => return Some((KeySource::Imported, 0)),
        };
        Some((KeySource::Phrase(ladder), index))
    }
}

/// `AccountRef` (`wallet-model/src/accounts/accounts.ts`). Public data only.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Account {
    account_contract_type: String,
    address: String,
    class_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    deployment_salt: Option<String>,
    derivation: Derivation,
    id: String,
    network_id: String,
    public_key_x: String,
    /// Present exactly for `starknet-bip44`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    public_key_y: Option<String>,
}

/// `SigningKeyRef`.
#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SigningKey {
    account_id: String,
    private_key: String,
}

/// `VaultV13`. The phrase and signing keys are zeroized on drop.
#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Vault {
    #[zeroize(skip)]
    accounts: Vec<Account>,
    #[zeroize(skip)]
    account_secrets: Unused,
    #[zeroize(skip)]
    active_account_id: Option<String>,
    #[zeroize(skip)]
    imported_seeds: Unused,
    mnemonic: Option<String>,
    #[zeroize(skip)]
    multisig_strk20_key_conflicts: Unused,
    #[zeroize(skip)]
    multisig_strk20_viewing_keys: Unused,
    #[zeroize(skip)]
    origin_grants: Unused,
    #[zeroize(skip)]
    schema_version: u32,
    #[zeroize(skip)]
    shadow_account_scopes: Unused,
    signing_keys: Vec<SigningKey>,
}

/// Room for one serialised account record and its signing key, with margin, so `encode`'s buffer
/// never reallocates and leaves no copy of the phrase or a key behind.
const RECORD_BYTES: usize = 1024;

/// Serialises `payload` as a v13 vault, deriving each phrase account's signing key for the call
/// only and sealing each imported key the session holds. Run `account_store::check_consistent`
/// first.
pub(crate) fn encode(payload: &VaultPayload) -> Result<Zeroizing<Vec<u8>>, CoreError> {
    let mut accounts = Vec::with_capacity(payload.accounts.len());
    let mut signing_keys = Vec::with_capacity(payload.accounts.len());
    for record in &payload.accounts {
        let (pair, public_key_y) = payload.signing_key(record)?;
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
        signing_keys.push(SigningKey {
            account_id: record.id.clone(),
            private_key: felt::canonical(pair.private_key.expose_secret()),
        });
    }
    let vault = Vault {
        accounts,
        account_secrets: Unused,
        active_account_id: payload.active_account_id.clone(),
        imported_seeds: Unused,
        mnemonic: Some(payload.mnemonic.clone()),
        multisig_strk20_key_conflicts: Unused,
        multisig_strk20_viewing_keys: Unused,
        origin_grants: Unused,
        schema_version: SCHEMA_VERSION,
        shadow_account_scopes: Unused,
        signing_keys,
    };
    let capacity = RECORD_BYTES * (vault.accounts.len() + 4);
    let mut json = Zeroizing::new(Vec::with_capacity(capacity));
    serde_json::to_writer(&mut *json, &vault).map_err(|_| storage("serialise vault"))?;
    Ok(json)
}

/// Parses and validates a decrypted v13 payload in full. Labels, groups and the backup flag are
/// not in the vault; the caller loads them (`public_store::load_preferences`).
pub(crate) fn decode(plaintext: &[u8]) -> Result<VaultPayload, CoreError> {
    let mut vault: Vault = serde_json::from_slice(plaintext).map_err(|_| invalid())?;
    if vault.schema_version != SCHEMA_VERSION {
        return Err(invalid());
    }
    let mnemonic = vault.mnemonic.take().ok_or_else(invalid)?;
    let accounts = vault
        .accounts
        .iter()
        .map(stored)
        .collect::<Option<Vec<_>>>()
        .ok_or_else(invalid)?;
    let mut payload = VaultPayload::opened(mnemonic, accounts, vault.active_account_id.take());
    // The extension's bijection: one signing key per account, none spare. With the ids unique
    // (`validate` checks them first) and the counts equal, each account finding its key is enough.
    if vault.signing_keys.len() != payload.accounts.len() {
        return Err(invalid());
    }
    // An imported account's key is the one it sealed: canonical, a usable scalar
    // (`isSigningKeyRef`).
    for record in &payload.accounts {
        if record.key != KeySource::Imported {
            continue;
        }
        let sealed = vault
            .signing_keys
            .iter()
            .find(|sealed| sealed.account_id == record.id)
            .and_then(|sealed| felt::parse_sealed_private_key(&sealed.private_key))
            .ok_or_else(invalid)?;
        payload
            .imported_keys
            .insert(record.id.clone(), Box::new(sealed));
    }
    // Mobile's rules (grammar, references, every record re-derived from its key), and beside
    // each record what the vault sealed: a phrase account's key and y-coordinate must be the
    // derived ones; an imported account carries no y-coordinate.
    account_store::validate(&payload, |record, (pair, public_key_y)| {
        let mut sealed = vault
            .signing_keys
            .iter()
            .filter(|sealed| sealed.account_id == record.id);
        let (Some(sealed), None) = (sealed.next(), sealed.next()) else {
            return false;
        };
        let wire_y = vault
            .accounts
            .iter()
            .find(|account| account.id == record.id)
            .map(|account| &account.public_key_y);
        // Equal to the derived canonical text, which also holds it canonical and in range. An
        // imported key was read from this very text above.
        let key_matches = record.key == KeySource::Imported || {
            let derived = Zeroizing::new(felt::canonical(pair.private_key.expose_secret()));
            *sealed.private_key == *derived
        };
        key_matches && wire_y == Some(public_key_y)
    })?;
    Ok(payload)
}

/// One v13 record in mobile's shape; `None` for a derivation mobile does not hold.
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

/// Opens a container's payload with its (already proven) DEK.
pub(crate) fn open_container(file: VaultFile, dek: Dek) -> Result<Opened, CoreError> {
    if file.version != SCHEMA_VERSION {
        return Err(outdated());
    }
    let payload = decode(&file.open_payload(&dek)?)?;
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
mod tests {
    use super::*;

    #[test]
    fn derivations_round_trip_through_ladders() {
        for (ladder, index) in [
            (Ladder::Account, 0),
            (Ladder::Account, 7),
            (Ladder::Address, 3),
            (Ladder::ArgentX, 0),
            (Ladder::ArgentX, 2),
        ] {
            let key = KeySource::Phrase(ladder);
            assert_eq!(Derivation::of(key, index).key(), Some((key, index)));
        }
        // Index 0 on the address ladder is the account ladder's index 0.
        assert_eq!(
            Derivation::of(KeySource::Phrase(Ladder::Address), 0).key(),
            Some((KeySource::Phrase(Ladder::Account), 0))
        );
        let both = Derivation::StarknetBip44 {
            account_index: 1,
            address_index: 1,
        };
        assert_eq!(both.key(), None);
        assert_eq!(
            Derivation::of(KeySource::Imported, 0).key(),
            Some((KeySource::Imported, 0))
        );
    }

    #[test]
    fn derivations_keep_the_extensions_spelling() {
        let json =
            serde_json::to_value(Derivation::of(KeySource::Phrase(Ladder::Address), 1)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"kind": "starknet-bip44", "accountIndex": 0, "addressIndex": 1})
        );
        let json =
            serde_json::to_value(Derivation::of(KeySource::Phrase(Ladder::ArgentX), 4)).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"kind": "argentx-bip44", "addressIndex": 4})
        );
        let json = serde_json::to_value(Derivation::of(KeySource::Imported, 0)).unwrap();
        assert_eq!(json, serde_json::json!({"kind": "imported-raw-key"}));
        let imported: Derivation =
            serde_json::from_value(serde_json::json!({"kind": "imported-raw-key"})).unwrap();
        assert_eq!(imported, Derivation::ImportedRawKey {});
        for refused in [
            serde_json::json!({"kind": "ledger", "accountIndex": 0}),
            serde_json::json!({"kind": "imported-raw-key", "addressIndex": 0}),
            serde_json::json!({"kind": "imported-raw-key", "seedId": "phrase-1"}),
            serde_json::json!({"kind": "argentx-bip44", "addressIndex": 0, "extra": 1}),
            serde_json::json!({"kind": "starknet-bip44", "accountIndex": 0}),
        ] {
            assert!(serde_json::from_value::<Derivation>(refused).is_err());
        }
    }

    #[test]
    fn unused_collections_must_be_empty() {
        assert!(serde_json::from_str::<Unused>("[]").is_ok());
        assert!(serde_json::from_str::<Unused>(r#"[{"origin":"x"}]"#).is_err());
        assert!(serde_json::from_str::<Unused>("null").is_err());
        assert_eq!(serde_json::to_string(&Unused).unwrap(), "[]");
    }
}
