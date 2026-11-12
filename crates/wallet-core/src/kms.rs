//! Key custody over Krusty KMS. Inputs are validated before they reach Krusty and outputs are
//! re-checked before they leave (the checks Bramble's `kms-krusty/adapter.ts` made in
//! TypeScript), so a Krusty regression surfaces as an error rather than as a wrong key.
//!
//! Secrets are borrowed in and come back zeroizing: phrases as `Zeroizing<String>`, private and
//! viewing keys as Krusty's `SecretFelt`, whose `Debug` is redacted. Turning a key into text is
//! the wallet's decision, made where it exports this crate across an FFI boundary.

use krusty_kms::OzAccountClassConfig;
use krusty_kms::STARKNET_COIN_TYPE;
use krusty_kms_common::{ChainId, SecretFelt};
use starknet_types_core::felt::Felt;
use zeroize::Zeroizing;

use crate::accounts::{family_address, AccountFamily, Ladder};
use crate::felt;
use crate::scrub::scrubbed;
use crate::Error;

/// Mnemonic lengths Krusty generates and the extension accepts.
pub const MNEMONIC_WORD_COUNTS: [u8; 5] = [12, 15, 18, 21, 24];
/// Resource bound on phrase text, in UTF-16 units (Bramble's `MAX_MNEMONIC_LENGTH`).
const MAX_MNEMONIC_TEXT_LENGTH: usize = 512;
/// Resource bound on a BIP-39 passphrase (Bramble's `MAX_MNEMONIC_PASSPHRASE_LENGTH`).
const MAX_BIP39_PASSPHRASE_LENGTH: usize = 1_024;

/// The networks Krusty publishes an OpenZeppelin account class for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkId {
    Mainnet,
    Sepolia,
}

impl From<NetworkId> for ChainId {
    fn from(network: NetworkId) -> Self {
        match network {
            NetworkId::Mainnet => ChainId::Mainnet,
            NetworkId::Sepolia => ChainId::Sepolia,
        }
    }
}

impl NetworkId {
    /// The chain id transaction hashes commit to, from Krusty and never from a node.
    pub fn chain_id(self) -> Felt {
        ChainId::from(self).as_felt()
    }

    /// The extension's network id (`SN_MAIN`, `SN_SEPOLIA`), used in account ids, token ids and
    /// stored records.
    pub fn catalog_id(self) -> &'static str {
        match self {
            Self::Mainnet => "SN_MAIN",
            Self::Sepolia => "SN_SEPOLIA",
        }
    }

    /// The network a catalog id names; `None` for any other spelling.
    pub fn from_catalog_id(value: &str) -> Option<Self> {
        [Self::Mainnet, Self::Sepolia]
            .into_iter()
            .find(|network| network.catalog_id() == value)
    }
}

/// Stark key material, as the extension's `StarkKeypair`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StarkKeypair {
    pub private_key: SecretFelt,
    pub public_key_x: String,
    pub public_key_y: String,
}

/// X-only Stark key material from Ready's ETH-seeded scheme (`StarkXOnlyKeypair`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StarkXOnlyKeypair {
    pub private_key: SecretFelt,
    pub public_key_x: String,
}

/// A Stark ECDSA signature over a caller-supplied hash (`StarkSignature`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StarkSignature {
    pub public_key: String,
    pub r: String,
    pub s: String,
}

/// Length in UTF-16 code units, the unit JavaScript's `String.length` counts.
pub fn utf16_length(value: &str) -> usize {
    value.encode_utf16().count()
}

/// `isMnemonicText`: non-blank and within the resource bound. Not a validity decision.
pub(crate) fn is_mnemonic_text(mnemonic: &str) -> bool {
    utf16_length(mnemonic) <= MAX_MNEMONIC_TEXT_LENGTH && !mnemonic.trim().is_empty()
}

pub(crate) fn check_derivation_input(
    mnemonic: &str,
    passphrase: Option<&str>,
) -> Result<(), Error> {
    if !is_mnemonic_text(mnemonic) {
        return Err(Error::invalid("invalid recovery phrase"));
    }
    if passphrase.is_some_and(|p| utf16_length(p) > MAX_BIP39_PASSPHRASE_LENGTH) {
        return Err(Error::invalid("passphrase too long"));
    }
    Ok(())
}

fn parse_felt(value: &str, reason: &str) -> Result<Felt, Error> {
    felt::parse(value).ok_or_else(|| Error::invalid(reason))
}

fn parse_non_zero(value: &str, reason: &str) -> Result<Felt, Error> {
    felt::parse_non_zero(value).ok_or_else(|| Error::invalid(reason))
}

fn checked_non_zero(value: Felt, operation: &str) -> Result<String, Error> {
    if value == Felt::ZERO {
        return Err(Error::kms(operation));
    }
    Ok(felt::canonical(&value))
}

fn checked_scalar(value: &Felt, operation: &str) -> Result<String, Error> {
    if !felt::is_private_key(value) {
        return Err(Error::kms(operation));
    }
    Ok(felt::canonical(value))
}

/// A caller's key, if it is a usable signing scalar.
fn usable_key<'a>(key: &'a SecretFelt, reason: &str) -> Result<&'a Felt, Error> {
    Some(key.expose_secret())
        .filter(|key| felt::is_private_key(key))
        .ok_or_else(|| Error::invalid(reason))
}

/// A key Krusty derived, if it is a usable signing scalar.
fn checked_secret(value: SecretFelt, operation: &str) -> Result<SecretFelt, Error> {
    if !felt::is_private_key(value.expose_secret()) {
        return Err(Error::kms(operation));
    }
    Ok(value)
}

/// Generates a fresh phrase from OS entropy.
pub fn generate_mnemonic(word_count: u8) -> Result<Zeroizing<String>, Error> {
    if !MNEMONIC_WORD_COUNTS.contains(&word_count) {
        return Err(Error::invalid("unsupported word count"));
    }
    scrubbed(|| {
        let phrase = Zeroizing::new(
            krusty_kms::generate_mnemonic(usize::from(word_count))
                .map_err(|_| Error::kms("generate"))?,
        );
        if !validate_mnemonic(&phrase) {
            return Err(Error::kms("generate"));
        }
        Ok(phrase)
    })
}

/// Whether text is a checksummed BIP-39 English phrase of a supported length. The text is judged
/// as given: the extension's import form normalises spacing and case before it gets here.
pub fn validate_mnemonic(mnemonic: &str) -> bool {
    if !is_mnemonic_text(mnemonic) {
        return false;
    }
    let words = mnemonic.split(' ').count();
    MNEMONIC_WORD_COUNTS
        .iter()
        .any(|count| usize::from(*count) == words)
        && scrubbed(|| krusty_kms::validate_mnemonic(mnemonic).is_ok())
}

/// The Starknet keypair at `m/44'/9004'/account_index'/0/address_index`, Krusty's argument order.
pub fn derive_starknet_keypair(
    mnemonic: &str,
    address_index: u32,
    account_index: u32,
    passphrase: Option<&str>,
) -> Result<StarkKeypair, Error> {
    check_derivation_input(mnemonic, passphrase)?;
    scrubbed(|| {
        let pair = krusty_kms::derive_keypair_with_coin_type(
            mnemonic,
            address_index,
            account_index,
            STARKNET_COIN_TYPE,
            passphrase,
        )
        .map_err(|_| Error::kms("derive"))?;
        let public = pair
            .public_key
            .to_affine()
            .map_err(|_| Error::kms("derive"))?;
        Ok(StarkKeypair {
            private_key: checked_secret(pair.private_key, "derive")?,
            public_key_x: checked_non_zero(public.x(), "derive")?,
            public_key_y: checked_non_zero(public.y(), "derive")?,
        })
    })
}

/// Ready (Argent X) legacy derivation: the Ethereum key at `m/44'/60'/0'/0/0` seeds
/// `m/44'/9004'/0'/0/address_index`. Ready fixes the hardened account field at zero and takes
/// no BIP-39 passphrase.
pub fn derive_argent_legacy_keypair(
    mnemonic: &str,
    address_index: u32,
) -> Result<StarkXOnlyKeypair, Error> {
    check_derivation_input(mnemonic, None)?;
    scrubbed(|| {
        // Krusty returns this key as a plain `Felt`; it moves straight into a `SecretFelt`.
        let key = checked_secret(
            SecretFelt::new(
                krusty_kms::derive_argent_legacy_private_key(mnemonic, address_index, 0)
                    .map_err(|_| Error::kms("derive"))?,
            ),
            "derive",
        )?;
        let public =
            krusty_kms::stark_public_key(key.expose_secret()).map_err(|_| Error::kms("derive"))?;
        Ok(StarkXOnlyKeypair {
            private_key: key,
            public_key_x: checked_non_zero(public, "derive")?,
        })
    })
}

/// The signing key of the account at `index` on `ladder`. Every account key goes through here,
/// so a record's ladder decides which tree its index walks.
pub fn account_key(
    mnemonic: &str,
    passphrase: Option<&str>,
    ladder: Ladder,
    index: u32,
) -> Result<SecretFelt, Error> {
    // Krusty's argument order is (address index, account index).
    scrubbed(|| {
        Ok(match ladder {
            Ladder::Account => derive_starknet_keypair(mnemonic, 0, index, passphrase)?.private_key,
            Ladder::Address => derive_starknet_keypair(mnemonic, index, 0, passphrase)?.private_key,
            Ladder::ArgentX if passphrase.is_some() => {
                return Err(Error::invalid("Ready accounts take no passphrase"))
            }
            Ladder::ArgentX => derive_argent_legacy_keypair(mnemonic, index)?.private_key,
        })
    })
}

/// The public key (x-coordinate) of `account_key`; the key itself is dropped inside the call.
pub fn account_public_key(
    mnemonic: &str,
    passphrase: Option<&str>,
    ladder: Ladder,
    index: u32,
) -> Result<Felt, Error> {
    scrubbed(|| {
        let key = account_key(mnemonic, passphrase, ladder, index)?;
        let public = krusty_kms::stark_public_key(key.expose_secret())
            .map_err(|_| Error::kms("public key"))?;
        if public == Felt::ZERO {
            return Err(Error::kms("public key"));
        }
        Ok(public)
    })
}

/// The counterfactual OpenZeppelin account address for a public key, class and deployment salt.
/// The salt is required: it is the wallet's choice, and a wrong default moves every address.
pub fn derive_oz_account_address(
    public_key_x: String,
    class_hash: String,
    salt: String,
) -> Result<String, Error> {
    let public_key = parse_felt(&public_key_x, "invalid address derivation input")?;
    let class_hash = parse_felt(&class_hash, "invalid address derivation input")?;
    let salt = parse_felt(&salt, "invalid address derivation input")?;
    // One placement for every family: `accounts::family_address`.
    let address = family_address(
        AccountFamily::OpenZeppelin,
        &class_hash,
        None,
        &salt,
        &public_key,
    )?;
    Ok(felt::canonical(&address))
}

/// The OpenZeppelin account class at manifest `version` (e.g. `"3.0.0"`) on `network`, from
/// Krusty's manifest. Pin an exact version: `latest` moves when Krusty does, and every address
/// with it.
pub fn oz_account_class_hash(network: NetworkId, version: &str) -> Result<String, Error> {
    let class_hash = OzAccountClassConfig::from_manifest(network.into(), version)
        .map_err(|_| Error::kms("class hash"))?
        .class_hash();
    checked_non_zero(class_hash, "class hash")
}

/// Deterministic RFC-6979 Stark ECDSA over `message_hash`.
pub fn sign_stark_hash(
    private_key: &SecretFelt,
    message_hash: &str,
) -> Result<StarkSignature, Error> {
    let key = usable_key(private_key, "invalid signing input")?;
    let hash = parse_felt(message_hash, "invalid signing input")?;
    let signature =
        scrubbed(|| krusty_kms::sign_stark_hash(key, &hash)).map_err(|_| Error::kms("sign"))?;
    Ok(StarkSignature {
        public_key: checked_non_zero(signature.public_key, "sign")?,
        r: checked_scalar(&signature.r, "sign")?,
        s: checked_scalar(&signature.s, "sign")?,
    })
}

/// The Stark public key (x-coordinate) of a signing scalar, such as a viewing key, so a wallet can
/// compare it with the key a privacy pool holds without the private key leaving Rust.
pub fn stark_public_key(private_key: &SecretFelt) -> Result<String, Error> {
    let key = usable_key(private_key, "invalid public-key input")?;
    let public =
        scrubbed(|| krusty_kms::stark_public_key(key)).map_err(|_| Error::kms("public key"))?;
    checked_non_zero(public, "public key")
}

/// The STRK20 viewing key scoped to `"<chain_id>:<pool_address>"`. Both scope values must be
/// canonical non-zero felts: the scope is hashed as text, and Krusty (0.11 in the extension
/// today, 2.0 here) refuses any other spelling. The result is a signing scalar too, so it is
/// held to the same range.
pub fn derive_strk20_viewing_key(
    private_key: &SecretFelt,
    chain_id: &str,
    pool_address: &str,
) -> Result<SecretFelt, Error> {
    let key = usable_key(private_key, "invalid viewing-key input")?;
    for scope in [chain_id, pool_address] {
        let parsed = parse_non_zero(scope, "invalid viewing-key input")?;
        if felt::canonical(&parsed) != scope {
            return Err(Error::invalid("invalid viewing-key input"));
        }
    }
    // Krusty returns the viewing key as a plain `Felt`; it moves straight into a `SecretFelt`.
    scrubbed(|| {
        let viewing_key = SecretFelt::new(
            krusty_kms::derive_scoped_strk20_viewing_key(key, chain_id, pool_address)
                .map_err(|_| Error::kms("viewing key"))?,
        );
        checked_secret(viewing_key, "viewing key")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHRASE: &str =
        "habit hope tip crystal because grunt nation idea electric witness alert like";

    #[test]
    fn mnemonic_validation_matches_the_adapter() {
        assert!(validate_mnemonic(PHRASE));
        assert!(!validate_mnemonic(&PHRASE.to_uppercase()));
        assert!(!validate_mnemonic(&format!("{PHRASE} like")));
        assert!(!validate_mnemonic(""));
        assert!(!validate_mnemonic("   "));
        assert!(!validate_mnemonic(&"a ".repeat(300)));
        for count in MNEMONIC_WORD_COUNTS {
            let phrase = generate_mnemonic(count).unwrap();
            assert_eq!(phrase.split(' ').count(), usize::from(count));
            assert!(validate_mnemonic(&phrase));
        }
        assert!(generate_mnemonic(13).is_err());
    }

    #[test]
    fn inputs_are_bounded() {
        assert!(check_derivation_input(PHRASE, Some(&"p".repeat(1_024))).is_ok());
        assert!(check_derivation_input(PHRASE, Some(&"p".repeat(1_025))).is_err());
        // JavaScript counts UTF-16 units: 1,024 two-byte characters are within the bound.
        assert!(check_derivation_input(PHRASE, Some(&"é".repeat(1_024))).is_ok());
        assert!(check_derivation_input(&"a".repeat(513), None).is_err());
        assert!(derive_oz_account_address("0xFF".into(), "0x1".into(), "0x0".into()).is_err());
        assert!(derive_oz_account_address("0x1".into(), "0x1".into(), "ff".into()).is_err());
        // A zero key or class places nothing.
        assert!(derive_oz_account_address("0x1".into(), "0x1".into(), "0x0".into()).is_ok());
        assert!(derive_oz_account_address("0x0".into(), "0x1".into(), "0x0".into()).is_err());
        assert!(derive_oz_account_address("0x1".into(), "0x0".into(), "0x0".into()).is_err());
    }

    #[test]
    fn raw_key_inputs_must_be_in_range() {
        let key = |hex: &str| SecretFelt::new(Felt::from_hex(hex).unwrap());
        let order = key("0x800000000000010ffffffffffffffffb781126dcae7b2321e66a241adc64d2f");
        let one = key("0x1");
        assert!(sign_stark_hash(&order, "0x1").is_err());
        assert!(sign_stark_hash(&key("0x0"), "0x1").is_err());
        assert!(sign_stark_hash(&one, "0x1").is_ok());
        assert!(sign_stark_hash(&one, "0x01").is_ok());
        assert!(sign_stark_hash(&one, "0xFF").is_err());
        let sepolia = "0x534e5f5345504f4c4941";
        assert!(derive_strk20_viewing_key(&one, "0x0", "0x123").is_err());
        assert!(derive_strk20_viewing_key(&one, sepolia, "0x0123").is_err());
        assert!(derive_strk20_viewing_key(&order, sepolia, "0x123").is_err());
        assert!(derive_strk20_viewing_key(&one, sepolia, "0x123").is_ok());
        assert!(derive_argent_legacy_keypair(PHRASE, 1 << 31).is_err());
    }

    #[test]
    fn ladders_step_different_fields_from_one_root() {
        let key = |ladder, index| account_public_key(PHRASE, None, ladder, index).unwrap();
        // Index 0 is `m/44'/9004'/0'/0/0` on both BIP-44 ladders; from 1 they part.
        assert_eq!(key(Ladder::Account, 0), key(Ladder::Address, 0));
        assert_ne!(key(Ladder::Account, 1), key(Ladder::Address, 1));
        assert_ne!(key(Ladder::Account, 0), key(Ladder::ArgentX, 0));
        assert!(account_key(PHRASE, Some("p"), Ladder::ArgentX, 0).is_err());
        assert!(account_key(PHRASE, Some("p"), Ladder::Address, 0).is_ok());
    }

    #[test]
    fn keys_are_redacted_in_debug_output() {
        let pair = derive_starknet_keypair(PHRASE, 0, 0, None).unwrap();
        let secret = felt::canonical(pair.private_key.expose_secret());
        assert!(!format!("{pair:?}").contains(&secret[2..]));
        let ready = derive_argent_legacy_keypair(PHRASE, 0).unwrap();
        let secret = felt::canonical(ready.private_key.expose_secret());
        assert!(!format!("{ready:?}").contains(&secret[2..]));
    }

    #[test]
    fn class_hashes_are_non_zero_per_network() {
        let main = oz_account_class_hash(NetworkId::Mainnet, "3.0.0").unwrap();
        let sepolia = oz_account_class_hash(NetworkId::Sepolia, "3.0.0").unwrap();
        assert!(felt::parse_non_zero(&main).is_some());
        assert!(felt::parse_non_zero(&sepolia).is_some());
    }
}
