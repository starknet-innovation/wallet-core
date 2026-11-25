//! A dapp's request, read at the trust boundary: the method name and params text a host's
//! bridge took from a page, checked against the extension's method table and param grammars
//! (`packages/foundation/wallet-protocol/src/provider/{methods,request-params,bridge,results,
//! typed-data-params,versions}.ts`) and turned into a typed [`Request`] or the
//! INVALID_REQUEST_PAYLOAD (114) the extension answers.
//!
//! The origin is not read here. Which origin asked is the host's to establish from what its
//! platform reports (the top frame's committed origin), and the wallet's to check against its
//! origin grammar before any request is acted on.

use super::add_chain::{add_chain, ChainRequest};
use super::budget::{parse_within, JsonBudget};
use super::error::WalletRpcError;
use super::invoke::{invoke_params, InvokeLimits};
use super::watch_asset::{watch_asset, WatchAsset};
use crate::js::json::JsValue;
use crate::send::calls::Call;
use starknet_types_core::felt::Felt;

/// A wallet API method the wallet accepts from a page (`PROVIDER_METHOD_POLICY`'s spec names).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Method {
    AddDeclareTransaction,
    AddInvokeTransaction,
    AddStarknetChain,
    DeploymentData,
    GetPermissions,
    RequestAccounts,
    RequestChainId,
    SignTypedData,
    Strk20Balances,
    Strk20InvokeTransaction,
    Strk20PrepareInvoke,
    Strk20ShadowAccountCommitment,
    SupportedSpecs,
    SupportedWalletApi,
    SwitchStarknetChain,
    WatchAsset,
}

const METHODS: [(&str, Method); 16] = [
    (
        "wallet_addDeclareTransaction",
        Method::AddDeclareTransaction,
    ),
    ("wallet_addInvokeTransaction", Method::AddInvokeTransaction),
    ("wallet_addStarknetChain", Method::AddStarknetChain),
    ("wallet_deploymentData", Method::DeploymentData),
    ("wallet_getPermissions", Method::GetPermissions),
    ("wallet_requestAccounts", Method::RequestAccounts),
    ("wallet_requestChainId", Method::RequestChainId),
    ("wallet_signTypedData", Method::SignTypedData),
    ("wallet_strk20Balances", Method::Strk20Balances),
    (
        "wallet_strk20InvokeTransaction",
        Method::Strk20InvokeTransaction,
    ),
    ("wallet_strk20PrepareInvoke", Method::Strk20PrepareInvoke),
    (
        "wallet_strk20ShadowAccountCommitment",
        Method::Strk20ShadowAccountCommitment,
    ),
    ("wallet_supportedSpecs", Method::SupportedSpecs),
    ("wallet_supportedWalletApi", Method::SupportedWalletApi),
    ("wallet_switchStarknetChain", Method::SwitchStarknetChain),
    ("wallet_watchAsset", Method::WatchAsset),
];

impl Method {
    /// The method a page named, if the wallet accepts it (`isProviderMethod`).
    pub fn from_name(name: &str) -> Option<Self> {
        METHODS
            .iter()
            .find(|(spelling, _)| *spelling == name)
            .map(|(_, method)| *method)
    }

    pub fn name(self) -> &'static str {
        METHODS
            .iter()
            .find(|(_, method)| *method == self)
            .map_or("", |(spelling, _)| spelling)
    }

    /// Whether answering may need the person (`approvalSurface`): the host should be ready to
    /// show the wallet when one of these arrives.
    pub fn may_need_approval(self) -> bool {
        !matches!(
            self,
            Self::AddStarknetChain
                | Self::DeploymentData
                | Self::GetPermissions
                | Self::RequestChainId
                | Self::SupportedSpecs
                | Self::SupportedWalletApi
        )
    }
}

/// The wallet's rules for reading a request: its params budgets by kind (`JSON_BUDGETS`, chosen
/// per method as `readProviderParams` chooses them), an invoke's limits, and add-chain's rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestPolicy {
    /// Every method not named below.
    pub default: JsonBudget,
    /// `wallet_signTypedData`, tighter because hashing is the expensive part.
    pub typed_data: JsonBudget,
    /// `wallet_addDeclareTransaction`, whose contract class is large.
    pub declare: JsonBudget,
    /// `wallet_addInvokeTransaction`, which may carry a STRK20 proof.
    pub invoke: JsonBudget,
    /// The bounds on an invoke's calls, beyond its JSON budget.
    pub invoke_limits: InvokeLimits,
    /// The wallet's built-in network names, which `wallet_addStarknetChain` may not reuse.
    pub reserved_network_names: &'static [&'static str],
    /// The most URLs in each of `wallet_addStarknetChain`'s lists (`MAX_CHAIN_URLS`).
    pub max_chain_urls: usize,
}

impl RequestPolicy {
    fn for_method(&self, method: Method) -> &JsonBudget {
        match method {
            Method::AddDeclareTransaction => &self.declare,
            Method::AddInvokeTransaction => &self.invoke,
            Method::SignTypedData => &self.typed_data,
            _ => &self.default,
        }
    }
}

/// A request whose params parsed. The optional `api_version` every method takes is checked for
/// shape and dropped, except where a discovery method echoes it.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    /// `wallet_requestAccounts`; `silent` is the spec's `silent_mode`.
    RequestAccounts {
        silent: bool,
    },
    GetPermissions,
    RequestChainId,
    SupportedSpecs {
        api_version: Option<String>,
    },
    SupportedWalletApi {
        api_version: Option<String>,
    },
    DeploymentData {
        api_version: Option<String>,
    },
    /// `wallet_switchStarknetChain`'s `chainId`, lowercase hex.
    SwitchChain {
        chain_id: String,
    },
    /// `wallet_signTypedData`'s SNIP-12 typed data, unchecked beyond the budget: its own shape is
    /// the review's to check before anything is signed.
    SignTypedData {
        typed_data: JsValue,
    },
    /// `wallet_addInvokeTransaction`'s calls, each address non-zero and every calldata entry a
    /// felt. Its review is the wallet's own (`send::invoke::Invoke::reviewed`).
    AddInvoke {
        calls: Vec<Call>,
    },
    /// `wallet_addDeclareTransaction`'s declare: the hashes as felts and the contract class
    /// carried as given, a non-empty object the review never shows.
    AddDeclare {
        class_hash: Option<Felt>,
        compiled_class_hash: Felt,
        contract_class: JsValue,
    },
    /// `wallet_addStarknetChain`'s chain.
    AddChain(ChainRequest),
    /// `wallet_watchAsset`'s asset.
    WatchAsset(WatchAsset),
}

/// Reads a page's request: `method` as named, `params` as the JSON text of its params, or `None`
/// where the page passed none (`params: undefined` included).
///
/// The STRK20 methods, whose params are P15's to read, are refused with UNKNOWN_ERROR (163)
/// until their readers land, so nothing acts on params no grammar has checked. So is an invoke
/// that carries a STRK20 proof, once its calls have parsed.
pub fn read_request(
    method: &str,
    params: Option<&str>,
    policy: &RequestPolicy,
) -> Result<Request, WalletRpcError> {
    let invalid = WalletRpcError::invalid_request_payload;
    let method = Method::from_name(method).ok_or_else(invalid)?;
    let params = match params {
        None => None,
        Some(text) => Some(parse_within(text, policy.for_method(method)).ok_or_else(invalid)?),
    };
    let params = params.as_ref();
    match method {
        Method::RequestAccounts => request_accounts(params),
        Method::GetPermissions => api_version_param(params).map(|_| Request::GetPermissions),
        Method::RequestChainId => api_version_param(params).map(|_| Request::RequestChainId),
        Method::SupportedSpecs => {
            api_version_param(params).map(|api_version| Request::SupportedSpecs { api_version })
        }
        Method::SupportedWalletApi => {
            api_version_param(params).map(|api_version| Request::SupportedWalletApi { api_version })
        }
        Method::DeploymentData => {
            api_version_param(params).map(|api_version| Request::DeploymentData { api_version })
        }
        Method::SwitchStarknetChain => switch_chain(params),
        Method::SignTypedData => typed_data(params)
            .map(|typed_data| Request::SignTypedData { typed_data })
            .ok_or_else(invalid),
        Method::AddInvokeTransaction => match invoke_params(params, &policy.invoke_limits) {
            None => Err(invalid()),
            Some(invoke) if invoke.has_proof => Err(WalletRpcError::unknown_error()),
            Some(invoke) => Ok(Request::AddInvoke {
                calls: invoke.calls,
            }),
        },
        Method::AddDeclareTransaction => declare(params).ok_or_else(invalid),
        Method::AddStarknetChain => {
            add_chain(params, policy.reserved_network_names, policy.max_chain_urls)
                .map(Request::AddChain)
                .ok_or_else(invalid)
        }
        Method::WatchAsset => watch_asset(params)
            .map(Request::WatchAsset)
            .ok_or_else(invalid),
        Method::Strk20Balances
        | Method::Strk20InvokeTransaction
        | Method::Strk20PrepareInvoke
        | Method::Strk20ShadowAccountCommitment => Err(WalletRpcError::unknown_error()),
    }
}

/// `API_VERSION_PATTERN`: `X.Y` or `X.Y.Z`, ASCII digits, no pre-release suffix. Shape only;
/// whether the wallet supports it is a separate question.
pub fn is_api_version(value: &str) -> bool {
    let parts: Vec<&str> = value.split('.').collect();
    (2..=3).contains(&parts.len())
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

/// `isOptionalApiVersion`: absent, or a string of the right shape.
pub(super) fn optional_api_version(value: Option<&JsValue>) -> bool {
    match value {
        None => true,
        Some(value) => value.as_str().is_some_and(is_api_version),
    }
}

/// `hasOnlyKnownKeys` over a parsed object.
fn only_keys<'a>(params: &'a JsValue, keys: &[&str]) -> Option<&'a [(String, JsValue)]> {
    let entries = params.as_object()?;
    entries
        .iter()
        .all(|(key, _)| keys.contains(&key.as_str()))
        .then_some(entries)
}

/// `silent_mode`, if present, must be a boolean.
fn silent_mode(params: &JsValue) -> Option<bool> {
    match params.get("silent_mode") {
        None => Some(false),
        Some(JsValue::Bool(silent)) => Some(*silent),
        Some(_) => None,
    }
}

/// `readApiVersionParam`: no params, or `{ api_version? }` alone.
fn api_version_param(params: Option<&JsValue>) -> Result<Option<String>, WalletRpcError> {
    let Some(params) = params else {
        return Ok(None);
    };
    only_keys(params, &["api_version"])
        .filter(|_| optional_api_version(params.get("api_version")))
        .map(|_| {
            params
                .get("api_version")
                .and_then(JsValue::as_str)
                .map(str::to_owned)
        })
        .ok_or_else(WalletRpcError::invalid_request_payload)
}

/// `isRequestAccountsParams`.
fn request_accounts(params: Option<&JsValue>) -> Result<Request, WalletRpcError> {
    let Some(params) = params else {
        return Ok(Request::RequestAccounts { silent: false });
    };
    only_keys(params, &["api_version", "silent_mode"])
        .filter(|_| optional_api_version(params.get("api_version")))
        .and_then(|_| silent_mode(params))
        .map(|silent| Request::RequestAccounts { silent })
        .ok_or_else(WalletRpcError::invalid_request_payload)
}

/// `providerSwitchChainId`: `{ chainId, silent_mode?, api_version? }`, the chain id as
/// `isHexChainId` (`0x` and lowercase hex).
fn switch_chain(params: Option<&JsValue>) -> Result<Request, WalletRpcError> {
    params
        .and_then(|params| {
            only_keys(params, &["api_version", "chainId", "silent_mode"]).map(|_| params)
        })
        .filter(|params| silent_mode(params).is_some())
        .filter(|params| optional_api_version(params.get("api_version")))
        .and_then(|params| params.get("chainId").and_then(JsValue::as_str))
        .filter(|chain_id| is_hex_chain_id(chain_id))
        .map(|chain_id| Request::SwitchChain {
            chain_id: chain_id.to_owned(),
        })
        .ok_or_else(WalletRpcError::invalid_request_payload)
}

fn is_hex_chain_id(value: &str) -> bool {
    value.strip_prefix("0x").is_some_and(|digits| {
        !digits.is_empty()
            && digits
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// `readProviderDeclare`: `{ declare_transaction: { compiled_class_hash, contract_class,
/// class_hash? }, api_version? }`, each hash in the lowercase-hex felt grammar and the contract
/// class a non-empty object. The extension also re-checks the class against the contract-class
/// budget; the whole params already passed that budget, and no part can exceed its whole.
fn declare(params: Option<&JsValue>) -> Option<Request> {
    let params = params?;
    only_keys(params, &["api_version", "declare_transaction"])?;
    if !optional_api_version(params.get("api_version")) {
        return None;
    }
    let declare = params.get("declare_transaction")?;
    only_keys(
        declare,
        &["class_hash", "compiled_class_hash", "contract_class"],
    )?;
    let felt = |key: &str| declare.get(key)?.as_str().and_then(crate::felt::parse);
    let compiled_class_hash = felt("compiled_class_hash")?;
    // A present `class_hash`, `null` included, must be a felt.
    let class_hash = match declare.get("class_hash") {
        None => None,
        Some(_) => Some(felt("class_hash")?),
    };
    let contract_class = declare.get("contract_class")?;
    if contract_class.as_object()?.is_empty() {
        return None;
    }
    Some(Request::AddDeclare {
        class_hash,
        compiled_class_hash,
        contract_class: contract_class.clone(),
    })
}

/// `readTypedDataParam`: the spec's `{ typed_data }`, the legacy `{ typedData }`, or the typed
/// data itself, each beside an optional well-formed `api_version`, which a bare object loses.
/// A `null` anywhere it would be returned refuses the request, as the extension's sentinel does.
fn typed_data(params: Option<&JsValue>) -> Option<JsValue> {
    let params = params?;
    let found = match params.as_object() {
        None => params.clone(),
        Some(entries) => {
            if !optional_api_version(params.get("api_version")) {
                return None;
            }
            if let Some(inner) = params.get("typed_data").or_else(|| params.get("typedData")) {
                inner.clone()
            } else {
                JsValue::Object(
                    entries
                        .iter()
                        .filter(|(key, _)| key != "api_version")
                        .cloned()
                        .collect(),
                )
            }
        }
    };
    (found != JsValue::Null).then_some(found)
}

#[cfg(test)]
#[path = "request_tests.rs"]
pub(super) mod tests;
