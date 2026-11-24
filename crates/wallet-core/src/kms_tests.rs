//! Krusty custody through `kms`: derivation, signing and the keyless primitives.

use super::*;

const PHRASE: &str = "habit hope tip crystal because grunt nation idea electric witness alert like";

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

/// Ready re-seeds with the Ethereum key as a number, so a key starting `0x00` loses that
/// byte (Krusty 3.0; 2.0 kept it and derived another key for ~1 phrase in 256). Throwaway
/// phrases from Krusty's `argent_double_derivation_strips_leading_zero_eth_key_bytes`; the
/// keys were rechecked against Argent X's derivation (ethers 5, `grindKey`).
#[test]
fn argent_legacy_strips_leading_zero_eth_key_bytes() {
    let cases = [
        (
            "verify burst quarter genuine artefact arrow spot keep ecology confirm sorry sauce",
            "0x78fb8e59a8c26d6f4204bcedd6e8427fdf8be8e3081d2d09ac01526c718326b",
        ),
        (
            "quote denial risk amused glory have finish mean review night any excite",
            "0x3b8042748e0251dedec3001966d42b5427e764f540d8168d22bb1780955b5d1",
        ),
    ];
    for (case, (phrase, expected)) in cases.into_iter().enumerate() {
        let expected = Felt::from_hex(expected).unwrap();
        // Boolean asserts: never print a key on failure.
        let pair = derive_argent_legacy_keypair(phrase, 0).unwrap();
        assert!(*pair.private_key.expose_secret() == expected, "case {case}");
        let ladder = account_key(phrase, None, Ladder::ArgentX, 0).unwrap();
        assert!(*ladder.expose_secret() == expected, "case {case}");
    }
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

// `extension-multisig-transactions.json` (Sepolia): the extension's signature by the phrase's
// OpenZeppelin account 2 over the proposal's transaction hash.
const SIGNER: &str = "0x5c9c7b49024d4323f5397f6ffe428288729d1829b24a8a95c0376cc057c230c";
const HASH: &str = "0x63988bfcee64e18fc3218d9c5806314f359af6ffca2afe689de588ada649057";
const R: &str = "0x65c75682b3f1080e75acb0e17c611c869e9dc1b0e6ad136035b9f2badc1481e";
const S: &str = "0x2df9ed6a71b8f9fc6ddda0f4ffd000224a79b46f479ae57031c14c51c11a4c3";

#[test]
fn an_extension_signature_verifies_and_nothing_else_does() {
    let f = |value: &str| felt::parse(value).unwrap();
    let (key, hash, r, s) = (f(SIGNER), f(HASH), f(R), f(S));
    assert!(stark_signature_is_valid(&key, &hash, &r, &s));
    assert!(!stark_signature_is_valid(&key, &hash, &r, &(s + Felt::ONE)));
    assert!(!stark_signature_is_valid(&key, &(hash + Felt::ONE), &r, &s));
    let other = f("0x426212993d56613e1886a4cbc5b58810570023581c2aab0b423277776b79d2e");
    assert!(!stark_signature_is_valid(&other, &hash, &r, &s));
    // A zero hash is refused even where the raw check holds (`r = s = public key`).
    assert!(!stark_signature_is_valid(&key, &Felt::ZERO, &key, &key));
    assert!(!stark_signature_is_valid(&key, &hash, &Felt::ZERO, &s));
    assert!(!stark_signature_is_valid(&key, &hash, &r, &Felt::MAX));
}

#[test]
fn selectors_are_starknet_keccak() {
    let hex = |name| felt::canonical(&selector(name));
    assert_eq!(
        hex("transfer"),
        "0x83afd3f4caedc6eebf44246fe54e38c95e3179a5ec9ea81740eca5b482d12e"
    );
    assert_eq!(
        hex("__execute__"),
        "0x15d40a3d6ca2ac30f4031e42be28da9b056fef9bb7357ac5e85627ee876e5ad"
    );
}
