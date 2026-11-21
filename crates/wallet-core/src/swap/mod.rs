//! Token swaps through AVNU, sans-IO: the extension's `transactions/src/swap/avnu.ts` (the quote
//! and build requests and their reply judges), `wallet-model/src/swap/avnu-route.ts` (the route
//! binding and the minimum-output floor) and the background's `swap-{inputs,build-policy,
//! value-check,ops,preparations}.ts` (the input rules, the build policy, the value check and the
//! preparation). The core builds each request for the host to perform as described
//! (`portfolio::wire`) and judges what came back; it never signs: a judged build is a reviewed
//! `send::invoke::Invoke` for `send::plan::Plan`.
//!
//! AVNU is an untrusted third party whose build decides what the person signs, so every field of
//! every reply is hostile: a body is capped before it is parsed, every count before it is walked,
//! and the whole call set must be one exact approval of the sell token to the router the WALLET
//! pinned for the network, plus one `multi_route_swap` on that router whose decoded route sells
//! exactly the reviewed amount of the reviewed token, buys the reviewed token, pays this account
//! and promises at least the floor the reviewed quote implies. Each value the flow carries
//! forward (`PendingQuote`, `SwapQuote`, `PendingBuild`, `SwapPreparation`) is bound to the
//! account and network it was made for, and refuses any other.
//!
//! Which aggregator deployments, slippage and limits apply is the wallet's `SwapPolicy`.

mod build;
mod policy;
mod prepare;
mod quote;

pub use build::PendingBuild;
pub use policy::{reviewed_min_buy_amount, BuildPolicyError, APPROVE_ENTRYPOINTS};
pub use prepare::{SwapActivity, SwapPreparation};
pub use quote::{PendingQuote, SwapQuote};

use crate::js::json::{self, JsValue};
use crate::portfolio::wire::HttpOutcome;
use crate::send::invoke::InvokeRefusal;
use crate::send::u256::U256;
use crate::{Felt, NetworkId};

/// Swap-aggregator quotes and builds (`HTTP_BODY_LIMITS.swap`).
pub const SWAP_BODY_LIMIT: u64 = 1024 * 1024;

/// One aggregator deployment the wallet pinned: the API it talks to and the exchange (router)
/// contract that API may make the account approve and call. One record, so a base URL never
/// exists without the counterparty that makes its builds reviewable (`AVNU_DEPLOYMENTS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AvnuDeployment {
    pub network: NetworkId,
    pub base_url: &'static str,
    pub router: Felt,
}

/// A wallet's swap choices. A network with no deployment has no swaps.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SwapPolicy {
    pub deployments: &'static [AvnuDeployment],
    /// The slippage a preparation asks for when the person names none.
    pub default_slippage: f64,
    /// The largest slippage a preparation may ask for; it must also be above zero.
    pub max_slippage: f64,
    /// Headroom under the floor for the aggregator's floating-point minimum, in basis points.
    pub slippage_rounding_tolerance_bps: u64,
    /// The largest integrator fee a route may carry, in basis points.
    pub max_integrator_fee_bps: u128,
    /// The most calls one build may return.
    pub max_build_calls: usize,
    /// The most calldata felts one built call may carry.
    pub max_call_calldata: usize,
    /// The longest quote id accepted, in UTF-16 units.
    pub max_quote_id_length: usize,
}

impl SwapPolicy {
    /// `deploymentFor`: the pinned deployment, or the refusal before any request.
    fn deployment(&self, network: NetworkId) -> Result<&AvnuDeployment, SwapRefusal> {
        self.deployments
            .iter()
            .find(|deployment| deployment.network == network)
            .ok_or(SwapRefusal::Unavailable)
    }
}

/// Why a swap step refused. `Display` is the extension's sentence; none carries an address, an
/// amount or reply text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SwapRefusal {
    #[error("swaps are not available on this network")]
    Unavailable,
    /// The amount is not decimal text.
    #[error("invalid swap quote request")]
    InvalidQuoteRequest,
    /// A token address that is not a felt.
    #[error("token unavailable on this network")]
    TokenUnavailable,
    #[error("pick two different tokens")]
    SameToken,
    /// Zero, more decimals than the token has, or 2^256 and above.
    #[error("enter an amount to swap")]
    NoAmount,
    /// The quote exchange failed, answered non-2xx, ran over the cap or was not JSON.
    #[error("swap quote failed")]
    QuoteFailed,
    #[error("no swap route available for this pair")]
    NoRoute,
    #[error("invalid swap quote")]
    InvalidQuote,
    #[error("swap quote does not match the requested pair")]
    QuoteMismatch,
    /// A quote id outside `[A-Za-z0-9-]`, or a slippage outside the policy's range.
    #[error("invalid swap preparation request")]
    InvalidPrepareRequest,
    /// No floor can be stated for the quote (a zero buy amount).
    #[error("swap quote is not reviewable — request a fresh price")]
    NotReviewable,
    #[error("swap build failed")]
    BuildFailed,
    #[error("invalid swap build response")]
    InvalidBuild,
    #[error("invalid swap call")]
    InvalidCall,
    /// A value reviewed for one account or network was offered for another.
    #[error("swap preparation changed")]
    PreparationChanged,
    #[error("{0}")]
    Policy(#[from] BuildPolicyError),
    #[error("{0}")]
    Invoke(#[from] InvokeRefusal),
}

/// `fetchBoundedJson`'s judgement of one exchange: a 2xx status and a body within the cap
/// (checked before anything is parsed) that is JSON. Anything else is `failed`.
fn read_json(outcome: &HttpOutcome, failed: SwapRefusal) -> Result<JsValue, SwapRefusal> {
    let body = outcome.body.as_deref().ok_or(failed)?;
    if !(200..300).contains(&outcome.status) || body.len() as u64 > SWAP_BODY_LIMIT {
        return Err(failed);
    }
    json::parse(&json::decode_utf8(body)).ok_or(failed)
}

/// `isFelt` over a JSON value: a string in the strict felt grammar.
fn felt_value(value: Option<&JsValue>) -> Option<Felt> {
    crate::felt::parse(value?.as_str()?)
}

/// `0x` + `toString(16)`.
fn u256_hex(value: U256) -> String {
    if value.high == 0 {
        format!("{:#x}", value.low)
    } else {
        format!("{:#x}{:032x}", value.high, value.low)
    }
}

/// `isHexAmount`: `0x` + 1–64 hex digits in either case, as a u256.
fn hex_u256(text: &str) -> Option<U256> {
    U256::from_hex(text.strip_prefix("0x")?)
}

#[cfg(test)]
mod fixtures;
