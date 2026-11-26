//! The one type-directed reading of a SNIP-12 leaf value (the extension's `snip12-scalars.ts`):
//! whether a value declared under a basic type is one that type can mean, on a given revision,
//! and the felt the pinned encoder puts into the hash for it.
//!
//! starknet.js's leaf encoder reads a value as a number whenever `BigInt` accepts it and as a
//! short string otherwise, and revision 0 range-checks nothing. So each type admits only the
//! spellings it can mean: numeric types take canonical decimal or `0x` text or a safe integer,
//! `bool` a boolean, the short-string types numeric text or printable ASCII `BigInt` refuses.
//! Ranges hold on both revisions. The value is untrusted; `None` means the gate must refuse it.

use super::typed_data::Revision;
use crate::js::big::{parse_bigint, JsBigInt};
use crate::js::json::JsValue;
use crate::js::num::{is_safe_integer, number_to_string, utf16_len};
use crate::{kms, Felt};

/// A felt is at most 66 characters as hex and 78 as decimal; longer text is refused unparsed.
const MAX_NUMERIC_TEXT: usize = 80;
/// `encodeShortString`'s limit.
const MAX_SHORT_STRING: usize = 31;
/// An entry point name is an identifier; the SNIP-12 name bound.
const MAX_SELECTOR_NAME: usize = 64;

/// The felt a leaf hashes to. A revision 1 `string` is a ByteArray whose hash this module does
/// not compute (only Krusty hashes); it is accepted for signing and never compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoded {
    Felt(Felt),
    ByteArray,
}

/// How the approval card prints a leaf (`Snip12ScalarKind`): a decimal amount, a hex felt, or
/// site-chosen text beside the felt it encodes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Boolean,
    ByteString,
    Felt,
    Integer,
    Selector,
    Text,
}

/// `Snip12Scalar`: a leaf resolved against its declared type, with the text the card shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reading {
    pub encoded: Encoded,
    pub kind: Kind,
    pub text: String,
}

/// `snip12Scalar`'s felt: what `value` declared as `ty` encodes to on `revision`, or `None` where
/// the type cannot describe the value (an unknown type included).
pub fn snip12_scalar(value: &JsValue, ty: &str, revision: Revision) -> Option<Encoded> {
    snip12_reading(value, ty, revision).map(|reading| reading.encoded)
}

/// `snip12Scalar`: the felt and the card's reading of one leaf.
pub fn snip12_reading(value: &JsValue, ty: &str, revision: Revision) -> Option<Reading> {
    match ty {
        // Revision 1 asserts a boolean; revision 0 would hash any scalar, so `"false"` is refused.
        "bool" => match value {
            JsValue::Bool(flag) => Some(Reading {
                encoded: Encoded::Felt(Felt::from(u8::from(*flag))),
                kind: Kind::Boolean,
                text: flag.to_string(),
            }),
            _ => None,
        },
        "string" => match revision {
            Revision::Zero => short_string_or_number(value),
            Revision::One => value
                .as_str()
                .filter(|text| is_printable_ascii(text))
                .map(|text| Reading {
                    encoded: Encoded::ByteArray,
                    kind: Kind::ByteString,
                    text: text.to_string(),
                }),
        },
        "felt" | "shortstring" => short_string_or_number(value),
        "selector" => selector(value),
        "timestamp" | "u128" => {
            let parsed = numeric_value(value, false)?;
            let n = (!parsed.negative || parsed.magnitude.is_zero())
                .then(|| parsed.magnitude.to_u128())
                .flatten()?;
            Some(integer(Felt::from(n), &parsed))
        }
        // Only revision 1 wraps a negative into the field; both keep the i128 ceiling.
        "i128" => {
            let parsed = numeric_value(value, revision == Revision::One)?;
            let magnitude = parsed.magnitude.to_u128()?;
            let limit = 1u128 << 127;
            let felt = if parsed.negative && magnitude != 0 {
                // A negative number reaches here on revision 0 too (text cannot): it is out of range.
                (revision == Revision::One && magnitude <= limit).then(|| -Felt::from(magnitude))?
            } else {
                (magnitude < limit).then(|| Felt::from(magnitude))?
            };
            Some(integer(felt, &parsed))
        }
        "ClassHash" | "ContractAddress" => bounded_felt(value).map(felt_reading),
        _ => None,
    }
}

/// A whole number, shown in decimal (`bigint.toString(10)`).
fn integer(felt: Felt, parsed: &JsBigInt) -> Reading {
    let digits = parsed.magnitude.to_decimal();
    let sign = if parsed.negative && !parsed.magnitude.is_zero() {
        "-"
    } else {
        ""
    };
    Reading {
        encoded: Encoded::Felt(felt),
        kind: Kind::Integer,
        text: format!("{sign}{digits}"),
    }
}

/// A hex-shaped value: its text is the canonical felt, so every spelling reads as one row.
fn felt_reading(felt: Felt) -> Reading {
    Reading {
        encoded: Encoded::Felt(felt),
        kind: Kind::Felt,
        text: crate::felt::canonical(&felt),
    }
}

/// Printable ASCII only: a control character in a signed string is a display trick.
fn is_printable_ascii(text: &str) -> bool {
    text.bytes().all(|b| (0x20..=0x7e).contains(&b))
}

fn is_decimal_text(text: &str) -> bool {
    let digits = text.strip_prefix('-').unwrap_or(text);
    !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
}

/// `/^0[xX][0-9a-fA-F]+$/`.
fn is_hex_text(text: &str) -> bool {
    text.strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// `numericValue`: a safe integer, or decimal or `0x` text of at most 80 UTF-16 units, signed
/// only where `signed`. Narrower than `BigInt`: `""`, `" 12 "`, `"+12"`, `"0b101"` are refused.
fn numeric_value(value: &JsValue, signed: bool) -> Option<JsBigInt> {
    let text = match value {
        JsValue::Number(n) => {
            let negative_zero = *n == 0.0 && n.is_sign_negative();
            if !is_safe_integer(*n) || (!signed && negative_zero) {
                return None;
            }
            // A safe integer prints as plain digits, which `BigInt` reads back exactly.
            return parse_bigint(&number_to_string(*n));
        }
        JsValue::String(text) => text,
        _ => return None,
    };
    let length = utf16_len(text);
    if length == 0 || length > MAX_NUMERIC_TEXT || (!signed && text.starts_with('-')) {
        return None;
    }
    if !is_decimal_text(text) && !is_hex_text(text) {
        return None;
    }
    parse_bigint(text)
}

/// A non-negative value below the field prime, as a felt.
fn to_felt(parsed: &JsBigInt) -> Option<Felt> {
    if parsed.negative && !parsed.magnitude.is_zero() {
        return None;
    }
    crate::felt::parse(&format!("0x{}", parsed.magnitude.to_hex()))
}

/// `boundedFelt`: canonical numeric input inside the field.
fn bounded_felt(value: &JsValue) -> Option<Felt> {
    to_felt(&numeric_value(value, false)?)
}

/// `shortStringOrNumber`: a number first, as `getHex` reads one first; otherwise printable ASCII
/// of 1–31 characters that `BigInt` itself refuses, encoded as its bytes.
fn short_string_or_number(value: &JsValue) -> Option<Reading> {
    if let Some(felt) = bounded_felt(value) {
        return Some(felt_reading(felt));
    }
    let text = value.as_str()?;
    let fits = !text.is_empty() && text.len() <= MAX_SHORT_STRING;
    (fits && is_printable_ascii(text) && parse_bigint(text).is_none()).then(|| Reading {
        encoded: Encoded::Felt(Felt::from_bytes_be_slice(text.as_bytes())),
        kind: Kind::Text,
        text: text.to_string(),
    })
}

/// `selectorScalar`: `0x` hex inside the field, or an entry point name
/// (`/^[A-Za-z_][A-Za-z0-9_]*$/`, at most 64 characters) hashed by Krusty.
fn selector(value: &JsValue) -> Option<Reading> {
    let text = value.as_str()?;
    if is_hex_text(text) {
        return bounded_felt(value).map(felt_reading);
    }
    let mut bytes = text.bytes();
    let name = bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && text.len() <= MAX_SELECTOR_NAME;
    name.then(|| Reading {
        encoded: Encoded::Felt(kms::selector(text)),
        kind: Kind::Selector,
        text: text.to_string(),
    })
}
