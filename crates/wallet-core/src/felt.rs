//! Felt grammar shared with the extension's `wallet-model/networks` helpers.
//!
//! Untrusted text is a felt only in the extension's own spelling: `0x` followed by 1–64
//! lowercase hex digits, below the field prime. The native apps pass addresses and hashes
//! through this module, never through their own parsers, so the two wallets cannot disagree
//! about which strings name the same value.

use starknet_types_core::felt::Felt;
use zeroize::Zeroizing;

use crate::js::num::js_trim;
use crate::scrub::scrubbed;
use crate::SecretFelt;

/// Stark curve order `n`; a signing scalar must lie in `[1, n)`.
const STARK_CURVE_ORDER: Felt =
    Felt::from_hex_unchecked("0x0800000000000010ffffffffffffffffb781126dcae7b2321e66a241adc64d2f");

/// Parses a felt in the extension's lowercase-hex grammar, rejecting values at or above the prime.
pub fn parse(value: &str) -> Option<Felt> {
    let digits = value.strip_prefix("0x")?;
    if digits.is_empty()
        || digits.len() > 64
        || !digits
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    // `Felt::from_hex` reduces modulo the prime; a value that does not round-trip was out of range.
    let felt = Felt::from_hex(value).ok()?;
    let trimmed = digits.trim_start_matches('0');
    let canonical_digits = format!("{felt:x}");
    let same = if trimmed.is_empty() {
        felt == Felt::ZERO
    } else {
        canonical_digits == trimmed
    };
    same.then_some(felt)
}

/// Parses a non-zero felt.
pub fn parse_non_zero(value: &str) -> Option<Felt> {
    parse(value).filter(|felt| *felt != Felt::ZERO)
}

/// The extension's canonical spelling: `0x` plus minimal lowercase hex (`0x0` for zero).
pub fn canonical(felt: &Felt) -> String {
    format!("{felt:#x}")
}

/// Parses a felt already in its [`canonical`] spelling (no leading zeros), as stored records and
/// signed intents must be.
pub fn parse_canonical(value: &str) -> Option<Felt> {
    parse(value).filter(|felt| canonical(felt) == value)
}

/// [`parse_canonical`], refusing zero.
pub fn parse_canonical_non_zero(value: &str) -> Option<Felt> {
    parse_canonical(value).filter(|felt| *felt != Felt::ZERO)
}

/// Whether a felt is a usable Stark signing scalar, `1 <= k < n`.
pub fn is_private_key(felt: &Felt) -> bool {
    *felt != Felt::ZERO && *felt < STARK_CURVE_ORDER
}

/// Hex digits (either case) of a usable signing scalar, decoded straight into a zeroizing buffer
/// so no text copy of the key is made. `None` for anything else.
fn private_key_digits(digits: &str) -> Option<SecretFelt> {
    scrubbed(|| {
        let significant = digits.trim_start_matches('0');
        if digits.is_empty() || significant.len() > 64 {
            return None;
        }
        let mut bytes = Zeroizing::new([0u8; 32]);
        for (at, digit) in significant.bytes().rev().enumerate() {
            let nibble = u8::try_from(char::from(digit).to_digit(16)?).ok()?;
            bytes[31 - at / 2] |= nibble << (4 * (at % 2));
        }
        let key = SecretFelt::new(Felt::from_bytes_be(&bytes));
        // `from_bytes_be` reduces modulo the prime: a value that does not round-trip was at or
        // above it, and reduced would name another scalar than the one given.
        let same = *Zeroizing::new(key.expose_secret().to_bytes_be()) == *bytes;
        (same && is_private_key(key.expose_secret())).then_some(key)
    })
}

/// A pasted private key, read as the extension's raw-key import reads one
/// (`readRawKeyAccountFields`, `apps/extension/src/background/accounts/import-inputs.ts`):
/// `normalizeFeltInput` (JavaScript `trim`, then after a lowercase `0x` hex digits in either case
/// and any number of leading zeros), then `isStarkPrivateKey` (`1 <= k < n`). A bare decimal,
/// `0X`, or any other base is refused, never reinterpreted.
pub fn parse_private_key_input(text: &str) -> Option<SecretFelt> {
    private_key_digits(js_trim(text).strip_prefix("0x")?)
}

/// A private key the vault sealed (`isSigningKeyRef`): the canonical spelling only, a usable
/// scalar.
pub fn parse_sealed_private_key(text: &str) -> Option<SecretFelt> {
    let digits = text.strip_prefix("0x")?;
    let canonical = !digits.starts_with('0')
        && digits
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    canonical.then(|| private_key_digits(digits)).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_extension_grammar_only() {
        assert_eq!(parse("0x0"), Some(Felt::ZERO));
        assert_eq!(
            parse("0x00ff").map(|f| canonical(&f)),
            Some("0xff".to_string())
        );
        for bad in ["", "0x", "ff", "0xFF", "0x-1", " 0x1", "0x1 ", "0xg"] {
            assert_eq!(parse(bad), None, "{bad:?}");
        }
        assert_eq!(parse(&format!("0x{}", "0".repeat(65))), None);
    }

    #[test]
    fn canonical_parsing_refuses_leading_zeros() {
        assert_eq!(parse_canonical("0xff"), Some(Felt::from(255u8)));
        assert_eq!(parse_canonical("0x00ff"), None);
        assert_eq!(parse_canonical("0x0"), Some(Felt::ZERO));
        assert_eq!(parse_canonical("0x00"), None);
        assert_eq!(parse_canonical_non_zero("0x0"), None);
        assert_eq!(parse_canonical_non_zero("0x1"), Some(Felt::ONE));
    }

    #[test]
    fn rejects_values_at_or_above_the_prime() {
        let prime = "0x800000000000011000000000000000000000000000000000000000000000001";
        assert_eq!(parse(prime), None);
        let below = "0x800000000000011000000000000000000000000000000000000000000000000";
        assert!(parse(below).is_some());
    }

    #[test]
    fn private_key_range_is_the_curve_order() {
        assert!(!is_private_key(&Felt::ZERO));
        assert!(is_private_key(&Felt::ONE));
        let order = STARK_CURVE_ORDER;
        assert!(!is_private_key(&order));
        assert!(is_private_key(&(order - Felt::ONE)));
    }

    // Boolean asserts throughout: a failure must not print a key.
    #[test]
    fn a_pasted_key_follows_the_extensions_import_grammar() {
        let read = |text: &str| parse_private_key_input(text).map(|key| *key.expose_secret());
        let abc = Some(Felt::from(0xabcu32));
        for accepted in ["0xabc", " 0xABC\n", "\u{feff}0xAbC\t", "0x0000abc"] {
            assert!(read(accepted) == abc, "{accepted:?}");
        }
        let padded = format!("0x{}1", "0".repeat(100));
        assert!(read(&padded) == Some(Felt::ONE));
        let order_less_one = "0x800000000000010ffffffffffffffffb781126dcae7b2321e66a241adc64d2e";
        assert!(read(order_less_one) == Some(STARK_CURVE_ORDER - Felt::ONE));
        let order = "0x800000000000010ffffffffffffffffb781126dcae7b2321e66a241adc64d2f";
        let prime = "0x800000000000011000000000000000000000000000000000000000000000001";
        // One above the prime reduces to 1, a usable key: refused, not reinterpreted.
        let prime_plus_one = "0x800000000000011000000000000000000000000000000000000000000000002";
        let too_long = format!("0x1{}", "0".repeat(64));
        for refused in [
            "",
            "0x",
            "0x0",
            "0x000",
            "abc",
            "0XABC",
            "1",
            "0b1",
            "0o1",
            "-0x1",
            "+0x1",
            "0x-1",
            "0x 1",
            "0xg",
            "0x1_0",
            "0xé",
            order,
            prime,
            prime_plus_one,
            &too_long,
        ] {
            assert!(read(refused).is_none(), "{refused:?}");
        }
    }

    #[test]
    fn a_sealed_key_is_canonical() {
        let read = |text: &str| parse_sealed_private_key(text).map(|key| *key.expose_secret());
        assert!(read("0xabc") == Some(Felt::from(0xabcu32)));
        for refused in ["0x0abc", "0xABC", " 0xabc", "0xabc ", "abc", "0x0", "0x"] {
            assert!(read(refused).is_none(), "{refused:?}");
        }
        let order = "0x800000000000010ffffffffffffffffb781126dcae7b2321e66a241adc64d2f";
        assert!(read(order).is_none());
    }
}
