//! `gasless-request.ts` and the gates of `gasless-submit.ts`: who may use the route
//! ([`GaslessSession::open`]), the `paymaster_*` bodies, and the readers of their results. The
//! posted params are `{ parameters, transaction }` only (a top-level `userAddress` is refused as
//! `invalid_gasless_request`); version `0x1`, fee mode `default` with no tip, and `time_bounds`
//! as JSON numbers inside the window the wallet signs. Sponsored modes are not representable.

use super::rpc::post;
use super::typed_data::selector_of;
use super::{signs_gasless_v2, GaslessCode, GaslessPolicy};
use crate::accounts::AccountFamily;
use crate::js::felt::canonical_felt;
use crate::js::json::JsValue;
use crate::portfolio::wire::HttpRequest;
use crate::send::calls::Call;
use crate::send::sign::Deployment;
use crate::send::u256::U256;
use crate::{felt, Felt, NetworkId};
use serde_json::{json, Value};

/// `Number.MAX_SAFE_INTEGER`.
const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

/// Which build the gateway is asked for. A deployed account invokes; an undeployed one deploys
/// in the same execution, which only Sepolia's route may do (Mainnet screens the subject first).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GaslessKind {
    Invoke,
    DeployAndInvoke,
}

impl GaslessKind {
    fn wire(self) -> &'static str {
        match self {
            Self::Invoke => "invoke",
            Self::DeployAndInvoke => "deploy_and_invoke",
        }
    }
}

/// The account a gasless fee is for (`GaslessAccountClass` and its address). `deployed` is the
/// chain's answer; `deployment` is the account's own DEPLOY_ACCOUNT, used only when it is not.
#[derive(Debug, Clone, Copy)]
pub struct GaslessAccount<'a> {
    pub network: NetworkId,
    pub address: Felt,
    pub family: AccountFamily,
    pub class_hash: Felt,
    pub deployed: bool,
    pub deployment: Option<&'a Deployment>,
}

/// One supported token (`GaslessListedToken`). The paymaster's price is never read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListedToken {
    pub address: Felt,
    pub decimals: u8,
}

/// A batch the route may carry, for one account on one network: every gate the extension checks
/// before its first request passed. Built per review or submission; nothing in it is secret.
#[derive(Debug, Clone, PartialEq)]
pub struct GaslessSession<'p> {
    pub(super) policy: &'p GaslessPolicy,
    pub(super) network: NetworkId,
    pub(super) url: &'static str,
    pub(super) forwarder: Felt,
    pub(super) account: Felt,
    pub(super) kind: GaslessKind,
    pub(super) calls: Vec<Call>,
    /// `{ invoke: { calls, user_address } }` minus the type: the build's encoded calls.
    pub(super) invoke_calls: Value,
    /// `gaslessDeploymentBody`, for a deploy-and-invoke.
    pub(super) deployment: Option<Value>,
}

/// `encodeCall`: `0x` felts, the selector by `selectorOf`. A zero address or selector refuses.
fn encode_call(call: &Call) -> Option<Value> {
    let selector = selector_of(&call.entrypoint);
    if call.contract_address == Felt::ZERO || selector == Felt::ZERO {
        return None;
    }
    let calldata: Vec<String> = call.calldata.iter().map(felt::canonical).collect();
    Some(json!({
        "calldata": calldata,
        "selector": felt::canonical(&selector),
        "to": felt::canonical(&call.contract_address),
    }))
}

/// `gaslessDeploymentBody`: `version` is the JSON number 1. `None` when the deployment's address
/// is not the invoking account.
fn deployment_body(deployment: &Deployment, account: &Felt) -> Option<Value> {
    if deployment.address()? != *account || deployment.class_hash == Felt::ZERO {
        return None;
    }
    let calldata: Vec<String> = deployment
        .constructor_calldata
        .iter()
        .map(felt::canonical)
        .collect();
    Some(json!({
        "address": felt::canonical(account),
        "calldata": calldata,
        "class_hash": felt::canonical(&deployment.class_hash),
        "salt": felt::canonical(&deployment.salt),
        "version": 1,
    }))
}

impl<'p> GaslessSession<'p> {
    /// The gates (`quoteGaslessOffers`, `prepareSubmission`), in the extension's order: the
    /// route is on and pinned (`Unavailable`), the account's class signs SNIP-9 v2
    /// (`V2Unsupported`), an undeployed account is not on Mainnet
    /// (`ScreeningSubjectUnverifiable`), and its deployment reproduces its address on Sepolia
    /// (`Unavailable`). An offer treats every refusal as "no offer"; a submission shows it.
    pub fn open(
        policy: &'p GaslessPolicy,
        account: &GaslessAccount<'_>,
        calls: &[Call],
        v2_classes: &[(AccountFamily, Felt)],
    ) -> Result<Self, GaslessCode> {
        let (route, forwarder) = policy
            .ready_route(account.network)
            .ok_or(GaslessCode::Unavailable)?;
        if !signs_gasless_v2(account.family, &account.class_hash, v2_classes) {
            return Err(GaslessCode::V2Unsupported);
        }
        let (kind, deployment) = if account.deployed {
            (GaslessKind::Invoke, None)
        } else if account.network == NetworkId::Mainnet {
            return Err(GaslessCode::ScreeningSubjectUnverifiable);
        } else {
            let body = account
                .deployment
                .and_then(|deployment| deployment_body(deployment, &account.address))
                .ok_or(GaslessCode::Unavailable)?;
            (GaslessKind::DeployAndInvoke, Some(body))
        };
        if calls.is_empty() || account.address == Felt::ZERO {
            return Err(GaslessCode::Unavailable);
        }
        let encoded = calls
            .iter()
            .map(encode_call)
            .collect::<Option<Vec<_>>>()
            .ok_or(GaslessCode::Unavailable)?;
        Ok(Self {
            policy,
            network: account.network,
            url: route.url,
            forwarder,
            account: account.address,
            kind,
            calls: calls.to_vec(),
            invoke_calls: Value::Array(encoded),
            deployment,
        })
    }

    pub(super) fn kind_wire(&self) -> &'static str {
        self.kind.wire()
    }

    /// `paymaster_isAvailable`, read by [`read_available`].
    pub fn available_request(&self) -> HttpRequest {
        post(self.policy, self.url, "paymaster_isAvailable", "[]")
    }

    /// `paymaster_getSupportedTokens`, read by [`read_supported_tokens`].
    pub fn supported_tokens_request(&self) -> HttpRequest {
        post(self.policy, self.url, "paymaster_getSupportedTokens", "[]")
    }

    /// `parameters` of a build and its execute: fee mode `default` in `gas_token`, after 1 and
    /// before `now` plus the policy's window. `Unavailable` for a clock that is not a safe integer.
    pub(super) fn parameters(&self, gas_token: &Felt, now: u64) -> Result<Value, GaslessCode> {
        let before = now
            .checked_add(self.policy.max_window_seconds)
            .filter(|before| *before <= MAX_SAFE_INTEGER)
            .ok_or(GaslessCode::Unavailable)?;
        if *gas_token == Felt::ZERO {
            return Err(GaslessCode::Unavailable);
        }
        Ok(json!({
            "fee_mode": {"gas_token": felt::canonical(gas_token), "mode": "default"},
            "time_bounds": {"execute_after": 1, "execute_before": before},
            "version": "0x1",
        }))
    }

    /// `transaction`, with `invoke` (`{calls, user_address}` on a build; the signed body on an
    /// execute) and the deployment when there is one.
    pub(super) fn transaction(&self, invoke: Value) -> Value {
        let mut transaction = json!({"invoke": invoke, "type": self.kind.wire()});
        if let (Some(deployment), Some(object)) = (&self.deployment, transaction.as_object_mut()) {
            object.insert("deployment".into(), deployment.clone());
        }
        transaction
    }

    /// `gaslessBuildParams` posted as `paymaster_buildTransaction`.
    pub(super) fn build_request(
        &self,
        gas_token: &Felt,
        now: u64,
    ) -> Result<HttpRequest, GaslessCode> {
        let invoke = json!({
            "calls": self.invoke_calls,
            "user_address": felt::canonical(&self.account),
        });
        let params = json!({
            "parameters": self.parameters(gas_token, now)?,
            "transaction": self.transaction(invoke),
        });
        Ok(post(
            self.policy,
            self.url,
            "paymaster_buildTransaction",
            &params.to_string(),
        ))
    }

    /// [`read_quote`] for the build this session asked for.
    pub(super) fn read_quote<'v>(&self, value: &'v JsValue) -> Option<(U256, &'v JsValue)> {
        read_quote(value, self.kind)
    }
}

/// `readGaslessQuote`: the quoted token maximum and the typed data to check, or `None` when the
/// result is not a `kind` build. The echoed `parameters` are not read.
pub fn read_quote(value: &JsValue, kind: GaslessKind) -> Option<(U256, &JsValue)> {
    const FEE_KEYS: [&str; 5] = [
        "estimated_fee_in_gas_token",
        "estimated_fee_in_strk",
        "gas_token_price_in_strk",
        "suggested_max_fee_in_gas_token",
        "suggested_max_fee_in_strk",
    ];
    let keys = ["deployment", "fee", "parameters", "type", "typed_data"];
    let fee = value.get("fee")?;
    let felt_at = |key: &str| felt::parse(fee.get(key)?.as_str()?);
    let well_formed = only_keys(value, &keys)
        && value.get("type")?.as_str()? == kind.wire()
        && matches!(value.get("parameters"), Some(JsValue::Object(_)))
        && (kind == GaslessKind::DeployAndInvoke || value.get("deployment").is_none())
        && only_keys(fee, &FEE_KEYS)
        && FEE_KEYS.iter().all(|key| felt_at(key).is_some());
    let quoted_max = felt_at("suggested_max_fee_in_gas_token")?;
    let quoted_max = U256::from_hex(&format!("{quoted_max:x}"))?;
    well_formed.then_some((quoted_max, value.get("typed_data")?))
}

/// `hasOnlyKnownKeys` (and `isRecord`).
fn only_keys(value: &JsValue, keys: &[&str]) -> bool {
    value
        .as_object()
        .is_some_and(|entries| entries.iter().all(|(key, _)| keys.contains(&key.as_str())))
}

/// `readGaslessAvailable`: only `true` counts.
pub fn read_available(value: &JsValue) -> bool {
    *value == JsValue::Bool(true)
}

/// `decimalCount`: an integer from 0 to 36.
fn decimal_count(value: Option<&JsValue>) -> Option<u8> {
    let count = value?.as_f64()?;
    (count.fract() == 0.0 && (0.0..=36.0).contains(&count)).then_some(count as u8)
}

fn listed_token(item: &JsValue) -> Option<ListedToken> {
    if !only_keys(item, &["decimals", "price_in_strk", "token_address"]) {
        return None;
    }
    if let Some(price) = item.get("price_in_strk") {
        felt::parse(price.as_str()?)?;
    }
    let address = canonical_felt(item.get("token_address")?.as_str()?)?;
    Some(ListedToken {
        address: felt::parse_non_zero(&address)?,
        decimals: decimal_count(item.get("decimals"))?,
    })
}

/// `readGaslessSupportedTokens`: address and decimals rows, the first row of an address kept
/// once. `None` for anything else, including one address listed with two decimal counts.
pub fn read_supported_tokens(value: &JsValue) -> Option<Vec<ListedToken>> {
    let mut tokens: Vec<ListedToken> = Vec::new();
    for item in value.as_array()? {
        let token = listed_token(item)?;
        match tokens.iter().find(|known| known.address == token.address) {
            Some(prior) if prior.decimals != token.decimals => return None,
            Some(_) => {}
            None => tokens.push(token),
        }
    }
    Some(tokens)
}

/// `readGaslessTransactionHash`: the hash an execute accepted, or `None`, which the caller treats
/// as a lost reply because the relay may already hold the execution.
pub fn read_transaction_hash(value: &JsValue) -> Option<Felt> {
    let hash = match value {
        JsValue::Object(_) => value.get("transaction_hash")?,
        other => other,
    };
    felt::parse_non_zero(hash.as_str()?)
}

#[cfg(test)]
#[path = "request_tests.rs"]
mod tests;
