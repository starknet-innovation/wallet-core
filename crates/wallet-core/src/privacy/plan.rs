//! `PrivacyReadPlan`: one private-balance read as a core-owned state machine, in the extension's
//! `stateResponse` order (Bramble's `mobile/docs/privacy.md` §3): chain id, preflight, note
//! discovery when registered, pool fee. The host relays opaque bodies and replies; it cannot pick
//! a URL, reorder a step, or see the viewing key, which is asked of the wallet (`PrivacyHost`)
//! for every `Post` and never stored here.
//!
//! Reply handling for each read lives in `plan_steps`.

use std::collections::BTreeSet;

use super::bodies::BlockRef;
use super::config::{is_service_url, Limits, Route};
use super::json::Ordered;
use super::notes::Totals;
pub use super::plan_steps::requested_token_keys;
use super::types::{PrivacyError, PrivacyFailure, PrivacyReply, PrivacyStep};
use crate::catalog::{NetworkInfo, TokenInfo};
use crate::send::u256::U256;
use crate::{Felt, NetworkId, SecretFelt};

/// Bound on discovery pages (the SDK has none).
pub const MAX_DISCOVERY_PAGES: u32 = 256;

/// The wallet behind a read: the viewing key of the plan's account. Lent to every `advance`, so
/// a plan holds no reference to the wallet. The wallet keeps the session, the record lookup and
/// the lock check; the phrase never reaches this module.
pub trait PrivacyHost {
    /// The STRK20 viewing key of the plan's account scoped to `"<chain_id>:<pool_address>"`
    /// (`kms::derive_strk20_viewing_key`), derived now, under the session the plan was prepared
    /// in, without renewing it. `None` once that session has ended (a lock, even one followed by
    /// a new unlock) or the account is gone: the plan then fails `Locked`. The plan drops the key
    /// as soon as the one request body that carries it is built.
    fn viewing_key(&self, chain_id: &str, pool_address: &str) -> Option<SecretFelt>;
}

/// The read a reply or a pause belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Read {
    Chain,
    Preflight,
    Discovery,
    PoolFee,
}

#[derive(Debug)]
pub(crate) enum Stage {
    Fresh,
    Awaiting { id: u64, read: Read },
    Waiting(Read),
    Closed,
}

/// Discovery progress within one attempt; dropped when the attempt restarts.
#[derive(Debug, Default)]
pub(crate) struct Discovery {
    pub totals: Totals,
    pub cursor: Option<Ordered>,
    pub block: Option<BlockRef>,
    pub pages: u32,
}

/// One read. Closed on lock, backgrounding, or a scope change; a new read means a new plan.
pub struct PrivacyReadPlan {
    pub(crate) network: NetworkId,
    /// The catalog's chain id, half of the viewing key's scope.
    pub(crate) chain_id: String,
    /// The wallet's id for the account, echoed in the state; the key is the host's to find.
    pub(crate) account_id: String,
    pub(crate) address: Felt,
    pub(crate) route: Route,
    pub(crate) limits: Limits,
    pub(crate) tokens: Vec<TokenInfo>,
    /// Keys of `tokens`, checked once when the plan is prepared. `None` when that list is longer
    /// than the extension allows, or has a zero, unparseable, or repeated address: the read fails
    /// `NotConfigured` before any request.
    pub(crate) requested: Option<BTreeSet<U256>>,
    pub(crate) stage: Stage,
    pub(crate) next_id: u64,
    /// 1-based attempt of the current read.
    pub(crate) attempt: u32,
    /// Host time the current attempt began, for the discovery budget.
    pub(crate) attempt_started_ms: u64,
    pub(crate) registered: bool,
    pub(crate) discovery: Discovery,
}

impl PrivacyReadPlan {
    /// A read for the account at `address` (the wallet's `account_id`) on `network`, over its
    /// tokens in catalog order, through `route` (with any override the wallet allows already
    /// applied) and under `limits`. `NotConfigured` when the route has OHTTP on, is another
    /// network's, or names a discovery URL that is not an `https` service URL.
    pub fn new(
        network: &NetworkInfo,
        account_id: String,
        address: Felt,
        route: Route,
        limits: Limits,
    ) -> Result<Self, PrivacyError> {
        if route.ohttp || route.network != network.network || !is_service_url(&route.discovery_url)
        {
            return Err(PrivacyError::NotConfigured);
        }
        Ok(Self {
            network: network.network,
            chain_id: network.chain_id.clone(),
            account_id,
            address,
            requested: requested_token_keys(&network.tokens, &limits),
            tokens: network.tokens.clone(),
            route,
            limits,
            stage: Stage::Fresh,
            next_id: 0,
            attempt: 0,
            attempt_started_ms: 0,
            registered: false,
            discovery: Discovery::default(),
        })
    }

    /// Feeds the last reply and returns the next step. A reply whose id is not the one outstanding,
    /// or a reply kind the current step does not expect, returns `OutOfOrder` and closes the plan.
    /// `Finished` and `Failed` close it too.
    pub fn advance(
        &mut self,
        host: &dyn PrivacyHost,
        reply: PrivacyReply,
    ) -> Result<PrivacyStep, PrivacyError> {
        if matches!(self.stage, Stage::Closed) {
            return Err(PrivacyError::PlanClosed);
        }
        let result = self.step(host, reply);
        if !matches!(
            result,
            Ok(PrivacyStep::Post { .. } | PrivacyStep::Rpc { .. } | PrivacyStep::Wait { .. })
        ) {
            self.stage = Stage::Closed;
            self.discovery = Discovery::default();
        }
        result
    }

    /// Stops the plan and drops its cursor and totals. Idempotent.
    pub fn close(&mut self) {
        self.stage = Stage::Closed;
        self.discovery = Discovery::default();
    }

    fn step(
        &mut self,
        host: &dyn PrivacyHost,
        reply: PrivacyReply,
    ) -> Result<PrivacyStep, PrivacyError> {
        let stage = std::mem::replace(&mut self.stage, Stage::Closed);
        match (stage, reply) {
            (Stage::Fresh, PrivacyReply::Start { now_ms }) => {
                Ok(self.begin(host, Read::Chain, now_ms))
            }
            (
                Stage::Awaiting { id, read },
                PrivacyReply::Http {
                    id: got,
                    status,
                    body,
                    now_ms,
                },
            ) if id == got && matches!(read, Read::Preflight | Read::Discovery) => {
                Ok(self.on_http(host, read, status, &body, now_ms))
            }
            (
                Stage::Awaiting { id, read },
                PrivacyReply::Rpc {
                    id: got,
                    body,
                    now_ms,
                },
            ) if id == got && matches!(read, Read::Chain | Read::PoolFee) => {
                Ok(self.on_rpc(host, read, id, &body, now_ms))
            }
            (Stage::Awaiting { id, read }, PrivacyReply::TransportFailure { id: got, now_ms })
                if id == got =>
            {
                Ok(self.on_transport_failure(read, now_ms))
            }
            (Stage::Waiting(read), PrivacyReply::WaitElapsed { now_ms }) => {
                self.attempt += 1;
                Ok(self.start_attempt(host, read, now_ms))
            }
            _ => Err(PrivacyError::OutOfOrder),
        }
    }

    /// Starts a read at its first attempt. A tracked token list the extension would reject never
    /// starts: no chain probe, and no viewing key in a request body.
    pub(crate) fn begin(&mut self, host: &dyn PrivacyHost, read: Read, now_ms: u64) -> PrivacyStep {
        if self.requested.is_none() {
            return self.fail(PrivacyFailure::NotConfigured);
        }
        self.attempt = 1;
        self.start_attempt(host, read, now_ms)
    }

    /// Starts (or restarts) the current attempt of a read: discovery restarts from page one.
    fn start_attempt(&mut self, host: &dyn PrivacyHost, read: Read, now_ms: u64) -> PrivacyStep {
        self.attempt_started_ms = now_ms;
        match read {
            Read::Chain => self.rpc(Read::Chain, |_, id| crate::rpc::chain_id_request(id)),
            Read::Preflight => self.post_preflight(host),
            Read::Discovery => {
                self.discovery = Discovery::default();
                self.post_incoming(host)
            }
            Read::PoolFee => self.rpc(Read::PoolFee, |state, id| {
                super::bodies::pool_fee(id, &state.route.pool_address)
            }),
        }
    }

    pub(crate) fn next_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub(crate) fn rpc(
        &mut self,
        read: Read,
        body: impl FnOnce(&Self, u64) -> String,
    ) -> PrivacyStep {
        let id = self.next_id();
        let body = body(self, id);
        self.stage = Stage::Awaiting { id, read };
        PrivacyStep::Rpc {
            id,
            body,
            purpose: super::plan_steps::purpose(read),
        }
    }

    /// Pauses `attempt × 1 s` before the next attempt of `read`, or fails with `last` when the
    /// attempts are spent.
    pub(crate) fn retry_or(&mut self, read: Read, last: PrivacyFailure) -> PrivacyStep {
        if self.attempt >= self.limits.max_service_attempts {
            return self.fail(last);
        }
        self.stage = Stage::Waiting(read);
        PrivacyStep::Wait {
            delay_ms: self.attempt.saturating_mul(1_000),
        }
    }

    pub(crate) fn fail(&mut self, failure: PrivacyFailure) -> PrivacyStep {
        self.stage = Stage::Closed;
        PrivacyStep::Failed { failure }
    }
}
