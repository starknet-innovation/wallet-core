//! P16's `request` block (`fixtures/extension-dapp.jsonl`): `read_page_request` reads each page
//! request as the extension's page and content scripts do. A request either of them refuses is
//! INVALID_REQUEST_PAYLOAD (114) here; one they admit reads to the runtime message the background
//! receives, compared field by field (felts by value). Hostile params are rebuilt from the
//! fixture's builder to the same JSON text length.
//!
//! The page script's `data: "invalid provider request"` is not the core's: on mobile the same
//! `inpage.js` runs in the page and refuses those before the bridge, so only the code is
//! compared for a `page` refusal.

mod dapp_parity_support;

use bramble_core::dapp::{read_page_request, PageRequest};
use dapp_parity_support::*;
use serde_json::{json, Value};
use starknet_wallet_core::dapp::{NftStandard, Request, WalletRpcError, WatchAsset};
use starknet_wallet_core::felt;

/// Where the core reads differently from the extension, each one reported with the fixture.
const DEVIATIONS: &[(&str, &str)] = &[
    (
        "mc_strk20Register with no params",
        "STRK20 waits on P15: the core answers 163 where the extension admits it",
    ),
    (
        "invoke with a well-formed proof",
        "a STRK20 proof waits on P15: the core answers 163 where the extension admits it",
    ),
    (
        "invoke with a malformed proof",
        "the core answers any non-null proof 163 before reading it; the extension refuses a \
         malformed one as 114",
    ),
    (
        "a STRK20 method",
        "STRK20 waits on P15: the core answers 163 where the extension admits it",
    ),
];

/// The JSON text a builder names, as `JSON.stringify` writes it.
fn built(build: &Value) -> String {
    let n = |key: &str| build[key].as_u64().expect("a size") as usize;
    let repeat = |count: usize, item: &str| vec![item; count].join(",");
    match text(&build["kind"]) {
        "nest" => format!("{}{}", "[".repeat(n("depth")), "]".repeat(n("depth"))),
        "array" => format!("[{}]", repeat(n("length"), "0")),
        "string" => format!("\"{}\"", "a".repeat(n("length"))),
        "keys" => {
            let keys: Vec<String> = (0..n("count")).map(|i| format!("\"k{i}\":0")).collect();
            format!("{{{}}}", keys.join(","))
        }
        "strings" => format!(
            "[{}]",
            repeat(n("count"), &format!("\"{}\"", "a".repeat(n("length"))))
        ),
        "grid" => format!(
            "[{}]",
            repeat(n("rows"), &format!("[{}]", repeat(n("columns"), "0")))
        ),
        other => panic!("unknown builder {other}"),
    }
}

/// A hostile case's params text: the builder's value alone, or set at `at` inside `base`.
fn hostile_params(case: &Value) -> String {
    let value = built(&case["build"]);
    let Some(path) = case["at"].as_array() else {
        return value;
    };
    let mut base = case["base"].clone();
    let mut cursor = &mut base;
    for key in path {
        cursor = &mut cursor[text(key)];
    }
    *cursor = json!("@@hostile@@");
    base.to_string().replace("\"@@hostile@@\"", &value)
}

fn felt_text(value: &Value) -> Value {
    json!(felt::canonical(&felt_of(text(value))))
}

/// An admitted runtime message with every felt canonical, so both sides compare by value.
fn canonical(runtime: &str, message: &Value) -> Value {
    let mut message = message.clone();
    match runtime {
        "wallet.addInvokeTransaction" => {
            for call in message["calls"].as_array_mut().into_iter().flatten() {
                call["contractAddress"] = felt_text(&call["contractAddress"]);
                let calldata: Vec<Value> = call["calldata"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(felt_text)
                    .collect();
                call["calldata"] = json!(calldata);
            }
            // The proof is P15's; only its presence is compared.
            if message.get("proof").is_some() {
                message["proof"] = json!(true);
            }
        }
        "wallet.addDeclareTransaction" => {
            for key in ["classHash", "compiledClassHash"] {
                if message["declare"].get(key).is_some() {
                    message["declare"][key] = felt_text(&message["declare"][key]);
                }
            }
        }
        "wallet.watchAsset" => {
            for kind in ["asset", "nft"] {
                if message.get(kind).is_some() {
                    message[kind]["address"] = felt_text(&message[kind]["address"]);
                }
            }
        }
        _ => {}
    }
    message
}

/// The core's read of a request as the extension's runtime message: `(type, message)`.
fn as_runtime(request: &PageRequest) -> (&'static str, Value) {
    let versioned = |api_version: &Option<String>| match api_version {
        Some(version) => json!({ "apiVersion": version }),
        None => json!({}),
    };
    let felt = |value: &starknet_wallet_core::Felt| json!(felt::canonical(value));
    let request = match request {
        PageRequest::GrantedAccounts => return ("wallet.getGrantedAccounts", json!({})),
        PageRequest::RequestAccounts => return ("wallet.requestAccounts", json!({})),
        PageRequest::Spec(request) => request,
    };
    match request {
        Request::RequestAccounts { silent } => (
            "wallet.requestAccounts",
            if *silent {
                json!({ "silent": true })
            } else {
                json!({})
            },
        ),
        Request::GetPermissions => ("wallet.getPermissions", json!({})),
        Request::RequestChainId => ("wallet.getChainId", json!({})),
        Request::SupportedSpecs { api_version } => {
            ("wallet.supportedSpecs", versioned(api_version))
        }
        Request::SupportedWalletApi { api_version } => {
            ("wallet.supportedWalletApi", versioned(api_version))
        }
        Request::DeploymentData { api_version } => {
            ("wallet.deploymentData", versioned(api_version))
        }
        Request::SwitchChain { chain_id } => ("wallet.switchChain", json!({ "chainId": chain_id })),
        Request::SignTypedData { typed_data } => (
            "wallet.signTypedData",
            json!({ "typedData": plain(typed_data) }),
        ),
        Request::AddInvoke { calls } => {
            let calls: Vec<Value> = calls
                .iter()
                .map(|call| {
                    let calldata: Vec<Value> = call.calldata.iter().map(felt).collect();
                    json!({
                        "calldata": calldata,
                        "contractAddress": felt(&call.contract_address),
                        "entrypoint": call.entrypoint,
                    })
                })
                .collect();
            ("wallet.addInvokeTransaction", json!({ "calls": calls }))
        }
        Request::AddDeclare {
            class_hash,
            compiled_class_hash,
            contract_class,
        } => {
            let mut declare = json!({
                "compiledClassHash": felt(compiled_class_hash),
                "contractClass": plain(contract_class),
            });
            if let Some(class_hash) = class_hash {
                declare["classHash"] = felt(class_hash);
            }
            (
                "wallet.addDeclareTransaction",
                json!({ "declare": declare }),
            )
        }
        Request::AddChain(chain) => {
            let mut read = json!({
                "chainId": chain.chain_id,
                "chainName": chain.chain_name,
                "id": chain.id,
            });
            for (key, urls) in [
                ("rpcUrls", &chain.rpc_urls),
                ("blockExplorerUrls", &chain.block_explorer_urls),
                ("iconUrls", &chain.icon_urls),
            ] {
                if let Some(urls) = urls {
                    read[key] = json!(urls);
                }
            }
            ("wallet.addStarknetChain", json!({ "chain": read }))
        }
        Request::WatchAsset(WatchAsset::Erc20 {
            address,
            decimals,
            name,
            symbol,
        }) => {
            let mut asset = json!({ "address": felt(address) });
            if let Some(decimals) = decimals {
                asset["decimals"] = json!(decimals);
            }
            if let Some(name) = name {
                asset["name"] = json!(name);
            }
            if let Some(symbol) = symbol {
                asset["symbol"] = json!(symbol);
            }
            ("wallet.watchAsset", json!({ "asset": asset }))
        }
        Request::WatchAsset(WatchAsset::Nft {
            address,
            standard,
            token_id,
        }) => {
            let standard = match standard {
                NftStandard::Erc721 => "ERC721",
                NftStandard::Erc1155 => "ERC1155",
            };
            (
                "wallet.watchAsset",
                json!({ "nft": { "address": felt(address), "standard": standard, "tokenId": token_id } }),
            )
        }
    }
}

fn outcome(read: Result<PageRequest, WalletRpcError>) -> Value {
    match read {
        Ok(request) => {
            let (runtime, message) = as_runtime(&request);
            json!({ "runtimeType": runtime, "message": canonical(runtime, &message) })
        }
        Err(error) => json!({ "error": error.code }),
    }
}

fn expected(case: &Value) -> Value {
    match text(&case["stage"]) {
        "admitted" => {
            let runtime = text(&case["runtimeType"]);
            json!({ "runtimeType": runtime, "message": canonical(runtime, &case["message"]) })
        }
        _ => json!({ "error": case["error"]["code"] }),
    }
}

#[test]
fn each_request_reads_as_the_extension_reads_it() {
    let mut deviations = Deviations::new(DEVIATIONS);
    for case in cases("request", 88) {
        let name = name(&case);
        let method = text(&case["method"]);
        let params = if case.get("build").is_some() {
            let params = hostile_params(&case);
            assert_eq!(
                params.len() as u64,
                case["bytes"].as_u64().unwrap(),
                "{name}"
            );
            Some(params)
        } else {
            case.get("params").map(Value::to_string)
        };
        let read = read_page_request(method, params.as_deref());
        let ours = outcome(read);
        // A hostile value the extension admits is the builder's own; only where it went is kept.
        let ours = if case.get("build").is_some() && ours.get("message").is_some() {
            json!({ "runtimeType": ours["runtimeType"], "message": "built" })
        } else {
            ours
        };
        let theirs = if case.get("build").is_some() && case["stage"] == "admitted" {
            json!({ "runtimeType": case["runtimeType"], "message": "built" })
        } else {
            expected(&case)
        };
        deviations.check(&name, ours, theirs);
    }
    deviations.finish();
}

#[test]
fn every_refusal_is_the_spec_payload_error() {
    for case in cases("request", 88) {
        if case["stage"] == "admitted" {
            continue;
        }
        let error = &case["error"];
        assert_eq!(error["code"], 114, "{}", name(&case));
        assert_eq!(
            text(&error["message"]),
            WalletRpcError::invalid_request_payload().message,
            "{}",
            name(&case)
        );
    }
}
