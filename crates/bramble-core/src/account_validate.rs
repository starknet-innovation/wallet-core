//! Checks on an unlocked vault's payload: each record's field grammar (`is_well_formed`, which
//! `vault_admit` applies to each record before it re-derives it) and the structural invariants
//! across records and preferences (`check_consistent`, on open and before every write).

use std::collections::HashSet;

use wallet_core::Felt;

use crate::account_catalog::AccountFamily;
use crate::account_names::{is_label, js_trim};
use crate::account_store::{
    invalid_payload, ladder_fits, KeySource, StoredAccount, VaultPayload, MAX_ACCOUNT_INDEX,
    MAX_ACCOUNT_RECORDS,
};
use crate::kms::Ladder;
use crate::networks::NetworkId;
use crate::{felt, groups, kms, rpc_override, CoreError};

fn is_canonical_felt(value: &str, non_zero: bool) -> bool {
    felt::parse(value)
        .filter(|parsed| !non_zero || *parsed != Felt::ZERO)
        .is_some_and(|parsed| felt::canonical(&parsed) == value)
}

/// Field grammar of one record, before any derivation.
pub(crate) fn is_well_formed(record: &StoredAccount) -> bool {
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

// Every structural invariant, without derivation. Ids are unique across every record the vault
// holds, kept ones included, and the active account may be a kept one.
fn is_consistent(payload: &VaultPayload) -> bool {
    let ids: HashSet<&str> = payload.accounts.iter().map(|r| r.id.as_str()).collect();
    let all: HashSet<&str> = ids
        .iter()
        .copied()
        .chain(payload.kept.account_ids())
        .collect();
    // A held key belongs to an operated imported or Ready record, and every imported one has one.
    let held = |record: &StoredAccount| payload.held_keys.contains_key(&record.id);
    let holders = payload
        .accounts
        .iter()
        .filter(|record| held(record))
        .count();
    kms::validate_mnemonic(&payload.mnemonic)
        && holders == payload.held_keys.len()
        && payload.accounts.iter().all(|record| match record.key {
            KeySource::Imported => held(record),
            KeySource::Phrase(Ladder::ArgentX) => true,
            KeySource::Phrase(_) => !held(record),
        })
        && payload.accounts.len() <= MAX_ACCOUNT_RECORDS
        && all.len() == payload.record_count()
        && payload.accounts.iter().all(is_well_formed)
        && match &payload.active_account_id {
            None => payload.record_count() == 0,
            Some(id) => all.contains(id.as_str()),
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

/// Checks every structural invariant without re-deriving: on open, after `vault_admit` has
/// re-derived each operated record, and before every write, whose new records were derived by
/// this core in the same call.
pub(crate) fn check_consistent(payload: &VaultPayload) -> Result<(), CoreError> {
    if is_consistent(payload) {
        Ok(())
    } else {
        Err(invalid_payload())
    }
}
