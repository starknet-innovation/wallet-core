//! Bramble's dapp provider choices, for `starknet_wallet_core::dapp`: the hostile-JSON budgets
//! (`JSON_BUDGETS` in `packages/foundation/core/src/json.ts`), the versions it advertises
//! (`provider/versions.ts`), and its own `mc_*` methods, which Bramble's page script calls
//! (`provider/inpage.ts`: `isPreauthorized` and `enable`). Plain Rust until the export, which
//! also checks the asking origin against the vault's origin grammar before reading anything.

use starknet_wallet_core::dapp::{
    read_request, InvokeLimits, JsonBudget, QueueLimits, Request, RequestPolicy, WalletRpcError,
};

/// `JSON_BUDGETS.provider`.
const PROVIDER_BUDGET: JsonBudget = JsonBudget {
    max_array_length: 4_096,
    max_bytes: 1024 * 1024,
    max_depth: 32,
    max_nodes: 50_000,
    max_object_keys: 2_048,
    max_string_length: 512 * 1024,
};

/// The extension's per-method budgets, as `readProviderParams` picks them.
pub const DAPP_POLICY: RequestPolicy = RequestPolicy {
    default: PROVIDER_BUDGET,
    // `JSON_BUDGETS.typedData`: hashing is the expensive part.
    typed_data: JsonBudget {
        max_array_length: 1_024,
        max_bytes: 256 * 1024,
        max_depth: 24,
        max_nodes: 10_000,
        max_object_keys: 512,
        max_string_length: 64 * 1024,
    },
    // `JSON_BUDGETS.contractClass`.
    declare: JsonBudget {
        max_array_length: 100_000,
        max_bytes: 8 * 1024 * 1024,
        max_depth: 64,
        max_nodes: 200_000,
        max_object_keys: 10_000,
        max_string_length: 2 * 1024 * 1024,
    },
    // `JSON_BUDGETS.strk20Proof`: an invoke may carry a STRK20 proof.
    invoke: JsonBudget {
        max_array_length: 4_096,
        max_bytes: 8 * 1024 * 1024,
        max_depth: 16,
        max_nodes: 50_000,
        max_object_keys: 32,
        max_string_length: 512 * 1024,
    },
    // `INVOKE_LIMITS`; the call and calldata caps are the ones a send holds to.
    invoke_limits: InvokeLimits {
        calls: crate::MAX_INVOKE_CALLS,
        calldata_felts: crate::MAX_CALL_CALLDATA_FELTS,
        entrypoint_chars: 64,
        // 66 covers `0x` hex; 78 also admits the decimal spelling starknet.js accepts.
        felt_chars: 78,
    },
    reserved_network_names: RESERVED_NETWORK_NAMES,
    // `MAX_CHAIN_URLS`: without a bound one approved card could fill the preferences record.
    max_chain_urls: 4,
};

/// The extension's built-in network names (`STARKNET_NETWORKS`), which a page may not give a
/// chain it asks to add, so an attacker's RPC cannot be added as "Starknet Mainnet". Devnet is
/// not in mobile's catalog but keeps its name reserved, as in the extension.
const RESERVED_NETWORK_NAMES: &[&str] = &["Starknet Mainnet", "Starknet Sepolia", "Local Devnet"];

/// The extension's approval queue limits (`approval-limits.ts`, `signature-waiters.ts`): five
/// pending per site and fifty in all, five minutes each, eight requests waiting on one.
pub const DAPP_QUEUE_LIMITS: QueueLimits = QueueLimits {
    max_pending: 50,
    max_per_origin: 5,
    ttl_ms: 5 * 60 * 1_000,
    max_waiters: 8,
};

/// `SUPPORTED_SPECS`: the Starknet JSON-RPC versions `wallet_supportedSpecs` reports.
pub const SUPPORTED_SPECS: &[&str] = &["0.10.0"];

/// `SUPPORTED_WALLET_API`: the wallet API versions `wallet_supportedWalletApi` reports (stable
/// `0.10.5`; release candidates are never advertised).
pub const SUPPORTED_WALLET_API: &[&str] = &["0.10.5"];

/// A request from Bramble's page script: a spec method, or one of Bramble's own.
#[derive(Debug, Clone, PartialEq)]
pub enum PageRequest {
    Spec(Request),
    /// `mc_accounts`: the granted account, without asking (`wallet.getGrantedAccounts`).
    GrantedAccounts,
}

/// Reads a page's request as `readProviderRequestArguments` and `toRuntimeMessage` do. The
/// `mc_*` methods take no params at all; `mc_requestAccounts` is `wallet_requestAccounts`.
/// `mc_strk20Register` waits for STRK20 (P15) and is refused as unknown until then.
pub fn read_page_request(
    method: &str,
    params: Option<&str>,
) -> Result<PageRequest, WalletRpcError> {
    let no_params = || {
        params
            .is_none()
            .then_some(())
            .ok_or_else(WalletRpcError::invalid_request_payload)
    };
    match method {
        "mc_accounts" => no_params().map(|()| PageRequest::GrantedAccounts),
        "mc_requestAccounts" => {
            no_params().map(|()| PageRequest::Spec(Request::RequestAccounts { silent: false }))
        }
        "mc_strk20Register" => Err(WalletRpcError::unknown_error()),
        _ => read_request(method, params, &DAPP_POLICY).map(PageRequest::Spec),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `provider-bridge.test.ts`: "reads direct provider request arguments"
    #[test]
    fn bramble_methods_take_no_params() {
        assert_eq!(
            read_page_request("mc_accounts", None),
            Ok(PageRequest::GrantedAccounts)
        );
        assert_eq!(
            read_page_request("mc_requestAccounts", None),
            Ok(PageRequest::Spec(Request::RequestAccounts {
                silent: false
            }))
        );
        for method in ["mc_accounts", "mc_requestAccounts"] {
            for params in ["[]", "{}", "null"] {
                assert_eq!(
                    read_page_request(method, Some(params)),
                    Err(WalletRpcError::invalid_request_payload()),
                    "{method} {params}"
                );
            }
        }
        assert_eq!(
            read_page_request("mc_strk20Register", None),
            Err(WalletRpcError::unknown_error())
        );
    }

    #[test]
    fn spec_methods_go_through_the_core_with_bramble_budgets() {
        assert_eq!(
            read_page_request(
                "wallet_switchStarknetChain",
                Some(r#"{"chainId":"0x534e5f4d41494e"}"#)
            ),
            Ok(PageRequest::Spec(Request::SwitchChain {
                chain_id: "0x534e5f4d41494e".into()
            }))
        );
        assert_eq!(
            read_page_request("wallet.signHash", None),
            Err(WalletRpcError::invalid_request_payload())
        );
        // Typed data over 64 KiB of one string is past `JSON_BUDGETS.typedData`.
        let long = format!(r#"{{"typed_data":{{"n":"{}"}}}}"#, "a".repeat(65_537));
        assert_eq!(
            read_page_request("wallet_signTypedData", Some(&long)),
            Err(WalletRpcError::invalid_request_payload())
        );
    }

    #[test]
    fn every_catalog_network_name_is_reserved() {
        for network in crate::catalogs::networks::all() {
            assert!(
                RESERVED_NETWORK_NAMES.contains(&network.name.as_str()),
                "{}",
                network.name
            );
        }
        assert_eq!(
            read_page_request(
                "wallet_addStarknetChain",
                Some(r#"{"id":"x","chain_id":"0x1","chain_name":"  starknet   MAINNET "}"#)
            ),
            Err(WalletRpcError::invalid_request_payload())
        );
    }

    #[test]
    fn advertised_versions_are_well_formed() {
        for version in SUPPORTED_WALLET_API.iter().chain(SUPPORTED_SPECS) {
            assert!(starknet_wallet_core::dapp::request::is_api_version(version));
        }
    }
}
