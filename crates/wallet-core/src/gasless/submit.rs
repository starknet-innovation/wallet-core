//! `gasless-submit.ts` without its I/O: one token's offer (`offerForListed`, `heldOffer`) and the
//! reviewed submission (`prepareSubmission`, `submitReviewedGaslessInvoke`), each a build the
//! host posts and a judgement of its result.
//!
//! The host's half of an offer round: post `available_request` then `supported_tokens_request`,
//! and for each listed token in order, while fewer than `max_offers` offers were made, look up
//! [`held_for`], read the independent price reference, then post [`GaslessSession::offer_build`]
//! and judge it. Any failed exchange ends the round with no offers (the extension's `catch`).
//! Offers are made only when `strk_affordability` is `Insufficient`.
//!
//! A submission: [`GaslessSession::submit_build`], post, [`PendingSubmit::judge`], sign
//! [`CheckedQuote::message_hash`], read the clock again, persist the outside execution, then
//! [`CheckedQuote::execute_request`] and [`CheckedQuote::judge_execute`]. The host checks the
//! session is still unlocked before signing, before persisting and before the execute.
//!
//! The `reference` both builds take is the network fee in the fee token's base units by the
//! wallet's own price book: its STRK fee estimate converted at its own fiat prices for STRK and
//! the token (the extension's `convertStrkFeeToToken`, `gasless-ceiling.ts`). It must never come
//! from the paymaster's reply (`gas_token_price_in_strk`, a listed `price_in_strk` or any fee it
//! quotes): the ceiling exists to bound the paymaster, so a reference it supplied bounds nothing.
//! No reference means no offer and an `Unverifiable` submission.

use super::offer::token_affordability;
use super::request::{GaslessSession, ListedToken};
use super::rpc::{post, GaslessAnswer};
use super::typed_data::{validate_typed_data, Accepted, Refusal, TypedDataCheck};
use super::{
    read_transaction_hash, token_spend, GaslessCode, GaslessError, GaslessFeeOffer,
    GaslessHeldToken,
};
use crate::js::json::JsValue;
use crate::portfolio::wire::HttpRequest;
use crate::send::types::FeeAffordability;
use crate::send::u256::U256;
use crate::{felt, Felt, NetworkId};
use serde_json::{json, Value};

/// The signed fee a submission records (`GaslessSubmission.userFee`): the base units in the
/// signed transfer as decimal digits, which may be below the reviewed maximum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserFee {
    pub amount: String,
    pub token: String,
    pub token_address: String,
}

/// `GaslessSubmission`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GaslessSubmission {
    pub transaction_hash: Felt,
    pub user_fee: UserFee,
}

/// The wallet token a listed token may be offered as: same address and decimals, never STRK.
pub fn held_for<'h>(
    listed: &ListedToken,
    held: &'h [GaslessHeldToken],
    strk: &Felt,
) -> Option<&'h GaslessHeldToken> {
    held.iter()
        .find(|token| token.address == listed.address && token.decimals == listed.decimals)
        .filter(|token| token.address != *strk)
}

/// One token's build in flight, with the ceiling and spend it is judged against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingOffer {
    bound: Bound,
    held: GaslessHeldToken,
    ceiling: U256,
    spent: U256,
    now: u64,
}

/// The account and network a build was asked for: its reply is judged only for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Bound {
    account: Felt,
    network: NetworkId,
}

/// The reviewed choice's build in flight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingSubmit {
    bound: Bound,
    reviewed: GaslessFeeOffer,
    gas_token: Felt,
    reviewed_max: U256,
    ceiling: U256,
    now: u64,
}

/// A build the wallet checked and may sign, bound to its session.
#[derive(Debug, Clone, PartialEq)]
pub struct CheckedQuote<'p> {
    session: GaslessSession<'p>,
    parameters: Value,
    accepted: Accepted,
    user_fee: UserFee,
}

/// `refusalFor`.
fn refusal_for(reason: Refusal) -> GaslessCode {
    match reason {
        Refusal::Ceiling => GaslessCode::AboveCeiling,
        Refusal::Window => GaslessCode::InvalidScreeningTransaction,
        Refusal::Calls | Refusal::Forwarder | Refusal::Shape => GaslessCode::InvalidGaslessRequest,
    }
}

impl<'p> GaslessSession<'p> {
    fn bound(&self) -> Bound {
        Bound {
            account: self.account,
            network: self.network,
        }
    }

    /// Checks a build's result for `gas_token` against `ceiling` at `now`.
    fn check(
        &self,
        result: &JsValue,
        gas_token: Felt,
        ceiling: U256,
        now: u64,
    ) -> Result<Accepted, GaslessCode> {
        let (quoted_max, typed_data) = self
            .read_quote(result)
            .ok_or(GaslessCode::InvalidGaslessRequest)?;
        let input = TypedDataCheck {
            account: self.account,
            network: self.network,
            calls: &self.calls,
            forwarder: self.forwarder,
            gas_token,
            quoted_max,
            ceiling,
            now,
            minimum_remaining: self.policy.sign_window_seconds,
        };
        validate_typed_data(typed_data, &input, self.policy).map_err(refusal_for)
    }

    /// `offerForListed` up to its build: `None` (no offer for this token) when there is no
    /// reference or it gives no ceiling, or when the batch's spend of the token cannot be counted.
    pub fn offer_build(
        &self,
        held: &GaslessHeldToken,
        reference: Option<U256>,
        now: u64,
    ) -> Option<(PendingOffer, HttpRequest)> {
        let ceiling = reference.map_or(U256::ZERO, |reference| self.policy.fee_ceiling(reference));
        let spent = token_spend(&self.calls, &held.address)?;
        if ceiling == U256::ZERO {
            return None;
        }
        let request = self.build_request(&held.address, now).ok()?;
        let pending = PendingOffer {
            bound: self.bound(),
            held: held.clone(),
            ceiling,
            spent,
            now,
        };
        Some((pending, request))
    }

    /// `prepareSubmission` up to its build: the reviewed token is a felt (`Unavailable`) with an
    /// independent reference (`Unverifiable`).
    pub fn submit_build(
        &self,
        reviewed: &GaslessFeeOffer,
        reference: Option<U256>,
        now: u64,
    ) -> Result<(PendingSubmit, HttpRequest), GaslessError> {
        let gas_token = felt::parse_non_zero(&reviewed.token_address);
        let reviewed_max = reviewed.max_amount();
        let (Some(gas_token), Some(reviewed_max)) = (gas_token, reviewed_max) else {
            return Err(GaslessCode::Unavailable.into());
        };
        let reference = reference.ok_or(GaslessCode::Unverifiable)?;
        let request = self.build_request(&gas_token, now)?;
        let pending = PendingSubmit {
            bound: self.bound(),
            reviewed: reviewed.clone(),
            gas_token,
            reviewed_max,
            ceiling: self.policy.fee_ceiling(reference),
            now,
        };
        Ok((pending, request))
    }
}

impl PendingOffer {
    /// `heldOffer` after its build: a checked quote whose fee the token's balance covers beside
    /// the batch's own spend, as an offer the wallet could render. `None` otherwise, and for a
    /// session of another account or network than the build's.
    pub fn judge(
        self,
        session: &GaslessSession<'_>,
        result: &JsValue,
        strk: &Felt,
    ) -> Option<GaslessFeeOffer> {
        if session.bound() != self.bound {
            return None;
        }
        let accepted = session
            .check(result, self.held.address, self.ceiling, self.now)
            .ok()?;
        let affordable = token_affordability(accepted.fee_amount, self.held.balance, self.spent);
        let offer = GaslessFeeOffer {
            amount: accepted.fee_amount.to_decimal(),
            decimals: self.held.decimals,
            label: self.held.label,
            token_address: felt::canonical(&self.held.address),
        };
        (affordable == FeeAffordability::Sufficient && offer.is_valid(strk)).then_some(offer)
    }
}

impl PendingSubmit {
    /// `prepareSubmission` after its build: a session for another account or network, or an
    /// unreadable quote, is `invalid_gasless_request`, a refused body its `refusalFor` code, and
    /// a fee above the reviewed maximum `ReviewedFeeExceeded`.
    pub fn judge<'p>(
        self,
        session: &GaslessSession<'p>,
        result: &JsValue,
    ) -> Result<CheckedQuote<'p>, GaslessError> {
        if session.bound() != self.bound {
            return Err(GaslessCode::InvalidGaslessRequest.into());
        }
        let accepted = session.check(result, self.gas_token, self.ceiling, self.now)?;
        if accepted.fee_amount > self.reviewed_max {
            return Err(GaslessError::ReviewedFeeExceeded);
        }
        Ok(CheckedQuote {
            session: session.clone(),
            parameters: session.parameters(&self.gas_token, self.now)?,
            user_fee: UserFee {
                amount: accepted.fee_amount.to_decimal(),
                token: self.reviewed.label,
                token_address: self.reviewed.token_address,
            },
            accepted,
        })
    }
}

impl CheckedQuote<'_> {
    /// What the account signs: Krusty's SNIP-12 hash of the gateway's body for this account.
    pub fn message_hash(&self) -> Felt {
        self.accepted.message_hash
    }

    /// The gateway's body, unchanged: what the host persists (its nonce) before the execute.
    pub fn typed_data(&self) -> &JsValue {
        &self.accepted.typed_data
    }

    pub fn user_fee(&self) -> &UserFee {
        &self.user_fee
    }

    /// `paymaster_executeTransaction` with the account's `signature` over [`Self::message_hash`],
    /// once the window, read again at `now` after signing, still has the policy's floor left and
    /// is no longer than its cap (`invalid_screening_transaction` otherwise). The typed data is
    /// the gateway's body as it came; the parameters are the build's.
    pub fn execute_request(
        &self,
        signature: &[Felt],
        now: u64,
    ) -> Result<HttpRequest, GaslessError> {
        let session = &self.session;
        let live = session.policy.window_is_live(
            self.accepted.execute_after,
            self.accepted.execute_before,
            now,
            session.policy.sign_window_seconds,
        );
        if !live {
            return Err(GaslessCode::InvalidScreeningTransaction.into());
        }
        if signature.is_empty() {
            return Err(GaslessCode::Unavailable.into());
        }
        let signature: Vec<String> = signature.iter().map(felt::canonical).collect();
        let deployment = session
            .deployment
            .as_ref()
            .map(|deployment| format!(r#""deployment":{deployment},"#))
            .unwrap_or_default();
        let params = format!(
            r#"{{"parameters":{},"transaction":{{{deployment}"invoke":{{"signature":{},"typed_data":{},"user_address":{}}},"type":{}}}}}"#,
            self.parameters,
            json!(signature),
            self.accepted.typed_data.to_json(),
            json!(felt::canonical(&session.account)),
            json!(session.kind_wire()),
        );
        Ok(post(
            session.policy,
            session.url,
            "paymaster_executeTransaction",
            &params,
        ))
    }

    /// The execute's answer: a result without a usable hash is `Lost`, since the relay may
    /// already have accepted the execution.
    pub fn judge_execute(&self, answer: GaslessAnswer) -> Result<GaslessSubmission, GaslessError> {
        let result = answer.settle()?;
        let transaction_hash = read_transaction_hash(&result).ok_or(GaslessError::Lost)?;
        Ok(GaslessSubmission {
            transaction_hash,
            user_fee: self.user_fee.clone(),
        })
    }
}

#[cfg(test)]
#[path = "submit_tests.rs"]
mod tests;
