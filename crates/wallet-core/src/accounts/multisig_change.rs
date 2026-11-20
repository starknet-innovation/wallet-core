//! Changing an Argent Multisig 0.2 account's signers, as the extension's
//! `packages/features/multisig/src/config-change.ts`: the change (`ArgentMultisigConfigChange`),
//! the self-call it becomes (`argentMultisigConfigChangeCall`), the strict parser of an untrusted
//! self-call (`parseArgentMultisigConfigChange`), the transition from a current configuration
//! (`nextArgentMultisigPublicConfig`) and the review of one (`reviewArgentMultisigConfigChange`).
//! Pinned by `extension-multisig-transactions-signers.json`.
//!
//! Refusals are typed; each one's `Display` is the extension's own sentence, which the fixture
//! checks. The signer limit comes in with the wallet's [`ArgentMultisigClass`].

use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Serialize};

use super::multisig::{self, ArgentMultisigClass, ArgentMultisigConfig};
use super::multisig_records::PublicConfig;
use crate::felt;
use crate::js::felt::{canonical_felt, is_non_zero_felt, same_felt};
use crate::send::multisig_invoke::{is_calls, MultisigCall};

/// `ArgentMultisigConfigChange`: one decoded signer-management operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ConfigChange {
    AddSigners {
        signer_public_keys: Vec<String>,
        threshold: u32,
    },
    RemoveSigners {
        signer_public_keys: Vec<String>,
        threshold: u32,
    },
    ChangeThreshold {
        threshold: u32,
    },
    ReplaceSigner {
        add_signer_public_key: String,
        remove_signer_public_key: String,
    },
}

/// `ArgentMultisigConfigReview`: the change, the configuration it leaves, and its summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigReview {
    pub change: ConfigChange,
    pub next_config: PublicConfig,
    pub summary: String,
}

/// Why a signer change was refused. `Display` is the extension's `WalletError` message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeError {
    InvalidSelfCall,
    UnsupportedSelfCall,
    InvalidCalldata,
    InvalidThreshold,
    InvalidSignerArray,
    NonStarknetSigner,
    InvalidSignerPublicKey,
    AlreadyConfigured,
    NotConfigured,
    RemovalNotUnique,
    ThresholdUnchanged,
    /// `normalizeArgentMultisigConfig`'s refusals of the configuration a change leaves.
    ThresholdOrSignerCount,
    ConfigSignerPublicKey,
    SignersNotUnique,
    SignerGuidsNotUnique,
}

impl ChangeError {
    pub fn message(self) -> &'static str {
        match self {
            Self::InvalidSelfCall => "Invalid Argent multisig management self-call",
            Self::UnsupportedSelfCall => "Unsupported Argent multisig self-call",
            Self::InvalidCalldata => "Invalid Argent multisig management calldata",
            Self::InvalidThreshold => "Invalid Argent multisig management threshold",
            Self::InvalidSignerArray => "Invalid Argent multisig management signer array",
            Self::NonStarknetSigner => {
                "Only Starknet signers are supported in Argent multisig management"
            }
            Self::InvalidSignerPublicKey => "Invalid Argent multisig signer public key",
            Self::AlreadyConfigured => "Argent multisig signer is already configured",
            Self::NotConfigured => "Argent multisig signer is not configured",
            Self::RemovalNotUnique => "Argent multisig removal signers must be unique",
            Self::ThresholdUnchanged => "Argent multisig threshold is unchanged",
            Self::ThresholdOrSignerCount => "Invalid Argent multisig threshold or signer count",
            Self::ConfigSignerPublicKey => "Invalid signer public key",
            Self::SignersNotUnique => "Argent multisig signers must be unique",
            Self::SignerGuidsNotUnique => "Argent multisig signer GUIDs must be unique",
        }
    }
}

impl fmt::Display for ChangeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for ChangeError {}

/// `canonicalSigner`: any `BigInt` spelling of a non-zero field element, canonicalised.
fn canonical_signer(value: &str) -> Result<String, ChangeError> {
    canonical_felt(value)
        .filter(|canonical| is_non_zero_felt(canonical))
        .ok_or(ChangeError::InvalidSignerPublicKey)
}

/// A canonical calldata felt as a small count, or `None` past `usize` (`Number.isSafeInteger`
/// fails long before any count this checks against).
fn small(value: &str) -> Option<usize> {
    felt::parse(value).and_then(|f| usize::try_from(f).ok())
}

fn signer_at(calldata: &[String], index: usize) -> Result<String, ChangeError> {
    if calldata.get(index).map(String::as_str) != Some("0x0") {
        return Err(ChangeError::NonStarknetSigner);
    }
    let key = calldata
        .get(index + 1)
        .ok_or(ChangeError::InvalidCalldata)?;
    canonical_signer(key)
}

fn threshold_at(
    calldata: &[String],
    index: usize,
    class: &ArgentMultisigClass,
) -> Result<u32, ChangeError> {
    let value = calldata.get(index).ok_or(ChangeError::InvalidCalldata)?;
    small(value)
        .filter(|threshold| (1..=class.max_signers()).contains(threshold))
        .and_then(|threshold| u32::try_from(threshold).ok())
        .ok_or(ChangeError::InvalidThreshold)
}

fn signer_array_at(
    calldata: &[String],
    index: usize,
    class: &ArgentMultisigClass,
) -> Result<Vec<String>, ChangeError> {
    let count = calldata.get(index).ok_or(ChangeError::InvalidCalldata)?;
    let count = small(count)
        .filter(|count| {
            (1..=class.max_signers()).contains(count) && calldata.len() == index + 1 + count * 2
        })
        .ok_or(ChangeError::InvalidSignerArray)?;
    (0..count)
        .map(|signer| signer_at(calldata, index + 1 + signer * 2))
        .collect()
}

/// `parseArgentMultisigConfigChange`: decodes one untrusted call against the exact stable-0.2
/// management ABI. Non-self, unknown, trailing, non-Stark or malformed calls are refused.
pub fn parse_config_change(
    account_address: &str,
    call: &MultisigCall,
    class: &ArgentMultisigClass,
) -> Result<ConfigChange, ChangeError> {
    if !is_calls(std::slice::from_ref(call)) || !same_felt(&call.contract_address, account_address)
    {
        return Err(ChangeError::InvalidSelfCall);
    }
    let calldata = &call.calldata;
    match call.entrypoint.as_str() {
        "change_threshold" => {
            if calldata.len() != 1 {
                return Err(ChangeError::InvalidCalldata);
            }
            Ok(ConfigChange::ChangeThreshold {
                threshold: threshold_at(calldata, 0, class)?,
            })
        }
        "replace_signer" => {
            if calldata.len() != 4 {
                return Err(ChangeError::InvalidCalldata);
            }
            // The extension reads the added signer first, so its refusal wins a tie.
            let add_signer_public_key = signer_at(calldata, 2)?;
            Ok(ConfigChange::ReplaceSigner {
                add_signer_public_key,
                remove_signer_public_key: signer_at(calldata, 0)?,
            })
        }
        entrypoint @ ("add_signers" | "remove_signers") => {
            // The signer array is read before the threshold, as the extension does.
            let signer_public_keys = signer_array_at(calldata, 1, class)?;
            let threshold = threshold_at(calldata, 0, class)?;
            Ok(if entrypoint == "add_signers" {
                ConfigChange::AddSigners {
                    signer_public_keys,
                    threshold,
                }
            } else {
                ConfigChange::RemoveSigners {
                    signer_public_keys,
                    threshold,
                }
            })
        }
        _ => Err(ChangeError::UnsupportedSelfCall),
    }
}

/// `argentMultisigConfigChangeCall`: the canonical self-call for `change`, parsed back through
/// [`parse_config_change`] before it is returned so no second reading of the ABI exists.
pub fn config_change_call(
    account_address: &str,
    change: &ConfigChange,
    class: &ArgentMultisigClass,
) -> Result<MultisigCall, ChangeError> {
    let signer = |key: &str| canonical_signer(key).map(|key| ["0x0".to_string(), key]);
    let count = |value: usize| format!("{value:#x}");
    let (entrypoint, calldata) = match change {
        ConfigChange::ChangeThreshold { threshold } => {
            ("change_threshold", vec![count(*threshold as usize)])
        }
        ConfigChange::ReplaceSigner {
            add_signer_public_key,
            remove_signer_public_key,
        } => {
            let mut calldata = signer(remove_signer_public_key)?.to_vec();
            calldata.extend(signer(add_signer_public_key)?);
            ("replace_signer", calldata)
        }
        ConfigChange::AddSigners {
            signer_public_keys,
            threshold,
        }
        | ConfigChange::RemoveSigners {
            signer_public_keys,
            threshold,
        } => {
            let mut calldata = vec![count(*threshold as usize), count(signer_public_keys.len())];
            for key in signer_public_keys {
                calldata.extend(signer(key)?);
            }
            let entrypoint = if matches!(change, ConfigChange::AddSigners { .. }) {
                "add_signers"
            } else {
                "remove_signers"
            };
            (entrypoint, calldata)
        }
    };
    let call = MultisigCall {
        calldata,
        contract_address: account_address.to_string(),
        entrypoint: entrypoint.to_string(),
    };
    parse_config_change(account_address, &call, class)?;
    Ok(call)
}

/// `normalizeArgentMultisigConfig` with the extension's refusals: the count and threshold, each
/// key (any `BigInt` spelling), then uniqueness; `multisig::normalize` then GUID-sorts.
fn normalized(
    signer_public_keys: Vec<String>,
    threshold: u32,
    class: &ArgentMultisigClass,
) -> Result<PublicConfig, ChangeError> {
    let count = signer_public_keys.len();
    if threshold < 1 || count < 1 || count > class.max_signers() || threshold as usize > count {
        return Err(ChangeError::ThresholdOrSignerCount);
    }
    let keys = signer_public_keys
        .iter()
        .map(|key| {
            canonical_felt(key)
                .filter(|canonical| is_non_zero_felt(canonical))
                .ok_or(ChangeError::ConfigSignerPublicKey)
        })
        .collect::<Result<Vec<String>, ChangeError>>()?;
    if keys.iter().collect::<HashSet<_>>().len() != count {
        return Err(ChangeError::SignersNotUnique);
    }
    // Past the checks above, the only refusal left is two keys sharing a GUID (or a zero GUID,
    // which would be a Poseidon preimage).
    let (config, _) = multisig::normalize(
        &ArgentMultisigConfig {
            threshold,
            signer_public_keys: keys,
        },
        class,
    )
    .map_err(|_| ChangeError::SignerGuidsNotUnique)?;
    Ok(PublicConfig {
        signer_public_keys: config.signer_public_keys,
        threshold,
    })
}

/// `nextArgentMultisigPublicConfig`: the configuration `change` leaves `current` in. No-ops,
/// missing or existing members and invalid resulting thresholds are refused. Membership compares
/// spellings exactly, as the extension does; a parsed change is always canonical.
pub fn next_public_config(
    current: &PublicConfig,
    change: &ConfigChange,
    class: &ArgentMultisigClass,
) -> Result<PublicConfig, ChangeError> {
    let current = normalized(current.signer_public_keys.clone(), current.threshold, class)?;
    let configured = |key: &String| current.signer_public_keys.contains(key);
    match change {
        ConfigChange::ChangeThreshold { threshold } => {
            if *threshold == current.threshold {
                return Err(ChangeError::ThresholdUnchanged);
            }
            normalized(current.signer_public_keys.clone(), *threshold, class)
        }
        ConfigChange::AddSigners {
            signer_public_keys,
            threshold,
        } => {
            if signer_public_keys.iter().any(configured) {
                return Err(ChangeError::AlreadyConfigured);
            }
            let mut signers = current.signer_public_keys.clone();
            signers.extend(signer_public_keys.iter().cloned());
            normalized(signers, *threshold, class)
        }
        ConfigChange::RemoveSigners {
            signer_public_keys,
            threshold,
        } => {
            let removed: HashSet<&String> = signer_public_keys.iter().collect();
            if removed.len() != signer_public_keys.len() {
                return Err(ChangeError::RemovalNotUnique);
            }
            if !signer_public_keys.iter().all(configured) {
                return Err(ChangeError::NotConfigured);
            }
            let signers = current
                .signer_public_keys
                .iter()
                .filter(|key| !removed.contains(key))
                .cloned()
                .collect();
            normalized(signers, *threshold, class)
        }
        ConfigChange::ReplaceSigner {
            add_signer_public_key,
            remove_signer_public_key,
        } => {
            let index = current
                .signer_public_keys
                .iter()
                .position(|key| key == remove_signer_public_key)
                .ok_or(ChangeError::NotConfigured)?;
            if configured(add_signer_public_key) {
                return Err(ChangeError::AlreadyConfigured);
            }
            let mut signers = current.signer_public_keys.clone();
            signers[index] = add_signer_public_key.clone();
            normalized(signers, current.threshold, class)
        }
    }
}

/// `configChangeSummary`: the extension's display line for a change.
pub fn config_change_summary(change: &ConfigChange) -> String {
    match change {
        ConfigChange::ChangeThreshold { threshold } => {
            format!("Change Argent multisig threshold to {threshold}")
        }
        ConfigChange::ReplaceSigner { .. } => "Replace one Argent multisig signer".to_string(),
        ConfigChange::AddSigners {
            signer_public_keys,
            threshold,
        } => format!(
            "Add {} Argent multisig signer(s) and set threshold to {threshold}",
            signer_public_keys.len()
        ),
        ConfigChange::RemoveSigners {
            signer_public_keys,
            threshold,
        } => format!(
            "Remove {} Argent multisig signer(s) and set threshold to {threshold}",
            signer_public_keys.len()
        ),
    }
}

/// `reviewArgentMultisigConfigChange`: the change an untrusted self-call makes to the trusted
/// `current` configuration.
pub fn review_config_change(
    account_address: &str,
    current: &PublicConfig,
    call: &MultisigCall,
    class: &ArgentMultisigClass,
) -> Result<ConfigReview, ChangeError> {
    let change = parse_config_change(account_address, call, class)?;
    Ok(ConfigReview {
        next_config: next_public_config(current, &change, class)?,
        summary: config_change_summary(&change),
        change,
    })
}

#[cfg(test)]
#[path = "multisig_change_tests.rs"]
mod tests;
