//! The offer round (`quoteGaslessOffers` after its gates, `collectOffers`, `offerForListed`),
//! sans-IO: `paymaster_isAvailable`, then `paymaster_getSupportedTokens`, then one build per
//! listed token the wallet holds, in the paymaster's order, until the policy's `max_offers`. The
//! host posts each [`RoundStep::Post`] (after its delay) and hands back what came back. A token
//! with no reference, no countable spend or a quote the wallet would not sign is skipped; a
//! failed exchange ends the round with no offers at all (the extension's `catch`), since a
//! paymaster that refuses one build is not one to offer.

use super::request::{read_available, read_supported_tokens, GaslessSession, ListedToken};
use super::rpc::{Exchange, Exchanged};
use super::submit::{held_for, PendingOffer};
use super::{GaslessFeeOffer, GaslessHeldToken};
use crate::portfolio::wire::{HttpOutcome, HttpRequest};
use crate::send::u256::U256;
use crate::Felt;

/// What the host does next.
#[derive(Debug, Clone, PartialEq)]
pub enum RoundStep {
    /// Wait `delay_ms`, then post `request` and pass its outcome to [`OfferRound::accept`].
    Post { request: HttpRequest, delay_ms: u64 },
    /// The round is over: the offers a review may show, possibly none.
    Done(Vec<GaslessFeeOffer>),
}

enum Stage {
    Available(Exchange),
    Tokens(Exchange),
    Building {
        listed: Vec<ListedToken>,
        next: usize,
        pending: Box<PendingOffer>,
        exchange: Exchange,
    },
    Done,
}

/// One review's offer round for a session whose gates passed.
pub struct OfferRound<'p> {
    session: GaslessSession<'p>,
    /// The wallet tokens that may be offered, each with its independent reference
    /// ([`super::fee_reference`]; `None` hides the token).
    held: Vec<(GaslessHeldToken, Option<U256>)>,
    strk: Felt,
    now: u64,
    stage: Stage,
    offers: Vec<GaslessFeeOffer>,
}

impl<'p> OfferRound<'p> {
    /// Starts the round at `now` (Unix seconds, one clock for every build): its first step is
    /// `paymaster_isAvailable`.
    pub fn start(
        session: GaslessSession<'p>,
        held: Vec<(GaslessHeldToken, Option<U256>)>,
        strk: Felt,
        now: u64,
    ) -> (Self, RoundStep) {
        let exchange = Exchange::new(session.available_request(), false);
        let step = post(&exchange, 0);
        let round = Self {
            session,
            held,
            strk,
            now,
            stage: Stage::Available(exchange),
            offers: Vec::new(),
        };
        (round, step)
    }

    /// Judges the outcome of the last `Post`. After `Done`, it answers `Done` again.
    pub fn accept(&mut self, outcome: &HttpOutcome) -> RoundStep {
        let policy = self.session.policy;
        let (exchange, settled) = match &mut self.stage {
            Stage::Available(exchange) | Stage::Tokens(exchange) => {
                let judged = exchange.judge(outcome, policy);
                (exchange.clone(), judged)
            }
            Stage::Building { exchange, .. } => {
                let judged = exchange.judge(outcome, policy);
                (exchange.clone(), judged)
            }
            Stage::Done => return RoundStep::Done(self.offers.clone()),
        };
        let result = match settled {
            Exchanged::Repeat { delay_ms } => return post(&exchange, delay_ms),
            Exchanged::Settled(Ok(result)) => result,
            Exchanged::Settled(Err(_)) => {
                self.offers.clear();
                return self.finish();
            }
        };
        match std::mem::replace(&mut self.stage, Stage::Done) {
            Stage::Available(_) if read_available(&result) => {
                let exchange = Exchange::new(self.session.supported_tokens_request(), false);
                let step = post(&exchange, 0);
                self.stage = Stage::Tokens(exchange);
                step
            }
            Stage::Tokens(_) => match read_supported_tokens(&result) {
                Some(listed) => self.build_from(listed, 0),
                None => self.finish(),
            },
            Stage::Building {
                listed,
                next,
                pending,
                ..
            } => {
                if let Some(offer) = pending.judge(&self.session, &result, &self.strk) {
                    self.offers.push(offer);
                }
                self.build_from(listed, next)
            }
            Stage::Available(_) | Stage::Done => self.finish(),
        }
    }

    /// `offerForListed` from `listed[from]` on: the next token worth a build, or the end.
    fn build_from(&mut self, listed: Vec<ListedToken>, from: usize) -> RoundStep {
        let held: Vec<GaslessHeldToken> = self.held.iter().map(|(t, _)| t.clone()).collect();
        for (at, token) in listed.iter().enumerate().skip(from) {
            if self.offers.len() >= self.session.policy.max_offers {
                break;
            }
            let Some(held) = held_for(token, &held, &self.strk) else {
                continue;
            };
            let reference = self
                .held
                .iter()
                .find(|(candidate, _)| candidate == held)
                .and_then(|(_, reference)| *reference);
            let Some((pending, request)) = self.session.offer_build(held, reference, self.now)
            else {
                continue;
            };
            let exchange = Exchange::new(request, false);
            let step = post(&exchange, 0);
            self.stage = Stage::Building {
                next: at + 1,
                listed,
                pending: Box::new(pending),
                exchange,
            };
            return step;
        }
        self.finish()
    }

    fn finish(&mut self) -> RoundStep {
        self.stage = Stage::Done;
        RoundStep::Done(self.offers.clone())
    }
}

fn post(exchange: &Exchange, delay_ms: u64) -> RoundStep {
    RoundStep::Post {
        request: exchange.request().clone(),
        delay_ms,
    }
}

#[cfg(test)]
#[path = "round_tests.rs"]
mod tests;
