//! The quote: `readSwapQuoteInput` and `swapPair`'s rules, `getQuote`'s request and `readQuote`'s
//! judge, and `walletSwapValueDelta` (the value check from the wallet's own price feed).

use super::{felt_value, hex_u256, read_json, u256_hex, SwapPolicy, SwapRefusal, SWAP_BODY_LIMIT};
use crate::catalog::TokenInfo;
use crate::felt;
use crate::js::json::JsValue;
use crate::js::num::{js_trim, string_to_number, utf16_len};
use crate::portfolio::prices::PriceView;
use crate::portfolio::wire::{HttpMethod, HttpOutcome, HttpRequest, HTTP_DEADLINE_MS};
use crate::send::calls::{format_token_amount, parse_token_amount, SendAmountError};
use crate::send::types::Account;
use crate::send::u256::U256;
use crate::{Felt, NetworkId};

/// One side of the pair: the tracked token as the wallet holds it, and its address as a felt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Side {
    pub(super) token: TokenInfo,
    pub(super) address: Felt,
}

impl Side {
    fn new(token: &TokenInfo) -> Result<Self, SwapRefusal> {
        let address = felt::parse_non_zero(&token.address).ok_or(SwapRefusal::TokenUnavailable)?;
        Ok(Self {
            token: token.clone(),
            address,
        })
    }
}

/// A quote request in flight, bound to the account, network, pair and amount it asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingQuote {
    pub(super) network: NetworkId,
    pub(super) taker: Felt,
    pub(super) sell: Side,
    pub(super) buy: Side,
    pub(super) sell_amount: U256,
    /// The amount as typed (trimmed), for the review, the value check and the activity row.
    pub(super) sell_text: String,
    pub(super) policy: SwapPolicy,
}

/// A judged quote, still bound to its request: only `SwapQuote::prepare` builds against it.
#[derive(Debug, Clone, PartialEq)]
pub struct SwapQuote {
    pub(super) request: PendingQuote,
    pub(super) quote_id: String,
    pub(super) buy_amount: U256,
    /// The aggregator's own figures, for display only (`walletSwapValueDelta` is the check).
    pub sell_amount_in_usd: f64,
    pub buy_amount_in_usd: f64,
    pub gas_fees_in_usd: f64,
}

impl PendingQuote {
    /// The quote request for selling `amount` (decimal text as typed) of `sell` for `buy` from
    /// `account`. Both tokens are the wallet's tracked tokens on the account's network, resolved
    /// by the host (the extension resolves them by id from its registry, never from the panel).
    pub fn new(
        account: &Account,
        sell: &TokenInfo,
        buy: &TokenInfo,
        amount: &str,
        policy: &SwapPolicy,
    ) -> Result<(Self, HttpRequest), SwapRefusal> {
        let (sell, buy) = (Side::new(sell)?, Side::new(buy)?);
        if sell.address == buy.address {
            return Err(SwapRefusal::SameToken);
        }
        let sell_amount =
            parse_token_amount(amount, sell.token.decimals).map_err(|error| match error {
                SendAmountError::Invalid => SwapRefusal::InvalidQuoteRequest,
                _ => SwapRefusal::NoAmount,
            })?;
        let base = policy.deployment(account.network)?.base_url;
        // `URLSearchParams` in insertion order; every value is a validated felt or `0x` hex, so
        // none needs escaping.
        let url = format!(
            "{base}/swap/v3/quotes?buyTokenAddress={}&sellAmount={}&sellTokenAddress={}&size=1&takerAddress={}",
            buy.token.address,
            u256_hex(sell_amount),
            sell.token.address,
            felt::canonical(&account.address),
        );
        let request = HttpRequest {
            method: HttpMethod::Get,
            url,
            headers: Vec::new(),
            body: None,
            max_body_bytes: SWAP_BODY_LIMIT,
            deadline_ms: HTTP_DEADLINE_MS,
        };
        let pending = Self {
            network: account.network,
            taker: account.address,
            sell,
            buy,
            sell_amount,
            sell_text: js_trim(amount).to_string(),
            policy: *policy,
        };
        Ok((pending, request))
    }

    /// `getQuote`'s judgement: the best (first) quote, every field in its grammar, for exactly
    /// the pair and amount requested.
    pub fn judge(self, outcome: &HttpOutcome) -> Result<SwapQuote, SwapRefusal> {
        let payload = read_json(outcome, SwapRefusal::QuoteFailed)?;
        let best = payload
            .as_array()
            .and_then(<[JsValue]>::first)
            .ok_or(SwapRefusal::NoRoute)?;
        self.read_quote(best)
    }

    /// `readQuote`.
    fn read_quote(self, value: &JsValue) -> Result<SwapQuote, SwapRefusal> {
        let max_id = self.policy.max_quote_id_length;
        let quote_id = value
            .get("quoteId")
            .and_then(JsValue::as_str)
            .filter(|id| !id.is_empty() && utf16_len(id) <= max_id);
        let amount = |key| value.get(key).and_then(JsValue::as_str).and_then(hex_u256);
        let usd = |key| {
            value
                .get(key)
                .and_then(JsValue::as_f64)
                .filter(|figure| figure.is_finite() && *figure >= 0.0)
        };
        let (
            Some(quote_id),
            Some(sell_token),
            Some(buy_token),
            Some(sell_amount),
            Some(buy_amount),
            Some(sell_amount_in_usd),
            Some(buy_amount_in_usd),
            Some(gas_fees_in_usd),
        ) = (
            quote_id,
            felt_value(value.get("sellTokenAddress")),
            felt_value(value.get("buyTokenAddress")),
            amount("sellAmount"),
            amount("buyAmount"),
            usd("sellAmountInUsd"),
            usd("buyAmountInUsd"),
            usd("gasFeesInUsd"),
        )
        else {
            return Err(SwapRefusal::InvalidQuote);
        };
        if sell_token != self.sell.address
            || buy_token != self.buy.address
            || sell_amount != self.sell_amount
        {
            return Err(SwapRefusal::QuoteMismatch);
        }
        Ok(SwapQuote {
            quote_id: quote_id.to_string(),
            request: self,
            buy_amount,
            sell_amount_in_usd,
            buy_amount_in_usd,
            gas_fees_in_usd,
        })
    }
}

impl SwapQuote {
    pub fn quote_id(&self) -> &str {
        &self.quote_id
    }

    /// The amount sold, as typed.
    pub fn sell_amount_text(&self) -> &str {
        &self.request.sell_text
    }

    /// The quoted buy amount in the buy token's decimals (`formatTokenAmount`).
    pub fn buy_amount_text(&self) -> String {
        format_token_amount(&self.buy_amount, self.request.buy.token.decimals)
    }

    /// `walletSwapValueDelta`: the percent by which the buy side is worth more (+) or less (-)
    /// than the sell side, priced from the wallet's own feed (`prices`, looked up by each token's
    /// stored address), so an aggregator cannot hide a loss behind its own USD figures. `None`
    /// without a feed, from a stale one, or without a positive finite price for both tokens.
    ///
    /// This is the only check on the price that does not come from AVNU: the build's floor is
    /// derived from AVNU's own quoted buy amount. So `None` means the swap's value is AVNU's word
    /// alone, and the host must show it as unverified, never as a fair price.
    pub fn value_delta_percent(&self, prices: Option<&PriceView>) -> Option<f64> {
        let view = prices.filter(|view| !view.stale)?;
        let price = |side: &Side| {
            view.price(&side.token.address)
                .filter(|price| price.is_finite() && *price > 0.0)
        };
        let sell_price = price(&self.request.sell)?;
        let buy_price = price(&self.request.buy)?;
        // The extension also returns `undefined` when an amount is not a positive finite
        // `Number`; here both are decimal u256 text and the sold amount passed
        // `parse_token_amount`, so neither case can arise.
        let sell_units = string_to_number(&self.request.sell_text);
        let buy_units = string_to_number(&self.buy_amount_text());
        let sell_value = sell_units * sell_price;
        Some((buy_units * buy_price - sell_value) / sell_value * 100.0)
    }
}

#[cfg(test)]
#[path = "quote_tests.rs"]
mod tests;
