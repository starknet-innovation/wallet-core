//! The extension's stored shape for a shared account, `ArgentMultisigRecord`
//! (`packages/features/multisig/src/repository-schema.ts`), and the rules that validate it
//! (`isArgentMultisigRecord`), so a wallet's registry maps one to one onto the extension's. The
//! registry file itself (where it lives, how it is written, how many records it holds) is the
//! wallet's.

use serde::{Deserialize, Serialize};
use starknet_types_core::felt::Felt;

use super::multisig::{self, ArgentMultisigClass, ArgentMultisigConfig};
use crate::kms::{utf16_length, NetworkId};
use crate::{felt, Error};

/// `ArgentMultisigCurrentConfig`: GUID-ordered Stark keys and a threshold.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicConfig {
    pub signer_public_keys: Vec<String>,
    pub threshold: u32,
}

/// `ArgentMultisigDeployment`: the counterfactual constructor commitment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Deployment {
    pub salt: String,
    pub signer_public_keys: Vec<String>,
    pub threshold: u32,
}

/// `ArgentMultisigRecord`, field for field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Record {
    pub account_contract_type: String,
    pub address: String,
    pub class_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_config: Option<PublicConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deployment: Option<Deployment>,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub network_id: String,
}

/// Signer authority a record commits to: threshold and ascending signer GUIDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Authority {
    pub threshold: u32,
    pub signer_guids: Vec<String>,
}

fn is_canonical_felt(value: &str) -> bool {
    felt::parse(value).is_some_and(|f| felt::canonical(&f) == value)
}

fn is_canonical_non_zero(value: &str) -> bool {
    felt::parse_non_zero(value).is_some_and(|f| felt::canonical(&f) == value)
}

/// `isArgentMultisigCurrentConfig`: normalising must leave the keys exactly as stored.
fn normalized_guids(
    keys: &[String],
    threshold: u32,
    class: &ArgentMultisigClass,
) -> Option<Vec<String>> {
    let config = ArgentMultisigConfig {
        threshold,
        signer_public_keys: keys.to_vec(),
    };
    let (normalized, guids) = multisig::normalize(&config, class).ok()?;
    (normalized.signer_public_keys == keys).then(|| guids.iter().map(felt::canonical).collect())
}

/// `isArgentMultisigRecord`, for records of `class` whose name is at most `max_name_length`
/// UTF-16 units.
pub fn is_record(record: &Record, class: &ArgentMultisigClass, max_name_length: usize) -> bool {
    let name_ok = record
        .name
        .as_deref()
        .is_none_or(|name| (1..=max_name_length).contains(&utf16_length(name)));
    let deployment_ok = record.deployment.as_ref().is_none_or(|deployment| {
        is_canonical_felt(&deployment.salt)
            && normalized_guids(&deployment.signer_public_keys, deployment.threshold, class)
                .is_some()
            && multisig::predict(
                &ArgentMultisigConfig {
                    threshold: deployment.threshold,
                    signer_public_keys: deployment.signer_public_keys.clone(),
                },
                &deployment.salt,
                class,
            )
            .is_ok_and(|prediction| prediction.address == record.address)
    });
    record.account_contract_type == "argent-multisig"
        && is_canonical_non_zero(&record.address)
        && record.class_hash == class.class_hash()
        && NetworkId::from_catalog_id(&record.network_id).is_some()
        && record.id == format!("{}:{}", record.network_id, record.address)
        && name_ok
        && record.current_config.as_ref().is_none_or(|config| {
            normalized_guids(&config.signer_public_keys, config.threshold, class).is_some()
        })
        && deployment_ok
}

/// Ascending, canonical, unique, non-zero GUIDs with a threshold in `1..=n`, at most the class's
/// signer limit.
pub fn is_authority(signer_guids: &[String], threshold: u32, class: &ArgentMultisigClass) -> bool {
    let parsed: Option<Vec<Felt>> = signer_guids
        .iter()
        .map(|guid| {
            is_canonical_non_zero(guid)
                .then(|| felt::parse(guid))
                .flatten()
        })
        .collect();
    parsed.is_some_and(|guids| {
        !guids.is_empty()
            && guids.len() <= class.max_signers()
            && threshold >= 1
            && threshold as usize <= guids.len()
            && guids.windows(2).all(|pair| pair[0] < pair[1])
    })
}

/// The authority a record's keys commit to: `currentConfig`, else `deployment`. A wallet that
/// keeps authority from elsewhere (mobile's observed GUIDs) uses it only for a record with
/// neither; a record whose keys do not normalise has no authority.
pub fn authority(record: &Record, class: &ArgentMultisigClass) -> Option<Authority> {
    let from_keys = |keys: &[String], threshold: u32| {
        normalized_guids(keys, threshold, class).map(|signer_guids| Authority {
            threshold,
            signer_guids,
        })
    };
    if let Some(config) = &record.current_config {
        return from_keys(&config.signer_public_keys, config.threshold);
    }
    record
        .deployment
        .as_ref()
        .and_then(|deployment| from_keys(&deployment.signer_public_keys, deployment.threshold))
}

/// Builds a record's id and checks the address is canonical and non-zero.
pub fn record_id(network: NetworkId, address: &str) -> Result<String, Error> {
    if !is_canonical_non_zero(address) {
        return Err(Error::invalid("invalid multisig address"));
    }
    Ok(format!("{}:{address}", network.catalog_id()))
}
