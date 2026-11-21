//! External allowance discovery (`allowance-scan.ts collectExternalAllowances`) as a sans-IO
//! step machine, one call at a time as the extension awaits them. For each tracked token:
//! the owner's `Approval` events by key (Cairo 1 layout), up to `owner_event_pages` pages; then
//! `starknet_blockNumber` and the unfiltered log walked back from the head one block window at a
//! time (legacy layout, whose owner no key filter reaches), up to `legacy_windows` windows of
//! `log_event_pages` pages, then the rest of the log from block 0; then one
//! `allowance(owner, spender)` read per distinct spender, at most `max_spenders_per_token`.
//!
//! A node's JSON-RPC error anywhere in a token's reads skips that token and marks the result
//! incomplete; a transport failure is `ScanStep::Transport`, and the host replays the whole scan
//! on the next endpoint (`restart`), as `withRpcFallback` reruns it. With every endpoint failed
//! the host lists `ExternalAllowanceScanResult::unavailable()`, as the extension's caller does.

use std::collections::HashSet;

use serde_json::{json, Value};

use super::discovery::{
    decode_approval_event, parse_allowance_amount, ExternalAllowance, APPROVAL_EVENT_KEY,
};
use super::tracked::allowance_call;
use super::AllowancePolicy;
pub use crate::history::events::ScanStep;
use crate::history::events::MAX_CONTINUATION_BYTES;
use crate::js::felt::{canonical_felt, is_non_zero_felt};
use crate::js::json::JsValue;
use crate::js::num::is_safe_integer;
use crate::portfolio::wire::{classify, rpc_call, Reply, RpcReply, RpcRequest};
use crate::send::u256::U256;
use crate::Error;

/// `ExternalAllowanceScanResult`. `complete` is false when any token's log was not read to its
/// end, held legacy approvals beyond what was read, named more spenders than are read, or could
/// not be read: an empty list then means "none found in what was read", not "none".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalAllowanceScanResult {
    pub allowances: Vec<ExternalAllowance>,
    pub complete: bool,
}

impl ExternalAllowanceScanResult {
    /// What the listing shows when discovery could not run on any endpoint.
    pub fn unavailable() -> Self {
        Self {
            allowances: Vec::new(),
            complete: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Read {
    Owner,
    Window { window: usize, from: u64, to: u64 },
    Rest { to: u64 },
}

#[derive(Debug, Clone)]
enum Phase {
    Events {
        read: Read,
        page: usize,
        continuation: Option<JsValue>,
        cairo1: bool,
        legacy: bool,
    },
    Latest,
    Reads {
        spenders: Vec<String>,
        next: usize,
    },
    Done,
}

#[derive(Debug, Clone)]
struct State {
    token: usize,
    phase: Phase,
    next_id: u64,
    outstanding: Option<u64>,
    owned: Vec<String>,
    owned_truncated: bool,
    log: Vec<String>,
    /// The current token's discovery completeness, settled before its reads.
    token_complete: bool,
    token_allowances: Vec<ExternalAllowance>,
    allowances: Vec<ExternalAllowance>,
    complete: bool,
}

/// One owner's external allowance scan on one network. A wallet that shares one across threads
/// wraps it in its own lock.
#[derive(Debug)]
pub struct ExternalAllowanceScan {
    owner: String,
    tokens: Vec<String>,
    policy: AllowancePolicy,
    state: State,
}

fn owner_phase() -> Phase {
    Phase::Events {
        read: Read::Owner,
        page: 0,
        continuation: None,
        cairo1: false,
        legacy: false,
    }
}

// Each value as `canonicalFelt` spells it, when that is a non-zero felt.
fn canonical_non_zero(value: &str) -> Option<String> {
    canonical_felt(value).filter(|felt| is_non_zero_felt(felt))
}

impl ExternalAllowanceScan {
    /// A scan of `owner`'s approvals on `tokens` (each kept only when a non-zero felt). An owner
    /// that is not one, or no token, finishes at once: empty and complete.
    pub fn new(owner: &str, tokens: &[String], policy: AllowancePolicy) -> Self {
        let owner = canonical_non_zero(owner);
        let tokens = match owner {
            Some(_) => tokens
                .iter()
                .filter_map(|t| canonical_non_zero(t))
                .collect(),
            None => Vec::new(),
        };
        let mut this = Self {
            owner: owner.unwrap_or_default(),
            tokens,
            policy,
            state: Self::fresh(),
        };
        this.restart();
        this
    }

    fn fresh() -> State {
        State {
            token: 0,
            phase: owner_phase(),
            next_id: 1,
            outstanding: None,
            owned: Vec::new(),
            owned_truncated: false,
            log: Vec::new(),
            token_complete: true,
            token_allowances: Vec::new(),
            allowances: Vec::new(),
            complete: true,
        }
    }

    /// Discards all progress so the scan replays from its first call on another endpoint.
    pub fn restart(&mut self) {
        self.state = Self::fresh();
        if self.tokens.is_empty() {
            self.state.phase = Phase::Done;
        }
    }

    /// The call to send now (at most one); empty while waiting for a reply or when done.
    pub fn next_calls(&mut self) -> Vec<RpcRequest> {
        if self.state.outstanding.is_some() {
            return Vec::new();
        }
        let id = self.state.next_id;
        let call = match &self.state.phase {
            Phase::Events {
                read, continuation, ..
            } => self.events_call(id, *read, continuation.as_ref()),
            Phase::Latest => Some(rpc_call(id, "starknet_blockNumber", json!([]))),
            Phase::Reads { spenders, next } => {
                let token = &self.tokens[self.state.token];
                Some(allowance_call(id, token, &self.owner, &spenders[*next]))
            }
            Phase::Done => None,
        };
        let Some(call) = call else {
            return Vec::new();
        };
        self.state.next_id += 1;
        self.state.outstanding = Some(id);
        vec![call]
    }

    fn events_call(
        &self,
        id: u64,
        read: Read,
        continuation: Option<&JsValue>,
    ) -> Option<RpcRequest> {
        let mut filter = json!({
            "address": self.tokens[self.state.token],
            "chunk_size": self.policy.event_chunk_size,
        });
        let range = match read {
            Read::Owner => None,
            Read::Window { from, to, .. } => Some((from, to)),
            Read::Rest { to } => Some((0, to)),
        };
        match range {
            // The owner read asks for this owner's approvals only, over the whole log.
            None => filter["keys"] = json!([[APPROVAL_EVENT_KEY], [self.owner]]),
            Some((from, to)) => {
                filter["keys"] = json!([[APPROVAL_EVENT_KEY]]);
                filter["from_block"] = json!({ "block_number": from });
                filter["to_block"] = json!({ "block_number": to });
            }
        }
        if let Some(token) = continuation {
            let value: Value = serde_json::from_str(&token.to_json()).ok()?;
            filter["continuation_token"] = value;
        }
        Some(rpc_call(
            id,
            "starknet_getEvents",
            json!({ "filter": filter }),
        ))
    }

    /// Judges one reply: a reply for another call, or after the end, is a transport failure.
    pub fn accept(&mut self, reply: RpcReply) -> ScanStep {
        let Some(id) = self.state.outstanding.filter(|id| *id == reply.id) else {
            return ScanStep::Transport;
        };
        self.state.outstanding = None;
        let result = match classify(&reply, id) {
            Reply::Transport => return ScanStep::Transport,
            Reply::Semantic => {
                self.skip_token();
                return ScanStep::Continue;
            }
            Reply::Result(result) => result,
        };
        let judged = match self.state.phase.clone() {
            Phase::Events {
                read,
                page,
                cairo1,
                legacy,
                ..
            } => self.accept_page(read, page, cairo1, legacy, &result),
            Phase::Latest => match result {
                JsValue::Number(n) if is_safe_integer(n) && n >= 0.0 => {
                    self.next_window(0, n as u64);
                    Ok(())
                }
                _ => Err(()),
            },
            Phase::Reads { spenders, next } => self.accept_read(&spenders, next, &result),
            Phase::Done => Err(()),
        };
        match judged {
            Ok(()) => ScanStep::Continue,
            Err(()) => ScanStep::Transport,
        }
    }

    // One events page; `Err(())` is a reply the extension would throw a non-RPC error on.
    fn accept_page(
        &mut self,
        read: Read,
        page: usize,
        cairo1: bool,
        legacy: bool,
        result: &JsValue,
    ) -> Result<(), ()> {
        if matches!(result, JsValue::Null) {
            return Err(());
        }
        let events: &[JsValue] = match result.get("events") {
            None | Some(JsValue::Null) => &[],
            Some(JsValue::Array(events)) => events,
            Some(_) => return Err(()),
        };
        let (mut cairo1, mut legacy) = (cairo1, legacy);
        let mut spenders = Vec::new();
        for event in events {
            let keys = event.get("keys").and_then(JsValue::as_array).ok_or(())?;
            let data = event.get("data").and_then(JsValue::as_array).ok_or(())?;
            cairo1 |= keys.len() >= 3;
            legacy |= keys.len() < 3;
            if let Some((owner, spender)) = decode_approval_event(keys, data) {
                // Both canonical (`decode_approval_event`, `new`).
                if owner == self.owner {
                    spenders.push(spender);
                }
            }
        }
        match read {
            Read::Owner => self.state.owned.extend(spenders),
            _ => self.state.log.extend(spenders),
        }
        let page = page + 1;
        let budget = match read {
            Read::Owner => self.policy.owner_event_pages,
            _ => self.policy.log_event_pages,
        };
        match result.get("continuation_token") {
            Some(token) if page < budget => {
                if token.to_json().len() > MAX_CONTINUATION_BYTES {
                    return Err(());
                }
                self.state.phase = Phase::Events {
                    read,
                    page,
                    continuation: Some(token.clone()),
                    cairo1,
                    legacy,
                };
                Ok(())
            }
            next => {
                self.finish_read(read, next.is_some(), cairo1, legacy);
                Ok(())
            }
        }
    }

    // `pageSpenders` ended: by its log's end, or with pages left unread (`truncated`).
    fn finish_read(&mut self, read: Read, truncated: bool, cairo1: bool, legacy: bool) {
        match read {
            Read::Owner => {
                self.state.owned_truncated = truncated;
                self.state.phase = Phase::Latest;
            }
            // `legacySpenders`: a window of Cairo 1 events only shows the token emits the layout
            // the owner read covers; a truncated one left legacy events unread; block 0 is the end.
            Read::Window { window, from, .. } => {
                if cairo1 && !legacy {
                    self.start_reads(true);
                } else if truncated {
                    self.start_reads(false);
                } else if from == 0 {
                    self.start_reads(true);
                } else {
                    self.next_window(window + 1, from - 1);
                }
            }
            Read::Rest { .. } => self.start_reads(!truncated || !legacy),
        }
    }

    // The window ending at `to`, or past the last window the rest of the log from block 0.
    fn next_window(&mut self, window: usize, to: u64) {
        let read = if window < self.policy.legacy_windows {
            let from = (to + 1).saturating_sub(self.policy.legacy_block_window);
            Read::Window { window, from, to }
        } else {
            Read::Rest { to }
        };
        self.state.phase = Phase::Events {
            read,
            page: 0,
            continuation: None,
            cairo1: false,
            legacy: false,
        };
    }

    // `discoverSpenders`: the distinct spenders of both reads, at most the cap.
    fn start_reads(&mut self, log_complete: bool) {
        // A node that ignores `chunk_size` can send a page of hundreds of thousands: linear, and
        // one past the cap is enough to know the cap was passed.
        let cap = self.policy.max_spenders_per_token;
        let mut seen = HashSet::new();
        let mut spenders: Vec<String> = Vec::new();
        for spender in self.state.owned.iter().chain(&self.state.log) {
            if spenders.len() > cap {
                break;
            }
            if seen.insert(spender) {
                spenders.push(spender.clone());
            }
        }
        self.state.token_complete =
            !self.state.owned_truncated && log_complete && spenders.len() <= cap;
        spenders.truncate(cap);
        if spenders.is_empty() {
            self.finish_token();
        } else {
            self.state.phase = Phase::Reads { spenders, next: 0 };
        }
    }

    fn accept_read(
        &mut self,
        spenders: &[String],
        next: usize,
        result: &JsValue,
    ) -> Result<(), ()> {
        let amount = match result {
            JsValue::Null => return Err(()),
            JsValue::Array(items) => parse_allowance_amount(items),
            _ => None,
        };
        if let Some(amount) = amount.filter(|a| *a != U256::ZERO) {
            self.state.token_allowances.push(ExternalAllowance {
                token: self.tokens[self.state.token].clone(),
                spender: spenders[next].clone(),
                amount,
            });
        }
        if next + 1 < spenders.len() {
            self.state.phase = Phase::Reads {
                spenders: spenders.to_vec(),
                next: next + 1,
            };
        } else {
            self.finish_token();
        }
        Ok(())
    }

    // `collectTokenAllowancesSafe`: a semantic failure drops the token, incomplete.
    fn skip_token(&mut self) {
        self.state.token_allowances.clear();
        self.state.token_complete = false;
        self.finish_token();
    }

    fn finish_token(&mut self) {
        let found = std::mem::take(&mut self.state.token_allowances);
        self.state.allowances.extend(found);
        self.state.complete &= self.state.token_complete;
        self.state.owned.clear();
        self.state.owned_truncated = false;
        self.state.log.clear();
        self.state.token_complete = true;
        self.state.token += 1;
        self.state.phase = if self.state.token < self.tokens.len() {
            owner_phase()
        } else {
            Phase::Done
        };
    }

    /// The result, once every call has been answered.
    pub fn finish(&self) -> Result<ExternalAllowanceScanResult, Error> {
        match self.state.phase {
            Phase::Done => Ok(ExternalAllowanceScanResult {
                allowances: self.state.allowances.clone(),
                complete: self.state.complete,
            }),
            _ => Err(Error::rpc("allowance scan is not finished")),
        }
    }
}
