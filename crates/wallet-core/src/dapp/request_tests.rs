//! Ported from the extension's `tests/provider/bridge/{provider-bridge,provider-discovery-bridge,
//! provider-bridge-results-guards,provider-add-chain-declare-bridge}.test.ts`, the cases a
//! method name and params text can carry.

use super::*;

const PROVIDER: JsonBudget = JsonBudget {
    max_array_length: 4_096,
    max_bytes: 1024 * 1024,
    max_depth: 32,
    max_nodes: 50_000,
    max_object_keys: 2_048,
    max_string_length: 512 * 1024,
};

pub(crate) const BUDGETS: RequestBudgets = RequestBudgets {
    default: PROVIDER,
    typed_data: JsonBudget {
        max_array_length: 1_024,
        max_bytes: 256 * 1024,
        max_depth: 24,
        max_nodes: 10_000,
        max_object_keys: 512,
        max_string_length: 64 * 1024,
    },
    declare: PROVIDER,
    invoke: PROVIDER,
    invoke_limits: crate::dapp::InvokeLimits {
        calls: 56,
        calldata_felts: 4_096,
        entrypoint_chars: 64,
        felt_chars: 78,
    },
};

fn read(method: &str, params: Option<&str>) -> Result<Request, WalletRpcError> {
    read_request(method, params, &BUDGETS)
}

fn invalid(method: &str, params: Option<&str>) -> bool {
    read(method, params) == Err(WalletRpcError::invalid_request_payload())
}

#[test]
fn method_names_round_trip_and_unknown_names_are_refused() {
    for (name, method) in METHODS {
        assert_eq!(Method::from_name(name), Some(method));
        assert_eq!(method.name(), name);
    }
    for name in [
        "",
        "wallet.signHash",
        "eth_requestAccounts",
        "WALLET_REQUESTACCOUNTS",
    ] {
        assert_eq!(Method::from_name(name), None);
        assert!(invalid(name, None));
    }
}

// `PROVIDER_METHOD_POLICY.approvalSurface`.
#[test]
fn approval_surface_matches_the_extension_table() {
    let quiet = [
        Method::AddStarknetChain,
        Method::DeploymentData,
        Method::GetPermissions,
        Method::RequestChainId,
        Method::SupportedSpecs,
        Method::SupportedWalletApi,
    ];
    for (_, method) in METHODS {
        assert_eq!(
            method.may_need_approval(),
            !quiet.contains(&method),
            "{method:?}"
        );
    }
}

// "forwards only explicitly supported safe methods"
#[test]
fn request_accounts_reads_silent_mode() {
    assert_eq!(
        read("wallet_requestAccounts", None),
        Ok(Request::RequestAccounts { silent: false })
    );
    assert_eq!(
        read("wallet_requestAccounts", Some(r#"{"silent_mode":true}"#)),
        Ok(Request::RequestAccounts { silent: true })
    );
    assert_eq!(
        read(
            "wallet_requestAccounts",
            Some(r#"{"silent_mode":false,"api_version":"0.10.5"}"#)
        ),
        Ok(Request::RequestAccounts { silent: false })
    );
    assert!(invalid(
        "wallet_requestAccounts",
        Some(r#"{"silent_mode":"yes"}"#)
    ));
    assert!(invalid("wallet_requestAccounts", Some(r#"{"debug":true}"#)));
    assert!(invalid("wallet_requestAccounts", Some("[]")));
    assert!(invalid("wallet_requestAccounts", Some("null")));
}

// `provider-discovery-bridge.test.ts`: "forwards discovery methods and negotiates api_version"
#[test]
fn discovery_methods_echo_a_well_formed_api_version() {
    assert_eq!(
        read("wallet_supportedSpecs", None),
        Ok(Request::SupportedSpecs { api_version: None })
    );
    assert_eq!(
        read("wallet_supportedWalletApi", None),
        Ok(Request::SupportedWalletApi { api_version: None })
    );
    assert_eq!(
        read("wallet_deploymentData", None),
        Ok(Request::DeploymentData { api_version: None })
    );
    assert_eq!(
        read("wallet_supportedSpecs", Some(r#"{"api_version":"0.7.2"}"#)),
        Ok(Request::SupportedSpecs {
            api_version: Some("0.7.2".into())
        })
    );
    assert_eq!(
        read("wallet_deploymentData", Some(r#"{"api_version":"0.7.2"}"#)),
        Ok(Request::DeploymentData {
            api_version: Some("0.7.2".into())
        })
    );
    assert!(invalid(
        "wallet_deploymentData",
        Some(r#"{"api_version":"not-a-version"}"#)
    ));
    assert!(invalid("wallet_supportedSpecs", Some(r#"{"junk":true}"#)));
}

// `provider-bridge-results-guards.test.ts`: "readApiVersionParam"
#[test]
fn api_version_only_methods() {
    for (method, request) in [
        ("wallet_getPermissions", Request::GetPermissions),
        ("wallet_requestChainId", Request::RequestChainId),
    ] {
        assert_eq!(read(method, None), Ok(request.clone()));
        assert_eq!(read(method, Some("{}")), Ok(request.clone()));
        assert_eq!(
            read(method, Some(r#"{"api_version":"1.2.3"}"#)),
            Ok(request)
        );
        assert!(invalid(method, Some(r#"{"other":1}"#)));
        assert!(invalid(method, Some(r#"{"api_version":"not-a-version"}"#)));
    }
}

// `provider-add-chain-declare-bridge.test.ts`'s well-formed and malformed versions.
#[test]
fn api_version_grammar() {
    for good in ["0.10", "0.10.4", "0.10.5", "1.2.3", "10.0"] {
        assert!(is_api_version(good), "{good}");
    }
    for bad in [
        "not.a.version.x",
        "",
        "v0.10.4",
        "0.10.4-rc.0",
        "1",
        "1.2.3.4",
        "1..2",
        "1.2.",
        "١.٢",
        "0.10.4\n",
    ] {
        assert!(!is_api_version(bad), "{bad:?}");
    }
    for bad in [r#""""#, "10", "null", r#""v0.10.4""#] {
        let params = format!(r#"{{"api_version":{bad}}}"#);
        assert!(invalid("wallet_supportedSpecs", Some(&params)), "{bad}");
    }
}

#[test]
fn switch_chain_takes_a_lowercase_hex_chain_id() {
    assert_eq!(
        read(
            "wallet_switchStarknetChain",
            Some(r#"{"chainId":"0x534e5f4d41494e"}"#)
        ),
        Ok(Request::SwitchChain {
            chain_id: "0x534e5f4d41494e".into()
        })
    );
    assert_eq!(
        read(
            "wallet_switchStarknetChain",
            Some(r#"{"chainId":"0x534e5f5345504f4c4941","silent_mode":true,"api_version":"0.10"}"#)
        ),
        Ok(Request::SwitchChain {
            chain_id: "0x534e5f5345504f4c4941".into()
        })
    );
    for bad in [
        r#"{"chainId":"SN_MAIN"}"#,
        r#"{"chainId":"0x534E5F4D41494E"}"#,
        r#"{"chainId":"0x"}"#,
        r#"{"chainId":1}"#,
        r#"{"chainId":"0x1","silent_mode":1}"#,
        r#"{"chainId":"0x1","extra":true}"#,
        r#"{}"#,
    ] {
        assert!(invalid("wallet_switchStarknetChain", Some(bad)), "{bad}");
    }
    assert!(invalid("wallet_switchStarknetChain", None));
}

// "readTypedDataParam"
#[test]
fn typed_data_wrappers() {
    let typed = r#"{"domain":{},"message":{},"primaryType":"X","types":{}}"#;
    let expected = crate::js::json::parse(typed).unwrap();
    let sign = |params: &str| read("wallet_signTypedData", Some(params));
    let ok = Ok(Request::SignTypedData {
        typed_data: expected.clone(),
    });
    assert_eq!(sign(&format!(r#"{{"typed_data":{typed}}}"#)), ok);
    assert_eq!(sign(&format!(r#"{{"typedData":{typed}}}"#)), ok);
    assert_eq!(sign(typed), ok);
    // A bare object loses a well-formed `api_version`; a wrapper keeps only its inner value.
    assert_eq!(
        sign(r#"{"domain":{},"message":{},"primaryType":"X","types":{},"api_version":"0.10.5"}"#),
        ok
    );
    assert_eq!(
        sign(&format!(r#"{{"typed_data":{typed},"api_version":"0.10"}}"#)),
        ok
    );
    // The spec wrapper wins over the legacy one.
    assert_eq!(
        sign(&format!(
            r#"{{"typedData":{{"x":1}},"typed_data":{typed}}}"#
        )),
        ok
    );
    assert!(invalid("wallet_signTypedData", None));
    assert!(invalid("wallet_signTypedData", Some("null")));
    assert!(invalid(
        "wallet_signTypedData",
        Some(r#"{"typed_data":null}"#)
    ));
    assert!(invalid(
        "wallet_signTypedData",
        Some(&format!(
            r#"{{"typed_data":{typed},"api_version":"0.10.4-rc.0"}}"#
        ))
    ));
    // Not an object: passed on for the review to refuse, as the extension does.
    assert_eq!(
        sign("[1]"),
        Ok(Request::SignTypedData {
            typed_data: JsValue::Array(vec![JsValue::Number(1.0)])
        })
    );
}

#[test]
fn params_are_held_to_the_method_budget() {
    // 70,000 units is within the provider's 512 KiB string bound but over typed data's 64 KiB.
    let long = format!(r#"{{"typed_data":{{"note":"{}"}}}}"#, "a".repeat(70_000));
    assert!(invalid("wallet_signTypedData", Some(&long)));
    let deep = format!("{}{}", "[".repeat(40), "]".repeat(40));
    assert!(invalid("wallet_requestChainId", Some(&deep)));
    assert!(invalid("wallet_requestChainId", Some("{")));
    assert!(invalid(
        "wallet_requestChainId",
        Some(r#"{"api_version":-0}"#)
    ));
}

#[test]
fn methods_not_read_yet_are_refused_as_unknown() {
    for name in [
        "wallet_addStarknetChain",
        "wallet_strk20Balances",
        "wallet_strk20InvokeTransaction",
        "wallet_strk20PrepareInvoke",
        "wallet_strk20ShadowAccountCommitment",
        "wallet_watchAsset",
    ] {
        assert_eq!(
            read(name, Some("{}")),
            Err(WalletRpcError::unknown_error()),
            "{name}"
        );
    }
}

// `provider-add-chain-declare-bridge.test.ts`: "wallet_addDeclareTransaction bridge mapping".
#[test]
fn declare_reads_its_hashes_and_carries_the_class() {
    use starknet_types_core::felt::Felt;
    const DECLARE: &str = r#"{"class_hash":"0x5abc","compiled_class_hash":"0x6def","contract_class":{"sierra_program":["0x1"]}}"#;
    let declare = |params: &str| read("wallet_addDeclareTransaction", Some(params));
    let expected = Ok(Request::AddDeclare {
        class_hash: Some(Felt::from(0x5abc_u64)),
        compiled_class_hash: Felt::from(0x6def_u64),
        contract_class: crate::js::json::parse(r#"{"sierra_program":["0x1"]}"#).unwrap(),
    });
    // "maps valid params to a safe runtime message"
    assert_eq!(
        declare(&format!(r#"{{"declare_transaction":{DECLARE}}}"#)),
        expected
    );
    // "accepts the optional top-level api_version and drops it"
    for version in ["0.10", "0.10.4", "0.10.5"] {
        assert_eq!(
            declare(&format!(
                r#"{{"declare_transaction":{DECLARE},"api_version":"{version}"}}"#
            )),
            expected
        );
    }
    // `class_hash` is optional.
    assert_eq!(
        declare(
            r#"{"declare_transaction":{"compiled_class_hash":"0x6def","contract_class":{"a":1}}}"#
        ),
        Ok(Request::AddDeclare {
            class_hash: None,
            compiled_class_hash: Felt::from(0x6def_u64),
            contract_class: crate::js::json::parse(r#"{"a":1}"#).unwrap(),
        })
    );
    let refused = |params: &str| {
        assert!(
            invalid("wallet_addDeclareTransaction", Some(params)),
            "{params}"
        )
    };
    // "rejects a malformed api_version, or one nested inside the declare"
    for bad in [
        r#""not.a.version.x""#,
        r#""""#,
        r#""v0.10.4""#,
        r#""0.10.4-rc.0""#,
        "10",
        "null",
    ] {
        refused(&format!(
            r#"{{"declare_transaction":{DECLARE},"api_version":{bad}}}"#
        ));
    }
    refused(
        r#"{"declare_transaction":{"compiled_class_hash":"0x6def","contract_class":{"a":1},"api_version":"0.10.4"}}"#,
    );
    // "rejects a declare missing its contract class"
    refused(r#"{"declare_transaction":{"compiled_class_hash":"0x6def"}}"#);
    // "rejects declare hashes outside the canonical Stark-field grammar"
    for hash in [format!("0x{}", "f".repeat(64)), "0xABC".into(), "1".into()] {
        refused(&format!(
            r#"{{"declare_transaction":{{"compiled_class_hash":"{hash}","contract_class":{{"a":1}}}}}}"#
        ));
        refused(&format!(
            r#"{{"declare_transaction":{{"class_hash":"{hash}","compiled_class_hash":"0x1","contract_class":{{"a":1}}}}}}"#
        ));
    }
    // A null class hash, an empty or non-object class, a missing or non-object declare, extra keys.
    for bad in [
        r#"{"declare_transaction":{"class_hash":null,"compiled_class_hash":"0x1","contract_class":{"a":1}}}"#,
        r#"{"declare_transaction":{"compiled_class_hash":"0x1","contract_class":{}}}"#,
        r#"{"declare_transaction":{"compiled_class_hash":"0x1","contract_class":["a"]}}"#,
        r#"{"declare_transaction":{"compiled_class_hash":"0x1","contract_class":"{}"}}"#,
        r#"{"declare_transaction":null}"#,
        r#"{"api_version":"0.10"}"#,
        r#"{"declare_transaction":{"compiled_class_hash":"0x1","contract_class":{"a":1}},"extra":1}"#,
        r#"[]"#,
    ] {
        refused(bad);
    }
    assert!(invalid("wallet_addDeclareTransaction", None));
}
