//! Gasless fees, sans-IO: a public transaction whose network fee is paid in a token other than
//! STRK, through the wallet gateway's paymaster (the extension's `transactions/src/gasless/` and
//! the `routing/outside-execution-window.ts` and `paymaster-http.ts` it shares). The gateway builds
//! a SNIP-9 v2 outside execution of the person's calls plus one fee transfer; the wallet checks it
//! (`typed_data`) before anything is signed and posts the gateway's body unchanged with the
//! signature.
//!
//! The core builds each JSON-RPC exchange (`request`) and judges what came back (`rpc`,
//! `request`); the host moves bytes, repeats a `screening_unavailable` answer up to the policy's
//! retries, checks the session is still unlocked before each step, and signs
//! [`CheckedQuote::message_hash`] under the account's key (`kms::sign_stark_hash`, then
//! `Account::signature`). The message hash is Krusty's SNIP-12 hash
//! (`kms::typed_data_message_hash`); this module hashes nothing itself.
//!
//! Trust boundary: the route URL, the forwarder and every number come from the wallet's
//! [`GaslessPolicy`] (Bramble's is `bramble-core`'s `gasless::GASLESS_POLICY`), never from a dapp
//! or the gateway. Every reply is untrusted. Which account classes sign SNIP-9 v2 is the wallet's
//! list too ([`signs_gasless_v2`]).

mod offer;
mod reference;
mod request;
mod round;
mod rpc;
mod submit;
mod submit_round;
mod typed_data;

pub use offer::{strk_affordability, token_spend, GaslessFeeOffer, GaslessHeldToken};
pub use reference::{convert_strk_fee_to_token, fee_reference, scaled_price, MAX_TOKEN_DECIMALS};
pub use request::{
    read_available, read_quote, read_supported_tokens, read_transaction_hash, GaslessAccount,
    GaslessKind, GaslessSession, ListedToken,
};
pub use round::{OfferRound, RoundStep};
pub use rpc::{classify, Exchange, Exchanged, GaslessAnswer};
pub use submit::{held_for, CheckedQuote, GaslessSubmission, PendingOffer, PendingSubmit, UserFee};
pub use submit_round::{SubmitRound, SubmitStep};
pub use typed_data::{
    validate_typed_data, Accepted, Refusal, TypedDataCheck, OUTSIDE_EXECUTION_DOMAIN,
};

use crate::accounts::AccountFamily;
use crate::portfolio::model::DisplayCurrency;
use crate::send::u256::U256;
use crate::{Felt, NetworkId};

/// The JSON-RPC id every gasless request sends and every reply must echo (`GASLESS_RPC_ID`).
pub const GASLESS_RPC_ID: u64 = 1;

/// One network's gasless route (`GaslessRoute`): the gateway URL, the wallet's switch and the
/// pinned forwarder. `forwarder: None` or `enabled: false` refuses the route before any request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GaslessRoute {
    pub network: NetworkId,
    pub url: &'static str,
    pub enabled: bool,
    pub forwarder: Option<Felt>,
}

/// A wallet's gasless choices. A network with no route has no gasless fees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GaslessPolicy {
    pub routes: &'static [GaslessRoute],
    /// The longest execution window the wallet signs (`GASLESS_MAX_WINDOW_SECONDS`), also the
    /// window a build asks for.
    pub max_window_seconds: u64,
    /// What the window must still have when the signature is sent (`GASLESS_SIGN_WINDOW_SECONDS`).
    pub sign_window_seconds: u64,
    /// The independent ceiling is `reference * numerator / denominator`, rounded up.
    pub ceiling_numerator: u64,
    pub ceiling_denominator: u64,
    /// How many tokens one review may offer (`MAX_GASLESS_OFFERS`).
    pub max_offers: usize,
    /// How many times the host repeats a `screening_unavailable` answer
    /// (`GASLESS_SCREENING_RETRIES`); the n-th repeat waits `250 * n` ms.
    pub screening_retries: u32,
    /// The deadline on one exchange (`PAYMASTER_TIMEOUT_MS`).
    pub deadline_ms: u64,
    /// The largest reply read (`PAYMASTER_MAX_RESPONSE_BYTES`).
    pub max_body_bytes: u64,
    /// The currency both legs of the reference are priced in (`REFERENCE_CURRENCY`). It cancels,
    /// so it is pinned rather than the person's display choice.
    pub reference_currency: DisplayCurrency,
    /// What a fiat price is multiplied by before it is rounded to an integer (`PRICE_SCALE`).
    pub price_scale: u64,
}

impl GaslessPolicy {
    /// `gaslessRoute` then `gaslessRouteReady`: the route and its forwarder, or `None` when the
    /// network has none, it is switched off or its forwarder is unpinned.
    pub fn ready_route(&self, network: NetworkId) -> Option<(&GaslessRoute, Felt)> {
        let route = self.routes.iter().find(|route| route.network == network)?;
        let forwarder = route.forwarder.filter(|_| route.enabled)?;
        Some((route, forwarder))
    }

    /// `gaslessFeeCeiling`: the most the wallet signs for `reference` base units of the fee
    /// token, rounded up so the margin is not shortened by division. Zero (which refuses every
    /// quote) for a zero reference. A product past 2^256 saturates: no fee transfer is above it,
    /// as none is above the extension's unbounded `bigint`.
    pub fn fee_ceiling(&self, reference: U256) -> U256 {
        let denominator = self.ceiling_denominator;
        if reference == U256::ZERO || denominator == 0 {
            return U256::ZERO;
        }
        let max = U256 {
            high: u128::MAX,
            low: u128::MAX,
        };
        // ceil((q * d + r) * n / d) = q * n + ceil(r * n / d), with r < d, so only q * n can
        // overflow, and then the exact value is past 2^256 too.
        let Some((quotient, remainder)) = reference.checked_div_rem_small(denominator) else {
            return U256::ZERO;
        };
        let (numerator, denominator) =
            (u128::from(self.ceiling_numerator), u128::from(denominator));
        let tail = (u128::from(remainder) * numerator).div_ceil(denominator);
        quotient
            .checked_mul_small(self.ceiling_numerator)
            .and_then(|scaled| scaled.checked_add(U256::from_u128(tail)))
            .unwrap_or(max)
    }

    /// `gaslessAuthorizationWindow`: the window is live at `now` (`isPaymasterAuthorizationLive`:
    /// after `execute_after`, before `execute_before`, with at least `minimum_remaining` left)
    /// and ends no more than `max_window_seconds` from now.
    pub fn window_is_live(
        &self,
        execute_after: u64,
        execute_before: u64,
        now: u64,
        minimum_remaining: u64,
    ) -> bool {
        now <= MAX_SAFE_INTEGER
            && execute_after < now
            && now < execute_before
            && execute_before - now >= minimum_remaining
            && execute_before - now <= self.max_window_seconds
    }
}

/// `Number.MAX_SAFE_INTEGER`: a clock past it is not a time the extension can hold.
const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

/// `accountSignsGaslessV2`: whether an account of `family` running `class_hash` may be offered a
/// gasless fee. `v2_classes` is the wallet's list of (family, class) pairs verified for SNIP-9 v2.
/// Braavos never is (its outside-execution nonce view is not verified for v2), and a multisig is
/// not an `AccountFamily`, so it never reaches here.
pub fn signs_gasless_v2(
    family: AccountFamily,
    class_hash: &Felt,
    v2_classes: &[(AccountFamily, Felt)],
) -> bool {
    family != AccountFamily::Braavos && v2_classes.contains(&(family, *class_hash))
}

/// A gateway `error` code (`GASLESS_GATEWAY_CODES`), the `unrecognised` bucket, or a local
/// refusal that never reached the gateway. `Display` is the extension's sentence (`messageFor`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GaslessCode {
    #[error("This account can't pay the network fee in another token.")]
    AddressNotAllowed,
    #[error("This account can't pay the network fee in another token.")]
    ScreeningDenied,
    #[error("The network fee check is still pending. Nothing was submitted.")]
    ScreeningPending,
    #[error("Deploying on Mainnet needs STRK.")]
    ScreeningSubjectUnverifiable,
    #[error("The network fee could not be checked. Nothing was submitted.")]
    ScreeningUnavailable,
    #[error("Paying the network fee in another token is unavailable.")]
    GaslessDisabled,
    #[error("Paying the network fee in another token is unavailable.")]
    Unavailable,
    /// The account's class is not one the wallet signs SNIP-9 v2 for.
    #[error("Paying the network fee in another token is unavailable.")]
    V2Unsupported,
    /// No independent price reference for the fee token.
    #[error("This token's network fee could not be checked.")]
    Unverifiable,
    #[error("This network fee quote was refused. Nothing was submitted.")]
    AboveCeiling,
    #[error("This network fee quote was refused. Nothing was submitted.")]
    InvalidGaslessRequest,
    #[error("This network fee quote was refused. Nothing was submitted.")]
    InvalidScreeningTransaction,
    #[error("This network fee quote was refused. Nothing was submitted.")]
    UpstreamRejected,
    #[error("This network fee quote was refused. Nothing was submitted.")]
    Unrecognised,
}

impl GaslessCode {
    /// `isGaslessGatewayCode`: a code the gateway writes that this client names; any other
    /// string is `None`, never guessed into a known code.
    pub fn from_gateway(code: &str) -> Option<Self> {
        Some(match code {
            "address_not_allowed" => Self::AddressNotAllowed,
            "gasless_disabled" => Self::GaslessDisabled,
            "invalid_gasless_request" => Self::InvalidGaslessRequest,
            "invalid_screening_transaction" => Self::InvalidScreeningTransaction,
            "screening_denied" => Self::ScreeningDenied,
            "screening_pending" => Self::ScreeningPending,
            "screening_subject_unverifiable" => Self::ScreeningSubjectUnverifiable,
            "screening_unavailable" => Self::ScreeningUnavailable,
            "upstream_rejected" => Self::UpstreamRejected,
            _ => return None,
        })
    }
}

/// How a gasless step failed. Only `Lost` may have left a signed execution with the relay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum GaslessError {
    /// A determinate refusal: nothing was accepted. `retryable` is true only for
    /// `screening_unavailable` once the host's repeats are spent (`GaslessRefusal`).
    #[error("{code}")]
    Refused { code: GaslessCode, retryable: bool },
    /// The execute answer could not be read (`IndeterminateRpcWriteError`).
    #[error(
        "Transaction submission status is unknown. Check activity and on-chain state before retrying."
    )]
    Lost,
    /// The fresh quote's fee is above the amount the person reviewed (`REVIEWED_FEE_EXCEEDED`).
    #[error(
        "The network fee for this transaction rose above the amount you reviewed. Nothing was submitted — review it again."
    )]
    ReviewedFeeExceeded,
}

impl From<GaslessCode> for GaslessError {
    fn from(code: GaslessCode) -> Self {
        Self::Refused {
            code,
            retryable: false,
        }
    }
}

#[cfg(test)]
mod tests;
