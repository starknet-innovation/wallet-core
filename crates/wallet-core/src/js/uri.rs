//! The text built-ins a `data:` URI goes through: `atob` (the forgiving-base64 decode) and
//! `decodeURIComponent`. Each returns `None` where JavaScript throws.

use base64::alphabet::STANDARD;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};
use base64::engine::DecodePaddingMode;
use base64::Engine;

// Forgiving-base64 after its padding has been removed: no padding, trailing bits ignored.
const FORGIVING: GeneralPurpose = GeneralPurpose::new(
    &STANDARD,
    GeneralPurposeConfig::new()
        .with_decode_padding_mode(DecodePaddingMode::RequireNone)
        .with_decode_allow_trailing_bits(true),
);

/// `atob(text)` as bytes: ASCII whitespace removed, then up to two `=` when the length is a
/// multiple of four, then the standard alphabet only (a length of 4n+1 throws).
pub fn atob(text: &str) -> Option<Vec<u8>> {
    let mut data: String = text
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\x0c' | '\r' | ' '))
        .collect();
    if data.len().is_multiple_of(4) {
        for _ in 0..2 {
            if data.ends_with('=') {
                data.pop();
            }
        }
    }
    if data.len() % 4 == 1 {
        return None;
    }
    FORGIVING.decode(data).ok()
}

/// `decodeURIComponent(text)`: every `%XX` escape decoded, and the escaped bytes must form valid
/// UTF-8 (a stray `%`, a short escape or a broken sequence throws).
pub fn decode_uri_component(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(&byte) = bytes.get(index) {
        if byte == b'%' {
            let digits = std::str::from_utf8(bytes.get(index + 1..index + 3)?).ok()?;
            if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
                return None;
            }
            out.push(u8::from_str_radix(digits, 16).ok()?);
            index += 3;
        } else {
            out.push(byte);
            index += 1;
        }
    }
    // Literal text is whole UTF-8 characters, so only an escaped sequence can break this.
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atob_is_forgiving_where_javascript_is() {
        assert_eq!(atob("SGk=").unwrap(), b"Hi");
        assert_eq!(atob("SGk").unwrap(), b"Hi");
        assert_eq!(atob(" S G\nk= ").unwrap(), b"Hi");
        assert_eq!(atob("SGl=").unwrap(), b"Hi");
        assert_eq!(atob("").unwrap(), b"");
        assert!(atob("SGk==").is_none());
        assert!(atob("S").is_none());
        assert!(atob("S=Gk").is_none());
        assert!(atob("SG-k").is_none());
        assert!(atob("SGk=\u{a0}").is_none());
    }

    #[test]
    fn decode_uri_component_throws_where_javascript_does() {
        assert_eq!(
            decode_uri_component("%7B%22a%22%3A1%7D").unwrap(),
            r#"{"a":1}"#
        );
        assert_eq!(decode_uri_component("%C3%A9t%C3%A9").unwrap(), "été");
        assert_eq!(decode_uri_component("plain é").unwrap(), "plain é");
        assert!(decode_uri_component("%7Bbad%").is_none());
        assert!(decode_uri_component("%zz").is_none());
        assert!(decode_uri_component("%C3").is_none());
        assert!(decode_uri_component("%C3é").is_none());
        assert!(decode_uri_component("%ED%A0%80").is_none());
    }
}
