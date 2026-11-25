//! Ported from the extension's `tests/provider/bridge/{provider-add-chain-declare-bridge,
//! provider-add-chain-names,cov-provider-add-chain-params}.test.ts`.

use super::ChainRequest;
use crate::dapp::request::tests::POLICY;
use crate::dapp::{read_request, Request, WalletRpcError};

const BASE: &str = r#""chain_id":"0x534e5f4b4154414e41","chain_name":"Katana","id":"katana""#;

fn chain(extra: &str) -> Result<Request, WalletRpcError> {
    let params = if extra.is_empty() {
        format!("{{{BASE}}}")
    } else {
        format!("{{{BASE},{extra}}}")
    };
    read_request("wallet_addStarknetChain", Some(&params), &POLICY)
}

fn katana(rpc_urls: Option<Vec<&str>>) -> Result<Request, WalletRpcError> {
    Ok(Request::AddChain(ChainRequest {
        id: "katana".into(),
        chain_id: "0x534e5f4b4154414e41".into(),
        chain_name: "Katana".into(),
        rpc_urls: rpc_urls.map(|urls| urls.into_iter().map(str::to_owned).collect()),
        block_explorer_urls: None,
        icon_urls: None,
    }))
}

fn refused(params: &str) -> bool {
    read_request("wallet_addStarknetChain", Some(params), &POLICY)
        == Err(WalletRpcError::invalid_request_payload())
}

// "maps valid params to a safe runtime message", "accepts the optional api_version"
#[test]
fn a_well_formed_chain() {
    assert_eq!(
        chain(r#""rpc_urls":["https://rpc.katana.example"]"#),
        katana(Some(vec!["https://rpc.katana.example"]))
    );
    assert_eq!(chain(""), katana(None));
    for version in ["0.10", "0.10.4", "0.10.5"] {
        assert_eq!(
            chain(&format!(r#""api_version":"{version}""#)),
            katana(None)
        );
    }
    // `native_currency` is accepted as an object and dropped; names are trimmed.
    assert_eq!(chain(r#""native_currency":{"symbol":"ETH"}"#), katana(None));
    assert_eq!(
        read_request(
            "wallet_addStarknetChain",
            Some(r#"{"chain_id":"0x1","chain_name":"  Katana  ","id":" katana "}"#),
            &POLICY
        )
        .map(|request| match request {
            Request::AddChain(chain) => (chain.id, chain.chain_name),
            other => panic!("{other:?}"),
        }),
        Ok(("katana".into(), "Katana".into()))
    );
}

// "rejects malformed chain params", "rejects a malformed api_version"
#[test]
fn malformed_chains_are_refused() {
    assert!(refused(r#"{"chain_name":"x"}"#));
    for bad in [
        r#""not.a.version.x""#,
        r#""""#,
        r#""v0.10.4""#,
        r#""0.10.4-rc.0""#,
        "10",
        "null",
    ] {
        assert_eq!(
            chain(&format!(r#""api_version":{bad}"#)),
            Err(WalletRpcError::invalid_request_payload())
        );
    }
    for extra in [
        r#""native_currency":null"#,
        r#""native_currency":"ETH""#,
        r#""extra":1"#,
        r#""rpc_urls":null"#,
    ] {
        assert_eq!(
            chain(extra),
            Err(WalletRpcError::invalid_request_payload()),
            "{extra}"
        );
    }
    // `isCanonicalChainId`: 0x, 1–64 hex digits of either case, not zero.
    for id in ["0x0", "0x", "534e", "0X1", &format!("0x{}", "f".repeat(65))] {
        assert!(
            refused(&format!(
                r#"{{"chain_id":"{id}","chain_name":"Katana","id":"katana"}}"#
            )),
            "{id}"
        );
    }
    assert!(!refused(&format!(
        r#"{{"chain_id":"0x{}","chain_name":"Katana","id":"katana"}}"#,
        "F".repeat(64)
    )));
}

// "refuses every built-in network's name, whatever its case or spacing", and the character rule.
#[test]
fn site_names() {
    let named = |name: &str| {
        let name = crate::js::json::JsValue::String(name.into()).to_json();
        (
            refused(&format!(
                r#"{{"chain_id":"0x1","chain_name":{name},"id":"katana"}}"#
            )),
            refused(&format!(
                r#"{{"chain_id":"0x1","chain_name":"Katana","id":{name}}}"#
            )),
        )
    };
    for reserved in POLICY.reserved_network_names {
        for spelling in [
            reserved.to_string(),
            reserved.to_uppercase(),
            format!("  {} ", reserved.replacen(' ', "   ", 1)),
        ] {
            assert_eq!(named(&spelling), (true, true), "{spelling}");
        }
    }
    for bad in [
        "Kat\u{0}ana",
        "Kat\u{202e}ana",
        "Starknet M\u{430}innet",
        "<b>Katana</b>",
        "",
        "   ",
        &"x".repeat(257),
    ] {
        assert_eq!(named(bad), (true, true), "{bad:?}");
    }
    for good in ["Katana", "Starknet Mainnet Fork", &"x".repeat(256)] {
        assert_eq!(named(good), (false, false), "{good}");
    }
}

// "refuses an endpoint in any spelling the URL parser would rewrite", the list rules and caps.
#[test]
fn url_lists() {
    for url in [
        "https://rpc.example/key secret",
        "https://rpc.example/key\u{a0}secret",
        "https://rpc.example/key\"secret",
        "https://rpc.example/key<secret",
        "https://rpc.example/key`secret",
        "https://rpc.example/key?k=a`b",
        "https:\\\\rpc.example\\key",
        "https:rpc.example/key",
        "https://rpc.example/k\tey",
        "HTTPS://RPC.example/key",
        "https://rpc.example:443/key",
        "not a url",
        "ftp://mirror.example",
    ] {
        let url = crate::js::json::JsValue::String(url.into()).to_json();
        for list in ["rpc_urls", "block_explorer_url", "icon_urls"] {
            assert_eq!(
                chain(&format!(r#""{list}":[{url}]"#)),
                Err(WalletRpcError::invalid_request_payload()),
                "{list} {url}"
            );
        }
    }
    for url in [
        "https://rpc.example/key",
        "https://rpc.example",
        "https://rpc.example/",
    ] {
        assert!(chain(&format!(r#""rpc_urls":["{url}"]"#)).is_ok(), "{url}");
    }
    let urls = |count: usize| {
        (0..count)
            .map(|index| format!(r#""https://rpc{index}.example""#))
            .collect::<Vec<_>>()
            .join(",")
    };
    for list in ["rpc_urls", "block_explorer_url", "icon_urls"] {
        assert!(chain(&format!(r#""{list}":[{}]"#, urls(POLICY.max_chain_urls))).is_ok());
        for bad in [
            format!("[{}]", urls(POLICY.max_chain_urls + 1)),
            "[]".to_owned(),
            "[123]".to_owned(),
            r#"["https://a.example","https://a.example"]"#.to_owned(),
        ] {
            assert_eq!(
                chain(&format!(r#""{list}":{bad}"#)),
                Err(WalletRpcError::invalid_request_payload()),
                "{list} {bad}"
            );
        }
    }
}
