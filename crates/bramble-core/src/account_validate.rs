//! Checks on an unlocked vault's payload: each record's field grammar, the structural invariants
//! across records and preferences, and the re-derivation of every record from its key. Used on
//! open (`validate`) and before every write (`check_consistent`).

use std::collections::HashSet;

use wallet_core::Felt;

use crate::account_catalog::AccountFamily;
use crate::account_names::{is_label, js_trim};
use crate::account_store::{
    invalid_payload, ladder_fits, record_at, DerivedKey, KeySource, StoredAccount, VaultPayload,
    MAX_ACCOUNT_INDEX, MAX_ACCOUNT_RECORDS,
};
use crate::networks::NetworkId;
use crate::{felt, groups, kms, rpc_override, CoreError};

fn is_canonical_felt(value: &str, non_zero: bool) -> bool {
    felt::parse(value)
        .filter(|parsed| !non_zero || *parsed != Felt::ZERO)
        .is_some_and(|parsed| felt::canonical(&parsed) == value)
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
    let key_ok = match record.key {
        KeySource::Phrase(ladder) => ladder_fits(family, ladder),
        KeySource::Imported => record.account_index == 0,
    };
    record.account_index <= MAX_ACCOUNT_INDEX
        && key_ok
        && is_canonical_felt(&record.address, true)
        && is_canonical_felt(&record.public_key, true)
        && is_canonical_felt(&record.class_hash, true)
        && salt_ok
        && record.id == format!("{}:{}", network.catalog_id(), record.address)
}

// Every structural invariant, without derivation.
fn is_consistent(payload: &VaultPayload) -> bool {
    let ids: HashSet<&str> = payload.accounts.iter().map(|r| r.id.as_str()).collect();
    let imported = payload
        .accounts
        .iter()
        .filter(|record| record.key == KeySource::Imported);
    kms::validate_mnemonic(&payload.mnemonic)
        // One sealed key per imported record and none spare (ids are unique).
        && imported.clone().count() == payload.imported_keys.len()
        && imported
            .clone()
            .all(|record| payload.imported_keys.contains_key(&record.id))
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
        && payload
            .explorer_fallback_off
            .iter()
            .all(|network| NetworkId::from_catalog_id(network).is_some())
}

// Re-derives every record from its key (the phrase's, or the imported one the vault sealed), one
// key per record, each also shown to `sealed`; any difference is tampering or corruption.
fn rederives(
    payload: &VaultPayload,
    sealed: &mut dyn FnMut(&StoredAccount, &DerivedKey) -> bool,
) -> Result<bool, CoreError> {
    for record in &payload.accounts {
        let key = payload.signing_key(record)?;
        let Some(public_key) = felt::parse_non_zero(&key.0.public_key_x) else {
            return Ok(false);
        };
        let family = record.family();
        let class_hash = (family != AccountFamily::OpenZeppelin).then(|| record.class_felt());
        let expected = record_at(
            family,
            record.key,
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
