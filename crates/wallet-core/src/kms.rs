//! Key custody over Krusty KMS. Inputs are validated before they reach Krusty and outputs are
//! re-checked before they leave (the checks Bramble's `kms-krusty/adapter.ts` made in
//! TypeScript), so a Krusty regression surfaces as an error rather than as a wrong key.
//!
//! Private keys are canonical hex strings. Whether they may leave the process is the wallet's
//! decision, made where it exports this crate across an FFI boundary.

use krusty_kms::OzAccountClassConfig;
use krusty_kms::STARKNET_COIN_TYPE;
use krusty_kms_common::ChainId;
use starknet_types_core::felt::Felt;
use zeroize::Zeroizing;

use crate::felt;
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

/// Stark key material, as the extension's `StarkKeypair`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StarkKeypair {
    pub private_key: String,
    pub public_key_x: String,
    pub public_key_y: String,
}

/// X-only Stark key material from Ready's ETH-seeded scheme (`StarkXOnlyKeypair`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StarkXOnlyKeypair {
    pub private_key: String,
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
pub(crate) fn utf16_length(value: &str) -> usize {
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

/// Moves phrase text and passphrase into buffers wiped on drop.
pub(crate) fn secret_inputs(
    mnemonic: String,
    passphrase: Option<String>,
) -> (Zeroizing<String>, Option<Zeroizing<String>>) {
    (Zeroizing::new(mnemonic), passphrase.map(Zeroizing::new))
}

fn parse_felt(value: &str, reason: &str) -> Result<Felt, Error> {
    felt::parse(value).ok_or_else(|| Error::invalid(reason))
}

fn parse_non_zero(value: &str, reason: &str) -> Result<Felt, Error> {
    felt::parse_non_zero(value).ok_or_else(|| Error::invalid(reason))
}

fn parse_private_key(value: &str, reason: &str) -> Result<Felt, Error> {
    felt::parse(value)
        .filter(felt::is_private_key)
        .ok_or_else(|| Error::invalid(reason))
}

fn checked_non_zero(value: Felt, operation: &str) -> Result<String, Error> {
    if value == Felt::ZERO {
        return Err(Error::kms(operation));
    }
    Ok(felt::canonical(&value))
}

fn checked_private_key(value: &Felt, operation: &str) -> Result<String, Error> {
    if !felt::is_private_key(value) {
        return Err(Error::kms(operation));
    }
    Ok(felt::canonical(value))
}

/// Generates a fresh phrase from OS entropy.
pub fn generate_mnemonic(word_count: u8) -> Result<String, Error> {
    if !MNEMONIC_WORD_COUNTS.contains(&word_count) {
        return Err(Error::invalid("unsupported word count"));
    }
    let phrase = Zeroizing::new(
        krusty_kms::generate_mnemonic(usize::from(word_count))
            .map_err(|_| Error::kms("generate"))?,
    );
    if !validate_mnemonic(phrase.to_string()) {
        return Err(Error::kms("generate"));
    }
    // The copy that crosses the boundary is the host's to wipe; this one is wiped on drop.
    Ok(phrase.to_string())
}

/// Whether text is a checksummed BIP-39 English phrase of a supported length. The text is judged
/// as given: the extension's import form normalises spacing and case before it gets here.
pub fn validate_mnemonic(mnemonic: String) -> bool {
    let mnemonic = Zeroizing::new(mnemonic);
    if !is_mnemonic_text(&mnemonic) {
        return false;
    }
    let words = mnemonic.split(' ').count();
    MNEMONIC_WORD_COUNTS
        .iter()
        .any(|count| usize::from(*count) == words)
        && krusty_kms::validate_mnemonic(&mnemonic).is_ok()
}

/// The Starknet keypair at `m/44'/9004'/account_index'/0/address_index`, Krusty's argument order.
pub fn derive_starknet_keypair(
    mnemonic: String,
    address_index: u32,
    account_index: u32,
    passphrase: Option<String>,
) -> Result<StarkKeypair, Error> {
    let (mnemonic, passphrase) = secret_inputs(mnemonic, passphrase);
    let passphrase = passphrase.as_ref().map(|p| p.as_str());
    check_derivation_input(&mnemonic, passphrase)?;
    let pair = krusty_kms::derive_keypair_with_coin_type(
        &mnemonic,
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
        private_key: checked_private_key(pair.private_key.expose_secret(), "derive")?,
        public_key_x: checked_non_zero(public.x(), "derive")?,
        public_key_y: checked_non_zero(public.y(), "derive")?,
    })
}

/// Ready (Argent X) legacy derivation: the Ethereum key at `m/44'/60'/0'/0/0` seeds
/// `m/44'/9004'/0'/0/address_index`. Ready fixes the hardened account field at zero and takes
/// no BIP-39 passphrase.
pub fn derive_argent_legacy_keypair(
    mnemonic: String,
    address_index: u32,
) -> Result<StarkXOnlyKeypair, Error> {
    let mnemonic = Zeroizing::new(mnemonic);
    check_derivation_input(&mnemonic, None)?;
    let key = krusty_kms::derive_argent_legacy_private_key(&mnemonic, address_index, 0)
        .map_err(|_| Error::kms("derive"))?;
    let public = krusty_kms::stark_public_key(&key).map_err(|_| Error::kms("derive"))?;
    Ok(StarkXOnlyKeypair {
        private_key: checked_private_key(&key, "derive")?,
        public_key_x: checked_non_zero(public, "derive")?,
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
    let address = krusty_kms::derive_oz_account_address(&public_key, &class_hash, Some(&salt))
        .map_err(|_| Error::kms("address"))?;
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
pub fn sign_stark_hash(private_key: String, message_hash: String) -> Result<StarkSignature, Error> {
    let key = parse_private_key(&private_key, "invalid signing input")?;
    let hash = parse_felt(&message_hash, "invalid signing input")?;
    let signature = krusty_kms::sign_stark_hash(&key, &hash).map_err(|_| Error::kms("sign"))?;
    Ok(StarkSignature {
        public_key: checked_non_zero(signature.public_key, "sign")?,
        r: checked_private_key(&signature.r, "sign")?,
        s: checked_private_key(&signature.s, "sign")?,
    })
}

/// The STRK20 viewing key scoped to `"<chain_id>:<pool_address>"`. Both scope values must be
/// canonical non-zero felts: the scope is hashed as text, and Krusty (0.11 in the extension
/// today, 2.0 here) refuses any other spelling. The result is a signing scalar too, so it is
/// held to the same range.
pub fn derive_strk20_viewing_key(
    private_key: String,
    chain_id: String,
    pool_address: String,
) -> Result<String, Error> {
    let key = parse_private_key(&private_key, "invalid viewing-key input")?;
    for scope in [&chain_id, &pool_address] {
        let parsed = parse_non_zero(scope, "invalid viewing-key input")?;
        if felt::canonical(&parsed) != *scope {
            return Err(Error::invalid("invalid viewing-key input"));
        }
    }
    let viewing_key = krusty_kms::derive_scoped_strk20_viewing_key(&key, &chain_id, &pool_address)
        .map_err(|_| Error::kms("viewing key"))?;
    checked_private_key(&viewing_key, "viewing key")
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHRASE: &str =
        "habit hope tip crystal because grunt nation idea electric witness alert like";

    #[test]
    fn mnemonic_validation_matches_the_adapter() {
        assert!(validate_mnemonic(PHRASE.into()));
        assert!(!validate_mnemonic(PHRASE.to_uppercase()));
        assert!(!validate_mnemonic(format!("{PHRASE} like")));
        assert!(!validate_mnemonic(String::new()));
        assert!(!validate_mnemonic("   ".into()));
        assert!(!validate_mnemonic("a ".repeat(300)));
        for count in MNEMONIC_WORD_COUNTS {
            let phrase = generate_mnemonic(count).unwrap();
            assert_eq!(phrase.split(' ').count(), usize::from(count));
            assert!(validate_mnemonic(phrase));
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
    }

    #[test]
    fn raw_key_inputs_must_be_in_range() {
        let order = "0x800000000000010ffffffffffffffffb781126dcae7b2321e66a241adc64d2f";
        assert!(sign_stark_hash(order.into(), "0x1".into()).is_err());
        assert!(sign_stark_hash("0x0".into(), "0x1".into()).is_err());
        assert!(sign_stark_hash("0x1".into(), "0x1".into()).is_ok());
        assert!(derive_strk20_viewing_key("0x1".into(), "0x0".into(), "0x123".into()).is_err());
        assert!(derive_strk20_viewing_key(
            "0x1".into(),
            "0x534e5f5345504f4c4941".into(),
            "0x0123".into()
        )
        .is_err());
        assert!(derive_strk20_viewing_key(
            "0x1".into(),
            "0x534e5f5345504f4c4941".into(),
            "0x123".into()
        )
        .is_ok());
        assert!(derive_argent_legacy_keypair(PHRASE.into(), 1 << 31).is_err());
    }

    #[test]
    fn class_hashes_are_non_zero_per_network() {
        let main = oz_account_class_hash(NetworkId::Mainnet, "3.0.0").unwrap();
        let sepolia = oz_account_class_hash(NetworkId::Sepolia, "3.0.0").unwrap();
        assert!(felt::parse_non_zero(&main).is_some());
        assert!(felt::parse_non_zero(&sepolia).is_some());
    }
}
