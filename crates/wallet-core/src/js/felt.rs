//! The extension's felt helpers that go through `BigInt(string)` rather than the strict grammar in
//! `crate::felt`: `canonicalFelt`, `sameFelt`, `asFelt`, `uint256Felts`, `paddedFelt`,
//! `canonicalAddress` (`wallet-model/networks`). Untrusted RPC and explorer text reaches these,
//! and the extension accepts decimal and upper-case spellings there, so mobile must too.

use super::big::{parse_bigint, JsBigInt};
use super::json::JsValue;
use super::num::{is_js_whitespace, js_trim};
use starknet_types_core::felt::Felt;

/// `isStarkFieldElement`: `0x` + 1–64 lowercase hex digits, below the field prime.
pub fn is_stark_field_element(value: &str) -> bool {
    crate::felt::parse(value).is_some()
}

/// `isNonZeroStarkFieldElement`.
pub fn is_non_zero_felt(value: &str) -> bool {
    crate::felt::parse_non_zero(value).is_some()
}

/// `canonicalFelt`: `0x` + `BigInt(value).toString(16)` (so `-5` gives `0x-5`, which every caller
/// then rejects as a field element), or `None` where `BigInt` throws.
pub fn canonical_felt(value: &str) -> Option<String> {
    parse_bigint(value).map(|b| format!("0x{}", b.to_hex_string()))
}

fn same_value(a: &JsBigInt, b: &JsBigInt) -> bool {
    a.magnitude == b.magnitude && (a.negative == b.negative || a.magnitude.is_zero())
}

/// `sameFelt`: both non-empty strings whose `BigInt` values are equal.
pub fn same_felt(left: &str, right: &str) -> bool {
    if left.is_empty() || right.is_empty() {
        return false;
    }
    match (parse_bigint(left), parse_bigint(right)) {
        (Some(a), Some(b)) => same_value(&a, &b),
        _ => false,
    }
}

/// `sameFelt` over untyped JSON values (anything but a non-empty string is never the same).
pub fn same_felt_value(left: Option<&JsValue>, right: &str) -> bool {
    left.and_then(JsValue::as_str)
        .is_some_and(|left| same_felt(left, right))
}

/// `asFelt`: a non-empty string parsed with `BigInt`.
pub fn as_felt(value: Option<&JsValue>) -> Option<JsBigInt> {
    let text = value?.as_str()?;
    if text.is_empty() {
        return None;
    }
    parse_bigint(text)
}

/// `uint256Felts(low, high)` as `(low, high)` limbs, each a non-negative felt below 2^128.
pub fn uint256_felts(low: Option<&JsValue>, high: Option<&JsValue>) -> Option<(u128, u128)> {
    let limb = |value: Option<&JsValue>| {
        let parsed = as_felt(value)?;
        if parsed.negative && !parsed.magnitude.is_zero() {
            return None;
        }
        parsed.magnitude.to_u128()
    };
    Some((limb(low)?, limb(high)?))
}

/// `paddedFelt`: trimmed, lowercased, a field element, rendered at 64 digits.
pub fn padded_felt(value: &str) -> Option<String> {
    let lower = js_trim(value).to_lowercase();
    let felt = crate::felt::parse(&lower)?;
    let hex = format!("{felt:x}");
    Some(format!("0x{hex:0>64}"))
}

/// `addressLabel`: a felt cut for display as `0x05e1…f2e4` from its [`padded_felt`] spelling, so
/// every stored spelling of one address reads the same. Anything that is not a felt (a decimal,
/// free text, a value at or above the prime) is returned unchanged. The one shortener: every
/// address or hash a screen shortens goes through it.
pub fn address_label(value: &str) -> String {
    match padded_felt(value) {
        // ASCII, 66 bytes: `0x` and 64 digits.
        Some(felt) => format!("{}…{}", &felt[..6], &felt[felt.len() - 4..]),
        None => value.to_string(),
    }
}

/// `addressText`: a felt spelled in full for display and copying, padded to 64 digits; anything
/// that is not a felt is returned unchanged.
pub fn address_text(value: &str) -> String {
    padded_felt(value).unwrap_or_else(|| value.to_string())
}

/// `canonicalAddress`: `paddedFelt` of a non-zero field element; `None` otherwise.
pub fn canonical_address(value: &str) -> Option<String> {
    let lower = js_trim(value).to_lowercase();
    crate::felt::parse_non_zero(&lower)?;
    padded_felt(&lower)
}

/// Hex digits (any case) to a felt below the prime; `None` for anything else.
fn hex_digits(digits: &str) -> Option<Felt> {
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let significant = digits.trim_start_matches('0').to_ascii_lowercase();
    if significant.len() > 64 {
        return None;
    }
    crate::felt::parse(&format!(
        "0x{}",
        if significant.is_empty() {
            "0"
        } else {
            &significant
        }
    ))
}

/// `canonicalFelt(value)` for `0x`-prefixed hex, non-zero: `BigInt` accepts trailing whitespace
/// and any digit case. Returns the canonical (minimal) felt.
pub fn lenient_hex_felt(value: &str) -> Option<Felt> {
    let digits = value.strip_prefix("0x")?;
    hex_digits(digits.trim_end_matches(is_js_whitespace)).filter(|f| *f != Felt::ZERO)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn felt_helpers_follow_bigint() {
        assert_eq!(canonical_felt("0x00AB").as_deref(), Some("0xab"));
        assert_eq!(canonical_felt("171").as_deref(), Some("0xab"));
        assert_eq!(canonical_felt("-5").as_deref(), Some("0x-5"));
        assert_eq!(canonical_felt("xyz"), None);
        assert!(same_felt("0x0ab", "171"));
        assert!(!same_felt("", "0x0"));
        assert_eq!(
            canonical_address(" 0xAB ").as_deref(),
            Some("0x00000000000000000000000000000000000000000000000000000000000000ab")
        );
        assert_eq!(canonical_address("0x0"), None);
        let two = |a: &str, b: &str| {
            uint256_felts(
                Some(&JsValue::String(a.into())),
                Some(&JsValue::String(b.into())),
            )
        };
        assert_eq!(two("0x10", "0"), Some((16, 0)));
        assert_eq!(two("-1", "0"), None);
        assert_eq!(two(&format!("0x1{}", "0".repeat(32)), "0"), None);
        assert_eq!(two("", "0"), None);
    }

    #[test]
    fn address_label_cuts_the_padded_spelling_and_passes_non_felts_through() {
        let account = "0x208361e3911b1d93a704400b88e7e53a89171fce64dbb48f8479e4f44b398d9";
        for (input, label) in [
            (account, "0x0208…98d9"),
            (&format!("0x0{}", &account[2..]), "0x0208…98d9"),
            (
                &format!("0x00{}", &account[2..]),
                &format!("0x00{}", &account[2..]),
            ),
            ("0x0", "0x0000…0000"),
            ("0x1", "0x0000…0001"),
            ("0XABC", "0x0000…0abc"),
            ("0xABC", "0x0000…0abc"),
            ("  0x1f  ", "0x0000…001f"),
            // At and just below the field prime.
            (
                "0x800000000000011000000000000000000000000000000000000000000000001",
                "0x800000000000011000000000000000000000000000000000000000000000001",
            ),
            (
                "0x800000000000011000000000000000000000000000000000000000000000000",
                "0x0800…0000",
            ),
            // 65 digits, even with a leading zero, is not the grammar.
            (
                &format!("0x0{}", "1".repeat(64)),
                &format!("0x0{}", "1".repeat(64)),
            ),
            ("1234567", "1234567"),
            ("-45", "-45"),
            ("0b101", "0b101"),
            ("0o777", "0o777"),
            ("0x", "0x"),
            ("0xg", "0xg"),
            ("not an address", "not an address"),
            ("", ""),
        ] {
            assert_eq!(address_label(input), label, "{input:?}");
        }
    }

    #[test]
    fn address_text_pads_felts_and_passes_non_felts_through() {
        assert_eq!(address_text("0x12"), format!("0x{}12", "0".repeat(62)));
        assert_eq!(address_text(" 0XAB "), format!("0x{}ab", "0".repeat(62)));
        assert_eq!(address_text("1234567"), "1234567");
        assert_eq!(address_text("  text  "), "  text  ");
        assert_eq!(address_text(""), "");
    }

    #[test]
    fn lenient_hex_felt_takes_any_case_and_trailing_whitespace() {
        let canonical = |value: &str| lenient_hex_felt(value).map(|f| crate::felt::canonical(&f));
        assert_eq!(canonical("0x00AB\n").as_deref(), Some("0xab"));
        assert_eq!(canonical("0xab"), Some("0xab".into()));
        for bad in ["0x0", "0x", " 0xab", "0x ab", "ab", "0xg", "0XAB"] {
            assert_eq!(lenient_hex_felt(bad), None, "{bad:?}");
        }
    }
}
