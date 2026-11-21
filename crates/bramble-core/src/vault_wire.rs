//! The extension's vault schema v13 as it is written (`VaultV13`,
//! `packages/platform/vault/src/schema/schema.ts`), and v14's one more key (`VaultV14`'s optional
//! `accountProvenance`, `vault_provenance`): one serde type per record. Unknown keys are
//! refused (`hasOnlyKnownKeys`), every key the extension requires must be present, and an optional
//! key is either absent or holds a value: a `null` there is refused, as the extension's field
//! checks refuse it. Numbers are plain non-negative integer literals. Shapes only: the rules over
//! them are `vault_rules`, and `vault_v13` maps the records mobile operates onto `account_store`.
//!
//! Secret-bearing records (signing keys, imported phrases, viewing keys) zeroize on drop and have
//! no `Debug`.

use serde::{Deserialize, Deserializer, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::vault_provenance::Provenance;

/// An optional key: absent is `None`, and a present one must hold a value.
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    T::deserialize(d).map(Some)
}

/// A required key that may be `null`.
fn nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(d)
}

/// `AccountDerivation` (`wallet-model/src/accounts/derivations.ts`), every kind v13 holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub(crate) enum Derivation {
    /// `Bip44Derivation`; `seedId` names an imported phrase, absent for the vault's own.
    #[serde(rename = "starknet-bip44", rename_all = "camelCase")]
    StarknetBip44 {
        account_index: u64,
        address_index: u64,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        seed_id: Option<String>,
    },
    /// `ArgentXBip44Derivation` (Ready's ETH-seeded scheme).
    #[serde(rename = "argentx-bip44", rename_all = "camelCase")]
    ArgentxBip44 {
        address_index: u64,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        seed_id: Option<String>,
    },
    /// `ImportedKeyDerivation`: a pasted raw key.
    #[serde(rename = "imported-raw-key")]
    ImportedRawKey {},
    /// `LedgerDerivation`: a device account; the vault holds no key for it.
    #[serde(rename = "ledger", rename_all = "camelCase")]
    Ledger {
        account_index: u64,
        #[serde(rename = "eip2645application")]
        eip2645_application: String,
        path: String,
    },
}

/// `AccountRef` (`wallet-model/src/accounts/accounts.ts`). Public data only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Account {
    pub(crate) account_contract_type: String,
    pub(crate) address: String,
    pub(crate) class_hash: String,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) deployment_salt: Option<String>,
    pub(crate) derivation: Derivation,
    pub(crate) id: String,
    pub(crate) network_id: String,
    pub(crate) public_key_x: String,
    /// Present exactly for `starknet-bip44`.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) public_key_y: Option<String>,
}

/// `SigningKeyRef`.
#[derive(Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SigningKey {
    pub(crate) account_id: String,
    pub(crate) private_key: String,
}

/// `OriginGrant`: a site and the accounts it may see.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct OriginGrant {
    pub(crate) account_ids: Vec<String>,
    pub(crate) origin: String,
}

/// `ImportedSeedRef`: another wallet's phrase, retained for the accounts derived from it.
#[derive(Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(deny_unknown_fields)]
pub(crate) struct ImportedSeed {
    pub(crate) id: String,
    pub(crate) mnemonic: String,
}

/// `LedgerStrk20ViewingKeyRef`.
#[derive(Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LedgerViewingKey {
    pub(crate) chain_id: String,
    pub(crate) pool_address: String,
    pub(crate) scheme: String,
    pub(crate) viewing_key: String,
}

/// `AccountSecretRef`: a Ledger account's cached viewing keys.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AccountSecret {
    pub(crate) account_id: String,
    pub(crate) ledger_strk20_viewing_keys: Vec<LedgerViewingKey>,
}

/// `ShadowAccountScopeRef` (`vault/src/accounts/shadow-account-scopes.ts`).
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ShadowScope {
    pub(crate) account_id: String,
    pub(crate) anonymizer: String,
    pub(crate) chain_id: String,
    pub(crate) label: String,
    pub(crate) nonces: Vec<String>,
    pub(crate) origin: String,
    pub(crate) pool_address: String,
}

/// `MultisigStrk20ViewingKeyRef` (`vault/src/accounts/multisig-strk20-key.ts`).
#[derive(Clone, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MultisigKey {
    pub(crate) chain_id: String,
    pub(crate) multisig_id: String,
    pub(crate) pool_address: String,
    pub(crate) scheme: String,
    pub(crate) viewing_key: String,
}

/// `MultisigStrk20KeyConflict`.
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct MultisigConflict {
    pub(crate) chain_id: String,
    pub(crate) multisig_id: String,
    pub(crate) pool_address: String,
}

/// `VaultV13`, or `VaultV14` with `accountProvenance`, which a v13 vault never holds
/// (`vault_rules::is_valid`). The phrase zeroizes on drop; every secret record zeroizes itself.
#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Vault {
    #[zeroize(skip)]
    pub(crate) accounts: Vec<Account>,
    #[zeroize(skip)]
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) account_provenance: Option<Provenance>,
    #[zeroize(skip)]
    pub(crate) account_secrets: Vec<AccountSecret>,
    #[zeroize(skip)]
    #[serde(deserialize_with = "nullable")]
    pub(crate) active_account_id: Option<String>,
    #[zeroize(skip)]
    pub(crate) imported_seeds: Vec<ImportedSeed>,
    #[serde(deserialize_with = "nullable")]
    pub(crate) mnemonic: Option<String>,
    #[zeroize(skip)]
    pub(crate) multisig_strk20_key_conflicts: Vec<MultisigConflict>,
    #[zeroize(skip)]
    pub(crate) multisig_strk20_viewing_keys: Vec<MultisigKey>,
    #[zeroize(skip)]
    pub(crate) origin_grants: Vec<OriginGrant>,
    #[zeroize(skip)]
    pub(crate) schema_version: u32,
    #[zeroize(skip)]
    pub(crate) shadow_account_scopes: Vec<ShadowScope>,
    #[zeroize(skip)]
    pub(crate) signing_keys: Vec<SigningKey>,
}

/// Everything a v13 vault holds that mobile does not operate, kept exactly as read so that every
/// write puts it back: the account records mobile holds no usable key for (a Ledger account, one
/// derived from an imported phrase, one on the extension's devnet), their sealed keys, and the
/// collections mobile has no use for. `vault_v13` reads and writes it; outside it, only ids
/// (`account_ids`, so no new record takes one) and public addresses (`addresses_on`, for the
/// address-poisoning check) leave it, so no list, selection or signing path can reach a record.
#[derive(Clone, Default)]
pub(crate) struct Kept {
    /// Each record with the number of mobile's records the vault listed before it.
    pub(crate) accounts: Vec<(usize, Account)>,
    /// The kept records' sealed keys (a Ledger record has none).
    pub(crate) signing_keys: Vec<SigningKey>,
    /// Every sealed key's account id, in the vault's order.
    pub(crate) key_order: Vec<String>,
    pub(crate) account_secrets: Vec<AccountSecret>,
    pub(crate) imported_seeds: Vec<ImportedSeed>,
    pub(crate) multisig_strk20_key_conflicts: Vec<MultisigConflict>,
    pub(crate) multisig_strk20_viewing_keys: Vec<MultisigKey>,
    pub(crate) origin_grants: Vec<OriginGrant>,
    pub(crate) shadow_account_scopes: Vec<ShadowScope>,
}

impl Kept {
    /// The kept records' ids.
    pub(crate) fn account_ids(&self) -> impl Iterator<Item = &str> {
        self.accounts.iter().map(|(_, account)| account.id.as_str())
    }

    /// The kept records' public addresses on the network `network_id` names.
    pub(crate) fn addresses_on<'a>(&'a self, network_id: &'a str) -> impl Iterator<Item = &'a str> {
        self.accounts
            .iter()
            .filter(move |(_, account)| account.network_id == network_id)
            .map(|(_, account)| account.address.as_str())
    }
}
