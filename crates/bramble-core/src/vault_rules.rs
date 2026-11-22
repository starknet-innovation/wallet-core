//! The extension's v13 validator (`isVaultV13`, `packages/platform/vault/src/schema/schema.ts`
//! and what it imports), rule for rule, and v14's (`isVaultV14`: v13's over the rest, and
//! `validAccountProvenance`, `vault_provenance`), over the vault as written (`vault_wire`):
//! every record's grammar, the references between collections, and the bounds. `vault_v13` runs
//! it on every vault it opens and on every vault it is about to write, so mobile never accepts a
//! vault the extension refuses and never writes one. The JSON budget is `vault_budget`; serde's
//! shapes cover `hasOnlyKnownKeys` and the required keys. Mobile's stronger rules for the records
//! it operates (every one re-derived from its key) are `account_validate`'s, on top of these.

use std::collections::HashSet;
use std::hash::Hash;

use wallet_core::Felt;

use crate::account_catalog::{deployment_classes, AccountFamily};
use crate::account_names::{js_trim, utf16_len};
use crate::felt;
use crate::networks::NetworkId;
use crate::vault_provenance;
use crate::vault_v13::PROVENANCE_SCHEMA;
use crate::vault_wire::{
    Account, AccountSecret, Derivation, ImportedSeed, MultisigConflict, MultisigKey, OriginGrant,
    ShadowScope, SigningKey, Vault,
};

/// `STARKNET_NETWORKS.SN_DEVNET.chainId`: the short string `SN_DEVNET`. The extension knows a
/// local devnet mobile has no network for; its records are kept, never operated.
const DEVNET_CHAIN_ID: &str = "0x534e5f4445564e4554";
/// `Number.MAX_SAFE_INTEGER`: an index past it is not a safe integer (`isSafeIndex`).
const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;
/// `MAX_HARDENED_INDEX` (`ledger-path.ts`): a Ledger index stays below it.
const MAX_LEDGER_INDEX: u64 = 0x8000_0000;
/// `MAX_MNEMONIC_LENGTH` (`passphrase.ts`), in UTF-16 code units.
const MAX_MNEMONIC_LENGTH: usize = 512;
/// `MAX_LEDGER_STRK20_VIEWING_KEYS` (`account-secrets.ts`).
const MAX_LEDGER_VIEWING_KEYS: usize = 8;
const LEDGER_VIEWING_KEY_SCHEME: &str = "ledger-strk20-typed-data-v1";
/// `MAX_SHADOW_ACCOUNT_SCOPES` and `MAX_SHADOW_ACCOUNT_NONCES` (`shadow-account-scopes.ts`).
const MAX_SHADOW_SCOPES: usize = 128;
const MAX_SHADOW_NONCES: usize = 64;
/// `MAX_MULTISIG_STRK20_VIEWING_KEYS` (`multisig-strk20-key.ts`), for each list.
const MAX_MULTISIG_KEYS: usize = 32;
const MULTISIG_VIEWING_KEY_SCHEME: &str = "multisig-strk20-shared-v1";

/// `PATH_SCHEMES` (`ledger-path.ts`): each scheme's path around the decimal account index.
const LEDGER_PATHS: [(&str, &str, &str); 2] = [
    ("LedgerW", "m/2645'/1195502025'/734980059'/0'/", "'/0"),
    ("argentx", "m/2645'/1195502025'/1148870696'/0'/0'/", ""),
];

/// `STARKNET_NETWORKS[networkId].chainId`, canonical; `None` for an unknown network id.
pub(crate) fn chain_id(network_id: &str) -> Option<String> {
    if network_id == "SN_DEVNET" {
        return Some(DEVNET_CHAIN_ID.into());
    }
    NetworkId::from_catalog_id(network_id).map(|network| felt::canonical(&network.chain_id()))
}

fn is_canonical_non_zero(value: &str) -> bool {
    felt::parse_canonical_non_zero(value).is_some()
}

/// `canonicalFelt(value) === value` alone, which reads any `BigInt` and bounds nothing: `0x` and
/// minimal lowercase hex, of any length (a deployment salt is held to no more).
fn is_canonical_hex(value: &str) -> bool {
    value.strip_prefix("0x").is_some_and(|digits| {
        !digits.is_empty()
            && (digits == "0" || !digits.starts_with('0'))
            && digits
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

/// `isStarkPrivateKey` in its canonical spelling.
fn is_canonical_private_key(value: &str) -> bool {
    felt::parse_sealed_private_key(value).is_some()
}

/// `hasUniqueStrings`.
fn unique<T: Eq + Hash>(values: impl IntoIterator<Item = T>) -> bool {
    let mut seen = HashSet::new();
    values.into_iter().all(|value| seen.insert(value))
}

/// `isMnemonicText`: non-blank and at most 512 UTF-16 code units. Not a validity decision.
fn is_mnemonic_text(value: &str) -> bool {
    utf16_len(value) <= MAX_MNEMONIC_LENGTH && !js_trim(value).is_empty()
}

/// `isAccountId`: `<network>:<address>`, the network one the extension knows, the address a
/// canonical non-zero felt.
pub(crate) fn is_account_id(value: &str) -> bool {
    let mut parts = value.split(':');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(network), Some(address), None) => {
            chain_id(network).is_some() && is_canonical_non_zero(address)
        }
        _ => false,
    }
}

/// `isWalletOrigin`: what `new URL(origin).origin` prints back, over `https`, or `http` on
/// `127.0.0.1` or `localhost`. The shared hand-parsed endpoint grammar
/// (`relay_url::is_custom_rpc_url`) with no path or query: an origin it would respell is refused.
pub(crate) fn is_wallet_origin(value: &str) -> bool {
    let authority = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"));
    authority.is_some_and(|authority| !authority.contains(['/', '?', '#']))
        && wallet_core::nostr::relay_url::is_custom_rpc_url(value)
}

/// `isImportedSeedId`: `phrase-1` to `phrase-9999`.
fn is_seed_id(value: &str) -> bool {
    value.strip_prefix("phrase-").is_some_and(|digits| {
        (1..=4).contains(&digits.len())
            && !digits.starts_with('0')
            && digits.bytes().all(|b| b.is_ascii_digit())
    })
}

/// `isBip44Derivation`, `isArgentXBip44Derivation`, `isImportedKeyDerivation` and
/// `isLedgerDerivation` beyond their shapes.
fn is_derivation(derivation: &Derivation) -> bool {
    let seed_ok = |seed: &Option<String>| seed.as_deref().is_none_or(is_seed_id);
    match derivation {
        Derivation::StarknetBip44 {
            account_index,
            address_index,
            seed_id,
        } => {
            *account_index <= MAX_SAFE_INTEGER
                && *address_index <= MAX_SAFE_INTEGER
                && seed_ok(seed_id)
        }
        Derivation::ArgentxBip44 {
            address_index,
            seed_id,
        } => *address_index <= MAX_SAFE_INTEGER && seed_ok(seed_id),
        Derivation::ImportedRawKey {} => true,
        Derivation::Ledger {
            account_index,
            eip2645_application,
            path,
        } => {
            *account_index < MAX_LEDGER_INDEX
                && LEDGER_PATHS.iter().any(|(application, prefix, suffix)| {
                    application == eip2645_application
                        && *path == format!("{prefix}{account_index}{suffix}")
                })
        }
    }
}

/// `hasValidContractMetadata` at the current era: OpenZeppelin carries no salt; Argent and
/// Braavos carry a canonical one and a class their family can be deployed with.
fn has_contract_metadata(account: &Account) -> bool {
    let Some(family) = AccountFamily::from_wire(&account.account_contract_type) else {
        return false;
    };
    if family == AccountFamily::OpenZeppelin {
        return account.deployment_salt.is_none();
    }
    let class_ok = felt::parse(&account.class_hash).is_some_and(|class: Felt| {
        deployment_classes(family).any(|entry| entry.class_hash == class)
    });
    class_ok
        && account
            .deployment_salt
            .as_deref()
            .is_some_and(is_canonical_hex)
}

/// `isAccountRef`: contract metadata, the derivation, `publicKeyY` exactly for `starknet-bip44`,
/// and canonical non-zero felts with `id == <networkId>:<address>`.
fn is_account(account: &Account) -> bool {
    let y_ok = match (&account.derivation, &account.public_key_y) {
        (Derivation::StarknetBip44 { .. }, Some(y)) => is_canonical_non_zero(y),
        (Derivation::StarknetBip44 { .. }, None) | (_, Some(_)) => false,
        (_, None) => true,
    };
    has_contract_metadata(account)
        && is_derivation(&account.derivation)
        && y_ok
        && is_canonical_non_zero(&account.public_key_x)
        && is_canonical_non_zero(&account.address)
        && is_canonical_non_zero(&account.class_hash)
        && chain_id(&account.network_id).is_some()
        && account.id == format!("{}:{}", account.network_id, account.address)
}

pub(crate) fn is_ledger(account: &Account) -> bool {
    matches!(account.derivation, Derivation::Ledger { .. })
}

pub(crate) fn seed_id(account: &Account) -> Option<&str> {
    match &account.derivation {
        Derivation::StarknetBip44 { seed_id, .. } | Derivation::ArgentxBip44 { seed_id, .. } => {
            seed_id.as_deref()
        }
        _ => None,
    }
}

/// `hasValidVaultReferences` (current shape, canonical, Ledger allowed).
fn has_references(vault: &Vault) -> bool {
    let accounts = &vault.accounts;
    let held = |id: &str| accounts.iter().any(|account| account.id == id);
    let keys_ok = |keys: &[SigningKey]| {
        keys.iter().all(|key| {
            is_account_id(&key.account_id) && is_canonical_private_key(&key.private_key)
        }) && unique(keys.iter().map(|key| &key.account_id))
            // `hasValidKeyBindings`: a key per account but a Ledger one, which holds none.
            && keys.iter().all(|key| held(&key.account_id))
            && accounts.iter().all(|account| {
                keys.iter().any(|key| key.account_id == account.id) != is_ledger(account)
            })
    };
    let grants_ok = |grants: &[OriginGrant]| {
        grants.iter().all(|grant| {
            !grant.account_ids.is_empty()
                && grant
                    .account_ids
                    .iter()
                    .all(|id| is_account_id(id) && held(id))
                && unique(&grant.account_ids)
                && is_wallet_origin(&grant.origin)
        }) && unique(grants.iter().map(|grant| &grant.origin))
    };
    let active_ok = match &vault.active_account_id {
        None => accounts.is_empty(),
        Some(id) => is_account_id(id) && held(id),
    };
    accounts.iter().all(is_account)
        && unique(accounts.iter().map(|account| &account.id))
        && grants_ok(&vault.origin_grants)
        && keys_ok(&vault.signing_keys)
        && active_ok
}

/// `isAccountSecretRef` and `hasValidAccountSecretBindings`: a Ledger account's viewing keys,
/// each on that account's chain, one scope once.
fn has_account_secrets(accounts: &[Account], secrets: &[AccountSecret]) -> bool {
    secrets.iter().all(|secret| {
        let keys = &secret.ledger_strk20_viewing_keys;
        let account = accounts
            .iter()
            .find(|account| account.id == secret.account_id);
        let chain = account
            .filter(|account| is_ledger(account))
            .and_then(|account| chain_id(&account.network_id));
        (1..=MAX_LEDGER_VIEWING_KEYS).contains(&keys.len())
            && keys.iter().all(|key| {
                is_canonical_non_zero(&key.chain_id)
                    && is_canonical_non_zero(&key.pool_address)
                    && key.scheme == LEDGER_VIEWING_KEY_SCHEME
                    && is_canonical_private_key(&key.viewing_key)
                    && chain.as_deref() == Some(key.chain_id.as_str())
            })
            && unique(keys.iter().map(|key| (&key.chain_id, &key.pool_address)))
    }) && unique(secrets.iter().map(|secret| &secret.account_id))
}

/// `hasValidSeedBindings`: well-formed, uniquely named phrases, and every account naming one
/// finds it.
fn has_seed_bindings(accounts: &[Account], seeds: &[ImportedSeed]) -> bool {
    seeds
        .iter()
        .all(|seed| is_seed_id(&seed.id) && is_mnemonic_text(&seed.mnemonic))
        && unique(seeds.iter().map(|seed| &seed.id))
        && accounts
            .iter()
            .filter_map(seed_id)
            .all(|id| seeds.iter().any(|seed| seed.id == id))
}

/// `hasValidShadowAccountScopes`: canonical records, nonces strictly ascending (so unique), each
/// on a retained account's own chain, every scope once.
fn has_shadow_scopes(accounts: &[Account], scopes: &[ShadowScope]) -> bool {
    let is_label = |label: &str| {
        (1..=66).contains(&label.len()) && label.bytes().all(|b| (0x20..=0x7e).contains(&b))
    };
    let ascending = |nonces: &[String]| {
        let parsed: Option<Vec<Felt>> = nonces.iter().map(|n| felt::parse_canonical(n)).collect();
        parsed.is_some_and(|nonces| nonces.windows(2).all(|pair| pair[0] < pair[1]))
    };
    let on_own_chain = |scope: &ShadowScope| {
        accounts
            .iter()
            .find(|account| account.id == scope.account_id)
            .and_then(|account| chain_id(&account.network_id))
            .is_some_and(|chain| chain == scope.chain_id)
    };
    scopes.len() <= MAX_SHADOW_SCOPES
        && scopes.iter().all(|scope| {
            is_canonical_non_zero(&scope.anonymizer)
                && is_canonical_non_zero(&scope.chain_id)
                && is_label(&scope.label)
                && is_wallet_origin(&scope.origin)
                && is_canonical_non_zero(&scope.pool_address)
                && scope.nonces.len() <= MAX_SHADOW_NONCES
                && ascending(&scope.nonces)
                && on_own_chain(scope)
        })
        && unique(scopes.iter().map(|scope| {
            [
                scope.account_id.as_str(),
                &scope.chain_id,
                &scope.pool_address,
                &scope.anonymizer,
                &scope.origin,
                &scope.label,
            ]
            .join("|")
        }))
}

/// `canonicalMultisigStrk20Scope` on an already canonical scope: an account id for the multisig,
/// a canonical non-zero chain and pool, the chain the multisig's network's.
fn is_multisig_scope(multisig_id: &str, chain: &str, pool: &str) -> bool {
    let network = multisig_id.split(':').next().unwrap_or_default();
    is_account_id(multisig_id)
        && is_canonical_non_zero(chain)
        && is_canonical_non_zero(pool)
        && chain_id(network).as_deref() == Some(chain)
}

/// `scopeId`.
fn scope_id(multisig_id: &str, chain: &str, pool: &str) -> String {
    format!("{multisig_id}:{chain}:{pool}")
}

/// `hasValidMultisigStrk20VaultData`: bounded lists of canonical scopes, each sorted and unique
/// by scope, and a conflict only beside a key this vault holds.
fn has_multisig_keys(keys: &[MultisigKey], conflicts: &[MultisigConflict]) -> bool {
    let key_ids: Vec<String> = keys
        .iter()
        .map(|key| scope_id(&key.multisig_id, &key.chain_id, &key.pool_address))
        .collect();
    let conflict_ids: Vec<String> = conflicts
        .iter()
        .map(|c| scope_id(&c.multisig_id, &c.chain_id, &c.pool_address))
        .collect();
    // Every id is ASCII here, so byte order is JavaScript's string order.
    let sorted_unique = |ids: &[String]| ids.windows(2).all(|pair| pair[0] < pair[1]);
    keys.len() <= MAX_MULTISIG_KEYS
        && conflicts.len() <= MAX_MULTISIG_KEYS
        && keys.iter().all(|key| {
            key.scheme == MULTISIG_VIEWING_KEY_SCHEME
                && is_canonical_private_key(&key.viewing_key)
                && is_multisig_scope(&key.multisig_id, &key.chain_id, &key.pool_address)
        })
        && conflicts
            .iter()
            .all(|c| is_multisig_scope(&c.multisig_id, &c.chain_id, &c.pool_address))
        && sorted_unique(&key_ids)
        && sorted_unique(&conflict_ids)
        && conflict_ids.iter().all(|id| key_ids.contains(id))
}

/// `isVaultV13` or `isVaultV14`, but for the JSON budget (`vault_budget::within`).
pub(crate) fn is_valid(vault: &Vault) -> bool {
    let provenance = vault.account_provenance.as_ref();
    let version_ok = match vault.schema_version {
        13 => provenance.is_none(),
        PROVENANCE_SCHEMA => vault_provenance::is_valid(provenance, &vault.accounts),
        _ => false,
    };
    version_ok
        && vault.mnemonic.as_deref().is_none_or(is_mnemonic_text)
        && has_references(vault)
        && has_account_secrets(&vault.accounts, &vault.account_secrets)
        && has_seed_bindings(&vault.accounts, &vault.imported_seeds)
        && has_shadow_scopes(&vault.accounts, &vault.shadow_account_scopes)
        && has_multisig_keys(
            &vault.multisig_strk20_viewing_keys,
            &vault.multisig_strk20_key_conflicts,
        )
}

/// Whether mobile operates `account`: one of the vault's own phrase records or an imported key,
/// on a network mobile has. Every other record is kept unused (`vault_wire::Kept`).
pub(crate) fn is_operable(account: &Account) -> bool {
    NetworkId::from_catalog_id(&account.network_id).is_some()
        && !is_ledger(account)
        && seed_id(account).is_none()
}
