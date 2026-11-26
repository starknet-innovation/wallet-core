//! Reading P16's golden fixture (`fixtures/extension-dapp.jsonl`, written by Bramble's
//! `scripts/mobile-parity-dapp/`): its lines by block, and the extension's values as the core
//! reads them.
#![allow(dead_code)] // Each parity file uses its own share.

use serde_json::{json, Value};
use starknet_wallet_core::dapp::WalletRpcError;
use starknet_wallet_core::js::json::{self as js, JsValue};
use starknet_wallet_core::{felt, Felt};

/// Every line of `block`, asserting the count the generator wrote, so a regenerated fixture with
/// more or fewer lines fails here rather than going unread.
pub fn cases(block: &str, count: usize) -> Vec<Value> {
    let found: Vec<Value> = include_str!("../fixtures/extension-dapp.jsonl")
        .lines()
        .map(|line| serde_json::from_str(line).expect("a JSON line"))
        .filter(|case: &Value| case["block"] == block)
        .collect();
    assert_eq!(found.len(), count, "{block} lines");
    found
}

pub fn name(case: &Value) -> String {
    case["name"].as_str().unwrap_or("").to_string()
}

pub fn text(value: &Value) -> &str {
    value.as_str().expect("a string")
}

/// A fixture value as the JSON text a page would send.
pub fn js_value(value: &Value) -> JsValue {
    js::parse(&value.to_string()).expect("fixture JSON")
}

/// A core JSON value as a comparable `serde_json::Value`.
pub fn plain(value: &JsValue) -> Value {
    serde_json::from_str(&value.to_json()).expect("core JSON")
}

/// A wallet API error as the page receives it.
pub fn page_error(error: &WalletRpcError) -> Value {
    let mut page = json!({ "code": error.code, "message": error.message });
    if let Some(data) = &error.data {
        page["data"] = json!(data);
    }
    page
}

/// A felt in any spelling the extension writes (padded or not, hex or decimal).
pub fn felt_of(text: &str) -> Felt {
    match text.strip_prefix("0x") {
        Some(digits) => {
            let significant = digits.trim_start_matches('0');
            felt::parse(&format!(
                "0x{}",
                if significant.is_empty() {
                    "0"
                } else {
                    significant
                }
            ))
            .expect("a hex felt")
        }
        None => Felt::from_dec_str(text).expect("a decimal felt"),
    }
}

/// Where the core and the extension part, by case name: each one a finding reported with this
/// fixture, never a silent skip. A listed case that agrees fails too, so the list stays true.
pub struct Deviations {
    expected: &'static [(&'static str, &'static str)],
    seen: Vec<String>,
}

impl Deviations {
    pub fn new(expected: &'static [(&'static str, &'static str)]) -> Self {
        Self {
            expected,
            seen: Vec::new(),
        }
    }

    /// Checks one case: `ours` against `theirs`, or the listed deviation.
    pub fn check(&mut self, case: &str, ours: Value, theirs: Value) {
        let listed = self.expected.iter().any(|(name, _)| *name == case);
        if listed {
            assert_ne!(ours, theirs, "{case}: listed as a deviation, but agrees");
            self.seen.push(case.to_string());
        } else {
            assert_eq!(ours, theirs, "{case}");
        }
    }

    /// Every listed deviation was met.
    pub fn finish(self) {
        for (name, why) in self.expected {
            assert!(
                self.seen.iter().any(|seen| seen == name),
                "listed deviation not met: {name} ({why})"
            );
        }
    }
}
