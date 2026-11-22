//! The build: `readSwapPrepareInput`'s quote id and slippage rules, the reviewed floor,
//! `buildCalls`' request and reply judge (`readCall`), then the build policy and
//! `send::invoke::Invoke::reviewed` (`swapPrepareResponse`).

use super::policy::{build_policy, reviewed_min_buy_amount, Intent};
use super::{felt_value, read_json, SwapPreparation, SwapQuote, SwapRefusal, SWAP_BODY_LIMIT};
use crate::felt;
use crate::js::json::JsValue;
use crate::js::num::number_to_string;
use crate::portfolio::wire::{HttpHeader, HttpMethod, HttpOutcome, HttpRequest, HTTP_DEADLINE_MS};
use crate::send::calls::Call;
// `isEntrypoint` (`avnu.ts`, `^[a-zA-Z_][a-zA-Z0-9_]{0,63}$`) is the invoke grammar.
use crate::send::invoke::{is_entrypoint, Invoke};
use crate::send::multisig_invoke::StarkPrimitives;
use crate::send::review_flags::ReviewContext;
use crate::send::types::{Account, Policy};
use crate::send::u256::U256;
use crate::Felt;

/// `isQuoteIdText` (`swap-inputs.ts`): `^[A-Za-z0-9-]{1,120}$`. The quote judge already held
/// the id to 1..=`max_quote_id_length` UTF-16 units, which for these ASCII characters is bytes.
fn is_quote_id(id: &str) -> bool {
    id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn json_string(value: &str) -> String {
    serde_json::Value::String(value.to_string()).to_string()
}

/// A build request in flight, bound to the quote it builds, the account it was prepared for,
/// the router pinned for that account's network and the floor the reviewed quote implies.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingBuild {
    pub(super) quote: SwapQuote,
    pub(super) account: Account,
    pub(super) router: Felt,
    pub(super) floor: U256,
}

impl SwapQuote {
    /// Prepares a build of this quote for `account` (the one it was quoted for, on the same
    /// network) at `slippage` (the policy's default when `None`).
    pub fn prepare(
        &self,
        account: &Account,
        slippage: Option<f64>,
    ) -> Result<(PendingBuild, HttpRequest), SwapRefusal> {
        let request = &self.request;
        let policy = request.policy;
        if account.network != request.network || account.address != request.taker {
            return Err(SwapRefusal::PreparationChanged);
        }
        let slippage = slippage.unwrap_or(policy.default_slippage);
        if !is_quote_id(&self.quote_id) || !(slippage > 0.0 && slippage <= policy.max_slippage) {
            return Err(SwapRefusal::InvalidPrepareRequest);
        }
        let floor = reviewed_min_buy_amount(
            self.buy_amount,
            slippage,
            policy.slippage_rounding_tolerance_bps,
        )
        .ok_or(SwapRefusal::NotReviewable)?;
        let deployment = policy.deployment(request.network)?;
        let body = format!(
            r#"{{"includeApprove":true,"quoteId":{},"slippage":{},"takerAddress":{}}}"#,
            json_string(&self.quote_id),
            number_to_string(slippage),
            json_string(&felt::canonical(&account.address)),
        );
        let http = HttpRequest {
            method: HttpMethod::Post,
            url: format!("{}/swap/v3/build", deployment.base_url),
            headers: vec![HttpHeader {
                name: "content-type".into(),
                value: "application/json".into(),
            }],
            body: Some(body),
            max_body_bytes: SWAP_BODY_LIMIT,
            deadline_ms: HTTP_DEADLINE_MS,
        };
        let pending = PendingBuild {
            quote: self.clone(),
            account: account.clone(),
            router: deployment.router,
            floor,
        };
        Ok((pending, http))
    }
}

impl PendingBuild {
    /// Judges the build reply: the envelope for this network, every call in its grammar, the
    /// build policy against the bound intent, then the batch review (`Invoke::reviewed` under
    /// `send_policy`, with everything the wallet knows on this network). The review's
    /// `known_contracts` are this build's pinned router alone, as the extension's
    /// (`knownContracts: [routerAddress]`): the policy admits no other contract to pin.
    pub fn judge(
        self,
        outcome: &HttpOutcome,
        context: &ReviewContext,
        send_policy: &Policy,
        primitives: &dyn StarkPrimitives,
    ) -> Result<SwapPreparation, SwapRefusal> {
        let payload = read_json(outcome, SwapRefusal::BuildFailed)?;
        let calls = self.read_build(&payload)?;
        let request = &self.quote.request;
        let route = build_policy(
            &calls,
            &Intent {
                sell_token: request.sell.address,
                sell_amount: request.sell_amount,
                buy_token: request.buy.address,
                taker: self.account.address,
                router: self.router,
                min_buy_amount: self.floor,
                max_calls: request.policy.max_build_calls,
                max_fee_bps: request.policy.max_integrator_fee_bps,
            },
        )?;
        let context = ReviewContext {
            known_contracts: std::slice::from_ref(&self.router),
            ..*context
        };
        let invoke = Invoke::reviewed(&self.account, calls, send_policy, &context, primitives)?;
        Ok(SwapPreparation::new(self, invoke, &route))
    }

    /// The build envelope: an object whose `chainId` is this network's and whose `calls` hold
    /// between one and the policy's most calls, counted before any call is read.
    fn read_build(&self, payload: &JsValue) -> Result<Vec<Call>, SwapRefusal> {
        let policy = self.quote.request.policy;
        let chain_id = felt_value(payload.get("chainId"));
        let calls = payload
            .get("calls")
            .and_then(JsValue::as_array)
            .filter(|calls| !calls.is_empty() && calls.len() <= policy.max_build_calls);
        match (chain_id, calls) {
            (Some(chain_id), Some(calls)) if chain_id == self.account.network.chain_id() => calls
                .iter()
                .map(|call| read_call(call, policy.max_call_calldata))
                .collect(),
            _ => Err(SwapRefusal::InvalidBuild),
        }
    }
}

/// `readCall`: a felt contract, an entrypoint in the grammar, and at most `max_calldata` felts,
/// counted before any is parsed.
fn read_call(value: &JsValue, max_calldata: usize) -> Result<Call, SwapRefusal> {
    let contract = felt_value(value.get("contractAddress"));
    let entrypoint = value
        .get("entrypoint")
        .and_then(JsValue::as_str)
        .filter(|name| is_entrypoint(name));
    let calldata = value
        .get("calldata")
        .and_then(JsValue::as_array)
        .filter(|calldata| calldata.len() <= max_calldata);
    let (Some(contract_address), Some(entrypoint), Some(calldata)) =
        (contract, entrypoint, calldata)
    else {
        return Err(SwapRefusal::InvalidCall);
    };
    let calldata = calldata
        .iter()
        .map(|felt| felt_value(Some(felt)))
        .collect::<Option<Vec<_>>>()
        .ok_or(SwapRefusal::InvalidCall)?;
    Ok(Call {
        contract_address,
        entrypoint: entrypoint.to_string(),
        calldata,
    })
}

#[cfg(test)]
#[path = "build_tests.rs"]
mod tests;
