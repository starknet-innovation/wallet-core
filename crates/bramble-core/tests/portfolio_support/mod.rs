//! Readers for `fixtures/portfolio-parity.jsonl`, which the extension's own TypeScript writes
//! (`scripts/mobile-parity-portfolio`), and Bramble's catalogs (`networks.json`,
//! `token-registry.json`, as `bun run mobile:assets` renders them) read into
//! `starknet_wallet_core::catalog`'s records. The parity fixture is read with the core's
//! JavaScript-faithful JSON reader so every expected number is exactly the double JavaScript
//! produced.
#![allow(dead_code)] // Each parity suite reads a different subset.

use std::sync::OnceLock;

use serde_json::Value;
use starknet_wallet_core::catalog::{
    Catalogs, NetworkInfo, RegistryToken, TokenInfo, TokenRegistry,
};
use starknet_wallet_core::host::Storage;
use starknet_wallet_core::js::json::{parse as parse_json, JsValue};
use starknet_wallet_core::portfolio::model::{DisplayCurrency, TokenConfig};
use starknet_wallet_core::portfolio::prices::{PricePoint, PriceView};
use starknet_wallet_core::portfolio::wire::HttpOutcome;
use starknet_wallet_core::NetworkId;

/// The host's storage in memory, for the activity log (mobile's tests used a temporary directory).
#[derive(Default)]
pub struct Memory(pub std::cell::RefCell<std::collections::BTreeMap<String, Vec<u8>>>);

impl Storage for Memory {
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, starknet_wallet_core::Error> {
        Ok(self.0.borrow().get(key).cloned())
    }
    fn write(&self, key: &str, value: &[u8]) -> Result<(), starknet_wallet_core::Error> {
        self.0.borrow_mut().insert(key.into(), value.to_vec());
        Ok(())
    }
    fn rename(&self, from: &str, to: &str) -> Result<(), starknet_wallet_core::Error> {
        let value = self.0.borrow_mut().remove(from);
        let value = value.ok_or_else(|| starknet_wallet_core::Error::storage("rename"))?;
        self.0.borrow_mut().insert(to.into(), value);
        Ok(())
    }
}

/// The fixture's JSON Lines (`{"block","value"}` per line) assembled into one object.
pub fn fixture() -> JsValue {
    let blocks = include_str!("../fixtures/portfolio-parity.jsonl")
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let record = parse_json(line).expect("fixture line parses");
            (text(&record, "block"), at(&record, "value").clone())
        })
        .collect();
    JsValue::Object(blocks)
}

fn catalog_file(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn str_at(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap().to_string()
}

fn decimals(value: &Value) -> u8 {
    u8::try_from(value["decimals"].as_u64().unwrap()).unwrap()
}

/// Bramble's network catalog, every network `NetworkId` names.
pub fn networks() -> &'static [NetworkInfo] {
    static NETWORKS: OnceLock<Vec<NetworkInfo>> = OnceLock::new();
    NETWORKS.get_or_init(|| {
        catalog_file("networks.json")["networks"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|entry| {
                Some(NetworkInfo {
                    network: NetworkId::from_catalog_id(entry["id"].as_str()?)?,
                    chain_id: str_at(entry, "chainId"),
                    explorer_base_url: str_at(entry, "explorerBaseUrl"),
                    name: str_at(entry, "name"),
                    rpc_urls: entry["rpcUrls"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|url| url.as_str().unwrap().to_string())
                        .collect(),
                    tokens: entry["tokens"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|t| TokenInfo {
                            address: str_at(t, "address"),
                            decimals: decimals(t),
                            name: str_at(t, "name"),
                            symbol: str_at(t, "symbol"),
                        })
                        .collect(),
                })
            })
            .collect()
    })
}

/// Bramble's curated token registry and bundled logos.
pub fn registry() -> &'static TokenRegistry {
    static REGISTRY: OnceLock<TokenRegistry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let file = catalog_file("token-registry.json");
        TokenRegistry {
            tokens: file["tokens"]
                .as_array()
                .unwrap()
                .iter()
                .map(|t| RegistryToken {
                    address: str_at(t, "address"),
                    decimals: decimals(t),
                    name: str_at(t, "name"),
                    network_id: str_at(t, "networkId"),
                    symbol: str_at(t, "symbol"),
                })
                .collect(),
            logos: file["logos"]
                .as_array()
                .unwrap()
                .iter()
                .map(|logo| logo.as_str().unwrap().to_string())
                .collect(),
        }
    })
}

/// Both of Bramble's catalogs.
pub fn catalogs() -> Catalogs<'static> {
    Catalogs {
        networks: networks(),
        registry: registry(),
    }
}

pub fn at<'a>(value: &'a JsValue, key: &str) -> &'a JsValue {
    value
        .get(key)
        .unwrap_or_else(|| panic!("fixture has {key}"))
}

pub fn list<'a>(value: &'a JsValue, key: &str) -> &'a [JsValue] {
    at(value, key)
        .as_array()
        .unwrap_or_else(|| panic!("{key} is a list"))
}

pub fn text(value: &JsValue, key: &str) -> String {
    at(value, key)
        .as_str()
        .unwrap_or_else(|| panic!("{key} is text"))
        .to_string()
}

pub fn opt_text(value: &JsValue, key: &str) -> Option<String> {
    value.get(key).and_then(JsValue::as_str).map(str::to_string)
}

pub fn num(value: &JsValue, key: &str) -> f64 {
    at(value, key)
        .as_f64()
        .unwrap_or_else(|| panic!("{key} is a number"))
}

pub fn opt_num(value: &JsValue, key: &str) -> Option<f64> {
    value.get(key).and_then(JsValue::as_f64)
}

/// Sorts object keys recursively so documents compare by content, not key order.
pub fn canon(value: &JsValue) -> JsValue {
    match value {
        JsValue::Object(entries) => {
            let mut sorted: Vec<(String, JsValue)> =
                entries.iter().map(|(k, v)| (k.clone(), canon(v))).collect();
            sorted.sort_by(|a, b| a.0.cmp(&b.0));
            JsValue::Object(sorted)
        }
        JsValue::Array(items) => JsValue::Array(items.iter().map(canon).collect()),
        other => other.clone(),
    }
}

pub fn currency(code: &str) -> DisplayCurrency {
    match code {
        "usd" => DisplayCurrency::Usd,
        "eur" => DisplayCurrency::Eur,
        "gbp" => DisplayCurrency::Gbp,
        other => panic!("currency {other}"),
    }
}

pub fn network(code: &str) -> NetworkId {
    match code {
        "SN_MAIN" => NetworkId::Mainnet,
        "SN_SEPOLIA" => NetworkId::Sepolia,
        other => panic!("network {other}"),
    }
}

/// What a host that honours `max_body_bytes` reports for a scripted answer.
pub fn outcome(answer: &JsValue, pad: usize, max_body_bytes: u64) -> HttpOutcome {
    match answer {
        JsValue::Null => HttpOutcome {
            status: 0,
            retry_after: None,
            body: None,
            timed_out: false,
        },
        answer => {
            let body = format!("{}{}", text(answer, "body"), " ".repeat(pad));
            let status = num(answer, "status") as u16;
            let body = (body.len() as u64 <= max_body_bytes).then(|| body.into_bytes());
            HttpOutcome {
                status,
                retry_after: None,
                body,
                timed_out: false,
            }
        }
    }
}

pub fn points(map: &JsValue) -> Vec<PricePoint> {
    map.as_object()
        .unwrap_or_default()
        .iter()
        .map(|(address, value)| PricePoint {
            address: address.clone(),
            value: value.as_f64().expect("price"),
        })
        .collect()
}

pub fn price_view(code: &str, prices: &JsValue, changes: &JsValue) -> PriceView {
    PriceView {
        currency: currency(code),
        prices: points(prices),
        changes: points(changes),
        as_of_ms: None,
        stale: false,
    }
}

/// A token config read without validation (the fixture holds invalid ones on purpose).
pub fn token(value: &JsValue) -> TokenConfig {
    TokenConfig {
        id: text(value, "id"),
        network: network(&text(value, "networkId")),
        address: text(value, "address"),
        symbol: text(value, "symbol"),
        name: text(value, "name"),
        decimals: num(value, "decimals") as u8,
    }
}

/// Compares a price view with `{asOf, stale, prices, changes}` as `getPrices` returned them.
pub fn assert_view(view: &PriceView, expected: &JsValue, context: &str) {
    assert_eq!(view.as_of_ms, opt_num(expected, "asOf"), "{context}: asOf");
    assert_eq!(
        view.stale,
        at(expected, "stale") == &JsValue::Bool(true),
        "{context}: stale"
    );
    assert_eq!(
        view.prices,
        points(at(expected, "prices")),
        "{context}: prices"
    );
    assert_eq!(
        view.changes,
        points(at(expected, "changes")),
        "{context}: changes"
    );
}
