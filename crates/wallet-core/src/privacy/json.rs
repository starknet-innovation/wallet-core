//! JSON helpers for byte parity with the SDK's `JSON.stringify` bodies.
//!
//! The discovery cursor is echoed back to the service exactly as the SDK does: `JSON.parse` keeps
//! object keys in document order and `JSON.stringify` writes them back compactly. `Ordered` keeps
//! that order (serde's `Value` sorts keys). To stay exact, numbers are limited to safe integers
//! (what `JSON.stringify` of a parsed integer reproduces digit for digit) and duplicate keys are
//! refused; either is `InvalidResponse`, a documented deviation (Bramble's
//! `mobile/docs/privacy.md` §3).
//!
//! `js_bigint` mirrors the `BigInt(...)` coercions the SDK applies to service values, over a fixed
//! grammar: decimal or `0x` hex text, or a safe non-negative integer. It yields a `U256`; a value
//! that does not fit is at least 2^256 and so refused by every caller's `< P` bound anyway.

use std::fmt;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use zeroize::Zeroizing;

use crate::send::u256::U256;

/// Largest integer `JSON.parse` represents exactly (`Number.MAX_SAFE_INTEGER`).
pub const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

/// A JSON value that keeps object keys in document order. Its `Debug` prints the shape only: a
/// service value can be a discovery cursor, which carries channel keys in clear.
#[derive(Clone, PartialEq, Eq)]
pub enum Ordered {
    Null,
    Bool(bool),
    Int(u64),
    Str(String),
    Array(Vec<Ordered>),
    Object(Vec<(String, Ordered)>),
}

impl fmt::Debug for Ordered {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Null => f.write_str("Null"),
            Self::Bool(_) => f.write_str("Bool"),
            Self::Int(_) => f.write_str("Int"),
            Self::Str(text) => write!(f, "Str({} bytes)", text.len()),
            Self::Array(items) => write!(f, "Array({} items)", items.len()),
            Self::Object(members) => write!(f, "Object({} members)", members.len()),
        }
    }
}

impl Ordered {
    /// A member of an object, by key.
    pub fn get(&self, key: &str) -> Option<&Ordered> {
        match self {
            Self::Object(members) => members.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Whether any object key, at any depth, is an array index (`"0"` to `"4294967294"`).
    /// JavaScript objects list those keys first, in numeric order, so `JSON.stringify` of the
    /// parsed value would not keep document order; a cursor holding one cannot be echoed exactly.
    pub fn has_index_key(&self) -> bool {
        match self {
            Self::Array(items) => items.iter().any(Self::has_index_key),
            Self::Object(members) => members
                .iter()
                .any(|(key, value)| is_array_index(key) || value.has_index_key()),
            _ => false,
        }
    }

    /// `JSON.stringify` of this value.
    pub fn write(&self, out: &mut dyn Sink) {
        match self {
            Self::Null => out.put("null"),
            Self::Bool(value) => out.put(if *value { "true" } else { "false" }),
            Self::Int(value) => out.put(&value.to_string()),
            Self::Str(value) => write_str(out, value),
            Self::Array(items) => {
                out.put("[");
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.put(",");
                    }
                    item.write(out);
                }
                out.put("]");
            }
            Self::Object(members) => {
                out.put("{");
                for (index, (key, value)) in members.iter().enumerate() {
                    if index > 0 {
                        out.put(",");
                    }
                    write_str(out, key);
                    out.put(":");
                    value.write(out);
                }
                out.put("}");
            }
        }
    }
}

/// Where JSON text is written: a string, or a [`Measure`] that sizes one first.
pub trait Sink {
    fn put(&mut self, text: &str);
}

impl Sink for String {
    fn put(&mut self, text: &str) {
        self.push_str(text);
    }
}

/// Counts the bytes a write would make, and keeps none of them.
pub struct Measure(pub usize);

impl Sink for Measure {
    fn put(&mut self, text: &str) {
        self.0 += text.len();
    }
}

/// What `build` writes, in one zeroizing string sized exactly by a first, counting pass, so it is
/// never reallocated: the discovery cursor it echoes carries channel keys in clear.
pub fn sized(build: impl Fn(&mut dyn Sink)) -> Zeroizing<String> {
    let mut measure = Measure(0);
    build(&mut measure);
    let mut out = Zeroizing::new(String::with_capacity(measure.0));
    build(&mut *out);
    debug_assert_eq!(out.len(), measure.0);
    debug_assert_eq!(out.capacity(), measure.0);
    out
}

fn is_array_index(key: &str) -> bool {
    key.parse::<u32>()
        .is_ok_and(|index| index != u32::MAX && index.to_string() == key)
}

/// A JSON string literal as `JSON.stringify` writes it (and serde_json, which a test checks):
/// `\"`, `\\`, `\b\f\n\r\t`, other controls as lowercase `\u00xx`, nothing else escaped.
/// Written straight into `out`, with no temporary copy of the text.
pub fn write_str(out: &mut dyn Sink, value: &str) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.put("\"");
    let mut start = 0;
    // Every escaped byte is ASCII, so each cut is on a character boundary.
    for (at, byte) in value.bytes().enumerate() {
        let short = match byte {
            b'"' => "\\\"",
            b'\\' => "\\\\",
            0x08 => "\\b",
            0x0c => "\\f",
            b'\n' => "\\n",
            b'\r' => "\\r",
            b'\t' => "\\t",
            0..=0x1f => "",
            _ => continue,
        };
        out.put(value.get(start..at).unwrap_or_default());
        if short.is_empty() {
            let code = [
                b'\\',
                b'u',
                b'0',
                b'0',
                HEX[usize::from(byte >> 4)],
                HEX[usize::from(byte & 0xf)],
            ];
            out.put(std::str::from_utf8(&code).unwrap_or_default());
        } else {
            out.put(short);
        }
        start = at + 1;
    }
    out.put(value.get(start..).unwrap_or_default());
    out.put("\"");
}

struct OrderedVisitor;

impl<'de> Visitor<'de> for OrderedVisitor {
    type Value = Ordered;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("JSON with safe-integer numbers and unique keys")
    }

    fn visit_unit<E: de::Error>(self) -> Result<Ordered, E> {
        Ok(Ordered::Null)
    }

    fn visit_none<E: de::Error>(self) -> Result<Ordered, E> {
        Ok(Ordered::Null)
    }

    fn visit_bool<E: de::Error>(self, value: bool) -> Result<Ordered, E> {
        Ok(Ordered::Bool(value))
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<Ordered, E> {
        if value > MAX_SAFE_INTEGER {
            return Err(E::custom("unsafe integer"));
        }
        Ok(Ordered::Int(value))
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<Ordered, E> {
        u64::try_from(value)
            .map_err(|_| E::custom("negative number"))
            .and_then(|v| self.visit_u64(v))
    }

    fn visit_f64<E: de::Error>(self, _: f64) -> Result<Ordered, E> {
        Err(E::custom("fractional number"))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Ordered, E> {
        Ok(Ordered::Str(value.to_string()))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Ordered, E> {
        Ok(Ordered::Str(value))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Ordered, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element::<Ordered>()? {
            items.push(item);
        }
        Ok(Ordered::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Ordered, A::Error> {
        let mut members: Vec<(String, Ordered)> = Vec::new();
        while let Some((key, value)) = map.next_entry::<String, Ordered>()? {
            if members.iter().any(|(existing, _)| *existing == key) {
                return Err(de::Error::custom("duplicate key"));
            }
            members.push((key, value));
        }
        Ok(Ordered::Object(members))
    }
}

impl<'de> Deserialize<'de> for Ordered {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(OrderedVisitor)
    }
}

/// `BigInt(value)` for a non-negative service value, over the fixed grammar above.
pub fn js_bigint(value: &Ordered) -> Option<U256> {
    match value {
        Ordered::Int(n) => Some(U256::from_u128(u128::from(*n))),
        Ordered::Str(text) => match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
            Some(digits) => U256::from_hex(digits.trim_start_matches('0')).or_else(|| {
                (!digits.is_empty() && digits.bytes().all(|b| b == b'0')).then_some(U256::ZERO)
            }),
            None => U256::from_decimal(text.trim_start_matches('0')).or_else(|| {
                (!text.is_empty() && text.bytes().all(|b| b == b'0')).then_some(U256::ZERO)
            }),
        },
        _ => None,
    }
}

/// The Stark field prime `P = 2^251 + 17·2^192 + 1`.
pub const FIELD_PRIME: U256 = U256 {
    high: (1 << 123) + (17 << 64),
    low: 1,
};

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Option<Ordered> {
        serde_json::from_str(text).ok()
    }

    #[test]
    fn keeps_document_order_and_writes_like_json_stringify() {
        let text = r#"{ "z": 1, "a": {"y": [true, null, "x\n"], "b": false} }"#;
        let mut out = String::new();
        parse(text).unwrap().write(&mut out);
        assert_eq!(out, r#"{"z":1,"a":{"y":[true,null,"x\n"],"b":false}}"#);
    }

    #[test]
    fn strings_escape_as_serde_json_does() {
        let mut every: String = (0u8..0x80).map(char::from).collect();
        every.push_str("é😀\u{2028}\u{7f}");
        for text in [every.as_str(), "", "plain", "\"\\", "a\u{1}b"] {
            let mut out = String::new();
            write_str(&mut out, text);
            assert_eq!(out, serde_json::to_string(text).unwrap(), "{text:?}");
        }
    }

    #[test]
    fn sized_writes_into_one_exact_buffer() {
        let cursor =
            parse(r#"{"channels":{"0x5":{"channel_key":"0xc0ffee","n":[1,null,true]}},"z":"\n"}"#)
                .unwrap();
        let out = sized(|out| cursor.write(out));
        assert_eq!(out.len(), out.capacity());
        let mut plain = String::new();
        cursor.write(&mut plain);
        assert_eq!(*out, plain);
    }

    #[test]
    fn refuses_what_json_stringify_would_respell() {
        for text in [
            r#"{"a":1.5}"#,
            r#"{"a":1.0}"#,
            r#"{"a":-1}"#,
            r#"{"a":9007199254740992}"#,
            r#"{"a":1,"a":2}"#,
        ] {
            assert_eq!(parse(text), None, "{text}");
        }
        assert_eq!(
            parse("9007199254740991"),
            Some(Ordered::Int(MAX_SAFE_INTEGER))
        );
    }

    #[test]
    fn array_index_keys_are_found_at_any_depth() {
        for (text, found) in [
            (r#"{"0x1":{"a":[{"b":1}]}}"#, false),
            (r#"{"01":1,"-1":1,"1.0":1,"4294967295":1," 1":1}"#, false),
            (r#"{"0":1}"#, true),
            (r#"{"a":{"4294967294":1}}"#, true),
            (r#"[{"b":{"7":null}}]"#, true),
        ] {
            assert_eq!(parse(text).unwrap().has_index_key(), found, "{text}");
        }
    }

    #[test]
    fn bigint_grammar() {
        let big = |t: &str| js_bigint(&Ordered::Str(t.into()));
        assert_eq!(big("1000"), Some(U256::from_u128(1000)));
        assert_eq!(big("0x3e8"), Some(U256::from_u128(1000)));
        assert_eq!(big("0X3E8"), Some(U256::from_u128(1000)));
        assert_eq!(big("0"), Some(U256::ZERO));
        assert_eq!(big("0x0"), Some(U256::ZERO));
        assert_eq!(big("0x000001"), Some(U256::from_u128(1)));
        for bad in ["", "0x", "-1", " 1", "1e3", "0xg"] {
            assert_eq!(big(bad), None, "{bad:?}");
        }
        assert_eq!(js_bigint(&Ordered::Int(7)), Some(U256::from_u128(7)));
        assert_eq!(js_bigint(&Ordered::Bool(true)), None);
        let p = big("0x800000000000011000000000000000000000000000000000000000000000001");
        assert_eq!(p, Some(FIELD_PRIME));
    }
}
