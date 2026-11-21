//! The extension's JSON budget (`isJson`, `JSON_BUDGETS.provider` in `core/src/json.ts`) as
//! `isVaultV11` applies it: to the whole vault but its shadow-account scopes and multisig viewing
//! keys, which `isVaultV12`/`isVaultV13` set aside first and bound by count. It is the vault's one
//! capacity in the extension (Sherlock 261): a vault past it does not open there, so mobile
//! refuses to open one and refuses to write one.
//!
//! Charged as `JsonBudgetTracker` charges a parsed value: 32 bytes and one node per value, a
//! string `2 + 6 * length` bytes and a key `3 + 6 * length` (UTF-16 code units), with the
//! extension's caps on depth, array length, object keys and string length. The walk reads the
//! JSON text once and copies no value (string values may be secrets).

use std::fmt;

use serde::de::{DeserializeSeed, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};

use crate::account_names::utf16_len;

const MAX_ARRAY_LENGTH: usize = 4_096;
const MAX_BYTES: usize = 1024 * 1024;
const MAX_DEPTH: usize = 32;
const MAX_NODES: usize = 50_000;
const MAX_OBJECT_KEYS: usize = 2_048;
const MAX_STRING_LENGTH: usize = 512 * 1024;
/// The top-level collections the budget leaves out.
const UNBUDGETED: [&str; 3] = [
    "multisigStrk20KeyConflicts",
    "multisigStrk20ViewingKeys",
    "shadowAccountScopes",
];

#[derive(Default)]
struct Tracker {
    bytes: usize,
    nodes: usize,
}

impl Tracker {
    fn charge(&mut self, bytes: usize) -> bool {
        self.bytes = self.bytes.saturating_add(bytes);
        self.bytes <= MAX_BYTES
    }

    fn node(&mut self) -> bool {
        self.nodes += 1;
        self.charge(32) && self.nodes <= MAX_NODES
    }

    /// A string value or key: within the length cap, charged `fixed + 6 * length`.
    fn text(&mut self, value: &str, fixed: usize) -> bool {
        let length = utf16_len(value);
        length <= MAX_STRING_LENGTH && self.charge(fixed + 6 * length)
    }
}

/// One value at `depth` (the root is 0); `top` leaves out the unbudgeted collections.
struct Node<'a> {
    tracker: &'a mut Tracker,
    depth: usize,
    top: bool,
}

impl Node<'_> {
    fn check<E: serde::de::Error>(ok: bool) -> Result<(), E> {
        if ok {
            Ok(())
        } else {
            Err(E::custom("over the vault's JSON budget"))
        }
    }

    fn child(&mut self) -> Node<'_> {
        Node {
            tracker: self.tracker,
            depth: self.depth + 1,
            top: false,
        }
    }

    /// `normalizeJsonObject`: a container at the depth cap is refused.
    fn open<E: serde::de::Error>(&mut self) -> Result<(), E> {
        Self::check(self.tracker.node() && self.depth < MAX_DEPTH)
    }
}

impl<'de> DeserializeSeed<'de> for Node<'_> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> Result<(), D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Node<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("JSON within the vault's budget")
    }

    fn visit_bool<E: serde::de::Error>(self, _: bool) -> Result<(), E> {
        Self::check(self.tracker.node())
    }

    fn visit_i64<E: serde::de::Error>(self, _: i64) -> Result<(), E> {
        Self::check(self.tracker.node())
    }

    fn visit_u64<E: serde::de::Error>(self, _: u64) -> Result<(), E> {
        Self::check(self.tracker.node())
    }

    /// JSON has no infinities; `-0` is refused (`Object.is(value, -0)`).
    fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<(), E> {
        Self::check(self.tracker.node() && !(value == 0.0 && value.is_sign_negative()))
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<(), E> {
        Self::check(self.tracker.node())
    }

    fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<(), E> {
        Self::check(self.tracker.node() && self.tracker.text(value, 2))
    }

    fn visit_seq<A: SeqAccess<'de>>(mut self, mut seq: A) -> Result<(), A::Error> {
        self.open()?;
        let mut length = 0;
        while seq.next_element_seed(self.child())?.is_some() {
            length += 1;
            Self::check(length <= MAX_ARRAY_LENGTH)?;
        }
        Ok(())
    }

    fn visit_map<A: MapAccess<'de>>(mut self, mut map: A) -> Result<(), A::Error> {
        self.open()?;
        let mut keys = 0;
        while let Some(key) = map.next_key::<String>()? {
            if self.top && UNBUDGETED.contains(&key.as_str()) {
                map.next_value::<IgnoredAny>()?;
                continue;
            }
            keys += 1;
            Self::check(keys <= MAX_OBJECT_KEYS && self.tracker.text(&key, 3))?;
            map.next_value_seed(self.child())?;
        }
        Ok(())
    }
}

/// Whether the vault JSON `text` is within the extension's budget. Text that is not one JSON
/// value is refused too.
pub(crate) fn within(text: &[u8]) -> bool {
    let mut tracker = Tracker::default();
    let mut deserializer = serde_json::Deserializer::from_slice(text);
    let root = Node {
        tracker: &mut tracker,
        depth: 0,
        top: true,
    };
    root.deserialize(&mut deserializer).is_ok() && deserializer.end().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Charged as `JsonBudgetTracker` charges: 32 per value, `2 + 6n` per string, `3 + 6n` per
    /// key. `{"a":"bc"}` is 32 + (3 + 6) + 32 + (2 + 12) = 87 bytes.
    #[test]
    fn charges_as_the_extension_does() {
        let mut tracker = Tracker::default();
        let mut deserializer = serde_json::Deserializer::from_str(r#"{"a":"bc"}"#);
        let root = Node {
            tracker: &mut tracker,
            depth: 0,
            top: false,
        };
        root.deserialize(&mut deserializer).unwrap();
        assert_eq!((tracker.bytes, tracker.nodes), (87, 2));
    }

    #[test]
    fn refuses_what_the_budget_refuses() {
        assert!(within(br#"{"a":[1,true,null,"x"]}"#));
        assert!(!within(b"-0"));
        assert!(!within(br#"{"a":-0}"#));
        assert!(!within(b"{} {}"));
        // A string past 512 Ki code units, and bytes past 1 MiB (6 per code unit).
        assert!(!within(
            format!(r#"{{"a":"{}"}}"#, "x".repeat(MAX_STRING_LENGTH + 1)).as_bytes()
        ));
        let half = "x".repeat(100_000);
        let wide = format!(r#"{{"a":"{half}","b":"{half}"}}"#);
        assert!(!within(wide.as_bytes()));
        // Arrays past 4,096 entries; nesting to the depth cap.
        let long = format!("[{}]", vec!["0"; MAX_ARRAY_LENGTH + 1].join(","));
        assert!(!within(long.as_bytes()));
        assert!(within(
            format!("[{}]", vec!["0"; MAX_ARRAY_LENGTH].join(",")).as_bytes()
        ));
        let deep = |n: usize| format!("{}{}", "[".repeat(n), "]".repeat(n));
        assert!(within(deep(MAX_DEPTH).as_bytes()));
        assert!(!within(deep(MAX_DEPTH + 1).as_bytes()));
        // The collections `isVaultV12`/`isVaultV13` set aside are not charged.
        let scopes = format!(r#"{{"shadowAccountScopes":["{half}","{half}"]}}"#);
        assert!(within(scopes.as_bytes()));
    }
}
