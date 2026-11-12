//! Argent Multisig 0.2 (the extension's "Ready Multisig"), mirroring
//! `packages/platform/starknet/src/accounts/account-contracts.ts`:
//! signer GUIDs, the GUID-sorted configuration, constructor calldata, and the counterfactual
//! address. Krusty 2.0.0 has no Argent-multisig descriptor (its `OpenZeppelinMultisig` is a
//! different contract whose addresses never match), so this composes Krusty's public
//! `calculate_contract_address` and `encode_short_string` with the 2-to-1 Poseidon hash from
//! `starknet-types-core`, the same primitive library Krusty uses. These are public hash functions,
//! not key handling. The formula is pinned to the extension by parity fixtures.
//!
//! Which class a wallet deploys, and its signer limit, are the wallet's: they come in as an
//! [`ArgentMultisigClass`] (Bramble's is `generated/multisig.json`'s `argentMultisig.classHash`
//! and `limits.signers`).

use starknet_types_core::felt::Felt;
use starknet_types_core::hash::{Poseidon, StarkHash};

use crate::{felt, Error};

/// The multisig class a wallet pins, and the most signers a configuration may have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgentMultisigClass {
    spelling: String,
    hash: Felt,
    max_signers: usize,
}

impl ArgentMultisigClass {
    /// `class_hash` is kept exactly as spelled (the extension stores and sends it padded), and
    /// must be a non-zero felt; `max_signers` must be at least one.
    pub fn new(class_hash: &str, max_signers: usize) -> Result<Self, Error> {
        let hash = felt::parse_non_zero(class_hash)
            .ok_or_else(|| Error::invalid("invalid multisig class hash"))?;
        if max_signers == 0 {
            return Err(Error::invalid("invalid multisig signer limit"));
        }
        Ok(Self {
            spelling: class_hash.to_string(),
            hash,
            max_signers,
        })
    }

    /// The class hash, spelled exactly as given.
    pub fn class_hash(&self) -> &str {
        &self.spelling
    }

    /// The class hash as a felt.
    pub fn hash(&self) -> Felt {
        self.hash
    }

    /// The most signers a configuration of this class may have.
    pub fn max_signers(&self) -> usize {
        self.max_signers
    }
}

/// A signer set and threshold in the extension's normalised form: canonical keys, GUID-sorted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgentMultisigConfig {
    pub threshold: u32,
    pub signer_public_keys: Vec<String>,
}

/// The counterfactual deployment of one configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgentMultisigPrediction {
    pub address: String,
    pub class_hash: String,
    pub salt: String,
    pub constructor_calldata: Vec<String>,
    pub config: ArgentMultisigConfig,
    pub signer_guids: Vec<String>,
}

/// `argentSignerGuid`: `poseidon(short_string("Starknet Signer"), publicKey)`.
pub fn signer_guid(public_key: &Felt) -> Result<Felt, Error> {
    if *public_key == Felt::ZERO {
        return Err(Error::invalid("invalid signer public key"));
    }
    let prefix = krusty_kms::encode_short_string("Starknet Signer")
        .map_err(|_| Error::kms("signer guid"))?;
    let guid = Poseidon::hash(&prefix, public_key);
    if guid == Felt::ZERO {
        return Err(Error::invalid("invalid signer guid"));
    }
    Ok(guid)
}

/// `argentSignerGuid` over the extension's felt spelling.
pub fn signer_guid_hex(public_key: &str) -> Result<String, Error> {
    let key = felt::parse_non_zero(public_key)
        .ok_or_else(|| Error::invalid("invalid signer public key"))?;
    Ok(felt::canonical(&signer_guid(&key)?))
}

/// `normalizeArgentMultisigConfig`: 1..=`max_signers` unique non-zero keys (and unique GUIDs), a
/// threshold in `1..=n`, keys canonicalised and sorted by GUID. Returns the GUIDs in that order.
pub fn normalize(
    config: &ArgentMultisigConfig,
    class: &ArgentMultisigClass,
) -> Result<(ArgentMultisigConfig, Vec<Felt>), Error> {
    let count = config.signer_public_keys.len();
    if config.threshold < 1
        || count < 1
        || count > class.max_signers
        || config.threshold as usize > count
    {
        return Err(Error::invalid("invalid multisig threshold or signer count"));
    }
    let mut signers = config
        .signer_public_keys
        .iter()
        .map(|key| {
            let key = felt::parse_non_zero(key)
                .ok_or_else(|| Error::invalid("invalid signer public key"))?;
            Ok((signer_guid(&key)?, key))
        })
        .collect::<Result<Vec<(Felt, Felt)>, Error>>()?;
    signers.sort();
    if signers.windows(2).any(|pair| pair[0].1 == pair[1].1) {
        return Err(Error::invalid("multisig signers must be unique"));
    }
    if signers.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err(Error::invalid("multisig signer guids must be unique"));
    }
    let guids = signers.iter().map(|(guid, _)| *guid).collect();
    Ok((
        ArgentMultisigConfig {
            threshold: config.threshold,
            signer_public_keys: signers
                .iter()
                .map(|(_, key)| felt::canonical(key))
                .collect(),
        },
        guids,
    ))
}

/// `argentMultisigConstructorCalldata`: `[threshold, n, 0, pk_0, 0, pk_1, …]` (decimal counts).
pub fn constructor_calldata(
    config: &ArgentMultisigConfig,
    class: &ArgentMultisigClass,
) -> Result<Vec<String>, Error> {
    let (normalized, _) = normalize(config, class)?;
    let mut calldata = vec![
        normalized.threshold.to_string(),
        normalized.signer_public_keys.len().to_string(),
    ];
    for key in &normalized.signer_public_keys {
        calldata.push("0".into());
        calldata.push(key.clone());
    }
    Ok(calldata)
}

fn calldata_felt(value: &str) -> Felt {
    if value.starts_with("0x") {
        felt::parse(value).expect("calldata keys are canonical felts")
    } else {
        Felt::from(value.parse::<u64>().expect("calldata counts are decimal"))
    }
}

/// `argentMultisigAddress` plus everything the Fund and Deploy review shows.
pub fn predict(
    config: &ArgentMultisigConfig,
    salt: &str,
    class: &ArgentMultisigClass,
) -> Result<ArgentMultisigPrediction, Error> {
    let salt = felt::parse(salt).ok_or_else(|| Error::invalid("invalid deployment salt"))?;
    let (normalized, guids) = normalize(config, class)?;
    let calldata = constructor_calldata(&normalized, class)?;
    let felts: Vec<Felt> = calldata.iter().map(|value| calldata_felt(value)).collect();
    // Krusty already reduces the address below 2**251 - 256, as starknet.js does.
    let address = krusty_kms::calculate_contract_address(&salt, &class.hash, &felts, &Felt::ZERO)
        .map_err(|_| Error::kms("multisig address"))?;
    if address == Felt::ZERO {
        return Err(Error::kms("multisig address"));
    }
    Ok(ArgentMultisigPrediction {
        address: felt::canonical(&address),
        class_hash: class.spelling.clone(),
        salt: felt::canonical(&salt),
        constructor_calldata: calldata,
        config: normalized,
        signer_guids: guids.iter().map(felt::canonical).collect(),
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Bramble's pinned class and limit at the time of the port; any class serves these tests.
    pub(crate) fn class() -> ArgentMultisigClass {
        ArgentMultisigClass::new(
            "0x07aeca3456816e3b833506d7cc5c1313d371fbdb0ae95ee70af72a4ddbf42594",
            32,
        )
        .unwrap()
    }

    fn config(threshold: u32, keys: &[&str]) -> ArgentMultisigConfig {
        ArgentMultisigConfig {
            threshold,
            signer_public_keys: keys.iter().map(|k| k.to_string()).collect(),
        }
    }

    #[test]
    fn rejects_what_the_extension_rejects() {
        for bad in [
            config(0, &["0x1"]),
            config(2, &["0x1"]),
            config(1, &[]),
            config(1, &["0x1", "0x1"]),
            config(1, &["0x0"]),
            config(1, &["0xABC"]),
        ] {
            assert!(normalize(&bad, &class()).is_err(), "{bad:?}");
        }
        let too_many: Vec<String> = (1..=33).map(|n| format!("0x{n:x}")).collect();
        let refs: Vec<&str> = too_many.iter().map(String::as_str).collect();
        assert!(normalize(&config(1, &refs), &class()).is_err());
        assert!(normalize(&config(1, &refs[..32]), &class()).is_ok());
    }

    #[test]
    fn signers_sort_by_guid_and_calldata_uses_decimal_counts() {
        let (normalized, guids) = normalize(&config(2, &["0x456", "0x123"]), &class()).unwrap();
        assert!(guids[0] < guids[1]);
        let calldata = constructor_calldata(&normalized, &class()).unwrap();
        assert_eq!(calldata[..3], ["2".to_string(), "2".into(), "0".into()]);
        assert_eq!(calldata.len(), 6);
        let prediction = predict(&normalized, "0x0", &class()).unwrap();
        assert_eq!(prediction.salt, "0x0");
        assert_eq!(prediction.class_hash, class().class_hash());
        assert!(felt::parse_non_zero(&prediction.address).is_some());
    }

    #[test]
    fn the_class_is_a_non_zero_felt_with_a_signer_limit() {
        assert!(ArgentMultisigClass::new("0x0", 32).is_err());
        assert!(ArgentMultisigClass::new("0xABC", 32).is_err());
        assert!(ArgentMultisigClass::new("0x1", 0).is_err());
        let limited = ArgentMultisigClass::new("0x1", 1).unwrap();
        assert!(normalize(&config(1, &["0x1", "0x2"]), &limited).is_err());
    }
}
