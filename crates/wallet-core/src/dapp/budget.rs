//! The extension's resource budget for hostile JSON (`JsonBudget`, `normalizeFeltBigints` in
//! `packages/foundation/core/src/json.ts`), applied to the JSON text a page sends.
//!
//! The extension checks a structured-clone value; here the host passes text. The page's own
//! script runs the extension's normaliser first (it refuses `undefined` array items, cycles,
//! accessors, negative `bigint`s, `NaN`, `±Infinity` and `-0`, then writes `JSON.stringify`), so
//! an honest request reaches this check as the text of a value the extension already admitted.
//! A page that calls the bridge directly is held to the same budget here: the text is the trust
//! boundary, not the page's script.

use crate::js::json::{self, JsValue};
use crate::js::num::utf16_len;

/// Bounds on one hostile JSON value. `max_bytes` bounds its serialisation from above: strings
/// and keys are charged six bytes per UTF-16 unit, every node 32.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JsonBudget {
    pub max_array_length: usize,
    pub max_bytes: usize,
    pub max_depth: usize,
    pub max_nodes: usize,
    pub max_object_keys: usize,
    pub max_string_length: usize,
}

/// Parses `text` as `JSON.parse` does and returns the value only if it is within `budget`.
///
/// Text longer than `max_bytes` is refused before parsing. That never refuses a value the
/// budget admits: its `JSON.stringify` text is at most what the budget charges for it (a number
/// prints in at most 24 characters against the node's 32, a string or key escapes to at most
/// six bytes per UTF-16 unit, and every comma, colon and bracket sits inside a node's 32).
pub fn parse_within(text: &str, budget: &JsonBudget) -> Option<JsValue> {
    if text.len() > budget.max_bytes {
        return None;
    }
    let value = json::parse(text)?;
    is_within(&value, budget).then_some(value)
}

/// Whether a parsed value is within `budget` (`normalizeJsonValue`'s accounting).
pub fn is_within(value: &JsValue, budget: &JsonBudget) -> bool {
    Tracker {
        budget,
        bytes: 0,
        nodes: 0,
    }
    .value(value, 0)
}

struct Tracker<'a> {
    budget: &'a JsonBudget,
    bytes: usize,
    nodes: usize,
}

impl Tracker<'_> {
    fn consume_bytes(&mut self, bytes: usize) -> bool {
        self.bytes = self.bytes.saturating_add(bytes);
        self.bytes <= self.budget.max_bytes
    }

    fn consume_text(&mut self, text: &str, overhead: usize) -> bool {
        let units = utf16_len(text);
        units <= self.budget.max_string_length
            && self.consume_bytes(units.saturating_mul(6).saturating_add(overhead))
    }

    fn value(&mut self, value: &JsValue, depth: usize) -> bool {
        self.nodes += 1;
        if !self.consume_bytes(32) || self.nodes > self.budget.max_nodes {
            return false;
        }
        if depth > self.budget.max_depth {
            return false;
        }
        match value {
            JsValue::Null | JsValue::Bool(_) => true,
            // `Number.isFinite(value) && !Object.is(value, -0)`.
            JsValue::Number(number) => {
                number.is_finite() && !(*number == 0.0 && number.is_sign_negative())
            }
            JsValue::String(text) => self.consume_text(text, 2),
            JsValue::Array(items) => {
                depth < self.budget.max_depth
                    && items.len() <= self.budget.max_array_length
                    && items.iter().all(|item| self.value(item, depth + 1))
            }
            JsValue::Object(entries) => {
                depth < self.budget.max_depth
                    && entries.len() <= self.budget.max_object_keys
                    && entries
                        .iter()
                        .all(|(key, item)| self.consume_text(key, 3) && self.value(item, depth + 1))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `json.test.ts`'s budget.
    const TINY: JsonBudget = JsonBudget {
        max_array_length: 2,
        max_bytes: 200,
        max_depth: 2,
        max_nodes: 5,
        max_object_keys: 2,
        max_string_length: 8,
    };

    const ROOMY: JsonBudget = JsonBudget {
        max_array_length: 4_096,
        max_bytes: 1024 * 1024,
        max_depth: 32,
        max_nodes: 50_000,
        max_object_keys: 2_048,
        max_string_length: 512 * 1024,
    };

    fn admits(text: &str, budget: &JsonBudget) -> bool {
        parse_within(text, budget).is_some()
    }

    // "hostile JSON resource budgets fail closed before expensive consumers"
    #[test]
    fn tiny_budget_cases_from_the_extension() {
        assert!(admits(r#"["one","two"]"#, &TINY));
        assert!(!admits(r#"["one","two","three"]"#, &TINY));
        assert!(!admits(r#"{"value":"xxxxxxxxx"}"#, &TINY));
        assert!(!admits(r#"{"one":{"two":{"three":true}}}"#, &TINY));
        assert!(!admits(r#"{"one":1,"three":3,"two":2}"#, &TINY));
    }

    // "isJson rejects values that stringify lossy JSON": the cases JSON text can carry.
    #[test]
    fn numbers_must_be_finite_and_not_negative_zero() {
        assert!(admits(r#"{"count":1,"items":["one",null,true]}"#, &ROOMY));
        assert!(!admits(r#"{"count":-0}"#, &ROOMY));
        assert!(!admits(r#"{"count":-0.0}"#, &ROOMY));
        assert!(!admits(r#"{"count":1e999}"#, &ROOMY));
        assert!(admits(r#"{"count":0}"#, &ROOMY));
    }

    // "bounded normalization preserves hostile property names": they are ordinary keys here.
    #[test]
    fn hostile_property_names_are_plain_keys() {
        let value = parse_within(
            r#"{"__proto__":{"polluted":true},"constructor":"own-value","prototype":"own-value"}"#,
            &ROOMY,
        );
        let value = value.unwrap();
        assert_eq!(
            value.get("__proto__").and_then(|v| v.get("polluted")),
            Some(&JsValue::Bool(true))
        );
        assert_eq!(
            value.get("constructor").and_then(JsValue::as_str),
            Some("own-value")
        );
    }

    #[test]
    fn depth_admits_scalars_at_the_limit_and_containers_below_it() {
        let one_deep = JsonBudget {
            max_depth: 1,
            ..ROOMY
        };
        assert!(admits("[1]", &one_deep));
        assert!(admits("[null]", &one_deep));
        assert!(!admits("[[]]", &one_deep));
        assert!(!admits(r#"[{}]"#, &one_deep));
        let flat = JsonBudget {
            max_depth: 0,
            ..ROOMY
        };
        assert!(admits("1", &flat));
        assert!(!admits("[]", &flat));
    }

    #[test]
    fn strings_and_keys_count_utf16_units() {
        let four = JsonBudget {
            max_string_length: 4,
            ..ROOMY
        };
        // Two astral characters are four UTF-16 units; a fifth unit is one too many.
        assert!(admits(r#""😀😀""#, &four));
        assert!(!admits(r#""😀😀a""#, &four));
        assert!(!admits(r#"{"abcde":1}"#, &four));
    }

    #[test]
    fn bytes_are_charged_as_the_extension_charges_them() {
        // One node (32) plus a string of 10 units (2 + 60) is 94 bytes.
        let exact = JsonBudget {
            max_bytes: 94,
            ..ROOMY
        };
        assert!(admits(r#""0123456789""#, &exact));
        let short = JsonBudget {
            max_bytes: 93,
            ..ROOMY
        };
        assert!(!admits(r#""0123456789""#, &short));
        // An object: 32 for it, 3 + 6 for the key, 32 for the value.
        let object = JsonBudget {
            max_bytes: 73,
            ..ROOMY
        };
        assert!(admits(r#"{"a":1}"#, &object));
        assert!(!admits(
            r#"{"a":1}"#,
            &JsonBudget {
                max_bytes: 72,
                ..ROOMY
            }
        ));
    }

    #[test]
    fn text_longer_than_the_byte_budget_is_refused_before_parsing() {
        let budget = JsonBudget {
            max_bytes: 40,
            ..ROOMY
        };
        assert!(admits("1", &budget));
        // Whitespace the budget would not charge still counts against the raw text.
        assert!(!admits(&format!("{}1", " ".repeat(40)), &budget));
    }

    #[test]
    fn duplicate_keys_count_once_as_json_parse_keeps_the_last() {
        let two_keys = JsonBudget {
            max_object_keys: 1,
            ..ROOMY
        };
        assert!(admits(r#"{"a":1,"a":2}"#, &two_keys));
        assert_eq!(
            parse_within(r#"{"a":1,"a":2}"#, &two_keys).and_then(|v| v.get("a").cloned()),
            Some(JsValue::Number(2.0))
        );
    }

    #[test]
    fn malformed_text_is_refused() {
        assert!(!admits("{", &ROOMY));
        assert!(!admits("", &ROOMY));
        assert!(!admits("undefined", &ROOMY));
    }
}
