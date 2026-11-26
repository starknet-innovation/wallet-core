//! `ProofJob`: one STRK20 proof as a core-owned state machine (#182, S5), in the SDK's order
//! (`createProofInvocation` then `executeWithInvocation`, on Bramble's one option path):
//!
//! 1. the `outgoing_state` walk (`outgoing`), for the recipients `compiler::walk_inputs` names;
//! 2. the `incoming_state` walk (`incoming`), only when a token is in deficit;
//! 3. compile (`compiler::compile`), refuse `USER_LINKAGE`, build the `compile_actions` calldata
//!    and bind it to the reviewed actions (`pool_actions`), all before any nonce or signature;
//! 4. the pool's nonce at `latest` (`invocation::pool_nonce_request`);
//! 5. the `CallSet` digest over the bound calldata (`call_set`), signed by the wallet and checked
//!    under the account's public key, and the `ProofInvocation` built from it;
//! 6. `starknet_proveTransaction` (`prover`), retried only while the prover is busy.
//!
//! An HTTP 409 from either walk (the SDK's `ReorgError`) restarts the job from step 1 once, as the
//! SDK's `compile` does; a second one fails. Like `PrivacyReadPlan`, the host relays opaque bodies
//! and replies, never picks a URL, and sees the viewing key only inside the zeroizing `Post`
//! bodies. Reply handling is `job_steps`.
//!
//! Deliberate difference from the SDK, which signs first and leaves the extension to refuse the
//! warning after: `USER_LINKAGE` is refused at compile, so no nonce is read and nothing signed.

use std::fmt;

use zeroize::Zeroizing;

use super::compiler::{walk_inputs, Contracts, WalkInputs};
use super::config::{is_service_url, Route};
use super::entropy::{Entropy, OsEntropy};
use super::incoming::IncomingWalk;
use super::invocation::ProofInvocation;
use super::outgoing::{OutgoingChannels, OutgoingWalk};
use super::plan::PrivacyHost;
use super::proof::{CallAndProof, ProofLimits, ProofRefusal, ReviewedAction};
use super::prover::ProverRetry;
use super::types::{PrivacyError, PrivacyReply};
use crate::catalog::NetworkInfo;
use crate::send::types::SignRefusal;
use crate::{felt, Felt, SecretFelt, StarkSignature};

/// The wallet behind a proof job: the viewing key (`PrivacyHost::viewing_key`, asked for each
/// discovery body and once to compile) and the account's signature over the `CallSet` digest.
pub trait ProofHost: PrivacyHost {
    /// Signs `hash` with the job's account key under the session the job was approved in (as
    /// `send::plan::PlanHost::sign`), `Locked` once that session has ended. Asked at most once
    /// per job, with the digest the core computed over its own bound calldata.
    fn sign(&self, hash: &Felt) -> Result<StarkSignature, SignRefusal>;
}

/// What a proof job needs. Built by the wallet; nothing here is Bramble's unless it passes it.
/// Its `Debug` prints how many actions were reviewed, never their amounts or recipients.
#[derive(Clone, PartialEq, Eq)]
pub struct ProofInputs {
    pub account: Felt,
    /// The account's signer public key; the `CallSet` signature must verify under it.
    pub public_key: Felt,
    pub actions: Vec<ReviewedAction>,
    /// The block the walks read at (as `BlockRef::Number`) and the prover proves at, picked by
    /// the wallet before the job starts (the extension's maturity rule).
    pub proving_block: u64,
    /// Pool and discovery URL; refused unless plain (no OHTTP), this network's, and a service URL.
    pub route: Route,
    /// The prover's URL, checked with `config::is_service_url`. Its own input until the wallet's
    /// routing catalog carries one.
    pub prover_url: String,
    /// The network's shadow-account anonymizer, which a `ShadowInvoke` runs through; `None` when
    /// the network has none, and a job with a `ShadowInvoke` is then refused before any request.
    /// Its own input until the wallet's routing catalog carries one.
    pub anonymizer: Option<Felt>,
    /// Clamped to `ProofLimits::CEILING`.
    pub limits: ProofLimits,
    /// The most nonzero notes the incoming walk keeps per token.
    pub max_notes: usize,
    /// Clamped to `ProverRetry::CEILING`.
    pub retry: ProverRetry,
}

impl fmt::Debug for ProofInputs {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProofInputs")
            .field("account", &self.account)
            .field("public_key", &self.public_key)
            .field(
                "actions",
                &format_args!("[redacted {} actions]", self.actions.len()),
            )
            .field("proving_block", &self.proving_block)
            .field("route", &self.route)
            .field("prover_url", &self.prover_url)
            .field("anonymizer", &self.anonymizer)
            .field("limits", &self.limits)
            .field("max_notes", &self.max_notes)
            .field("retry", &self.retry)
            .finish()
    }
}

/// What a step is for; progress copy only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofPurpose {
    OutgoingState,
    IncomingState,
    PoolNonce,
    Prove,
}

/// The next thing the host must do for a proof job.
#[derive(Clone, PartialEq, Eq)]
pub enum ProofStep {
    /// POST `body` to `url` with `Content-Type: application/json`; no redirects, no cache, read at
    /// most `MAX_RESPONSE_BYTES` (`prover::MAX_PROVE_RESPONSE_BYTES` for `Prove`); reply `Http`
    /// (same id) or `TransportFailure`. A `Prove` attempt times out after
    /// `ProverRetry::request_timeout_ms`, reported as `TransportFailure`.
    /// `body` CONTAINS THE VIEWING KEY: never log, persist, display or inspect it. It is wiped
    /// when the step is dropped; a wallet that must copy it out (across FFI) makes that one copy.
    Post {
        id: u64,
        url: String,
        body: Zeroizing<Vec<u8>>,
        purpose: ProofPurpose,
    },
    /// JSON-RPC to the network's chain-proven endpoint (public data only); reply `Rpc` or
    /// `TransportFailure`.
    Rpc {
        id: u64,
        body: String,
        purpose: ProofPurpose,
    },
    /// Sleep, then reply `WaitElapsed`.
    Wait { delay_ms: u64 },
    /// Terminal. The job is closed.
    Finished { proof: CallAndProof },
    /// Terminal. The job is closed.
    Failed { refusal: ProofRefusal },
}

impl fmt::Debug for ProofStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Post {
                id,
                url,
                body,
                purpose,
            } => f
                .debug_struct("Post")
                .field("id", id)
                .field("url", url)
                .field("body", &format_args!("[redacted {} bytes]", body.len()))
                .field("purpose", purpose)
                .finish(),
            Self::Rpc { id, body, purpose } => f
                .debug_struct("Rpc")
                .field("id", id)
                .field("body", body)
                .field("purpose", purpose)
                .finish(),
            Self::Wait { delay_ms } => f.debug_struct("Wait").field("delay_ms", delay_ms).finish(),
            Self::Finished { proof } => f.debug_struct("Finished").field("proof", proof).finish(),
            Self::Failed { refusal } => f.debug_struct("Failed").field("refusal", refusal).finish(),
        }
    }
}

/// Where the job is. Every stage past `Fresh` holds what the next reply needs and nothing more;
/// the secrets in it (channel keys, the calldata, the invocation) are zeroed when it is replaced.
pub(super) enum Stage {
    Fresh,
    /// Step 1: an outgoing page is out.
    Outgoing {
        id: u64,
        walk: OutgoingWalk,
    },
    /// Step 2: an incoming page is out; the channels step 1 found wait for the compiler.
    Incoming {
        id: u64,
        walk: IncomingWalk,
        channels: OutgoingChannels,
    },
    /// Step 4: the nonce read is out; the bound `compile_actions` calldata waits to be signed.
    Nonce {
        id: u64,
        calldata: Vec<SecretFelt>,
    },
    /// Step 6: a prove attempt (from 0) is out under JSON-RPC id `rpc_id`.
    Proving {
        id: u64,
        rpc_id: u64,
        attempt: u32,
        invocation: ProofInvocation,
    },
    /// The prover was busy; `attempt` is the next one.
    Waiting {
        attempt: u32,
        invocation: ProofInvocation,
    },
    Closed,
}

/// One proof. Closed on lock, backgrounding, or a scope change; a new proof means a new job.
/// `E` is the randomness the compiler draws: the OS's ([`ProofJob::new`]), or a replay's in tests
/// ([`ProofJob::with_entropy`]).
pub struct ProofJob<E: Entropy = OsEntropy> {
    pub(super) inputs: ProofInputs,
    /// The `CallSet`'s chain, `NetworkId::chain_id` of the catalog's network.
    pub(super) chain_id: Felt,
    /// The catalog's chain-id spelling, half of the viewing key's scope, exactly as
    /// `PrivacyReadPlan` asks with it, so both derive the same key.
    pub(super) scope_chain_id: String,
    pub(super) entropy: E,
    /// What the walks fetch, from `walk_inputs` at `Start`.
    pub(super) walks: WalkInputs,
    /// Whether the one reorg restart has been used.
    pub(super) restarted: bool,
    pub(super) stage: Stage,
    pub(super) next_id: u64,
}

impl ProofJob<OsEntropy> {
    /// A job on `network` (the wallet's catalog record) for `inputs`, drawing its randomness from
    /// the OS CSPRNG. See [`ProofJob::with_entropy`] for the refusals.
    pub fn new(network: &NetworkInfo, inputs: ProofInputs) -> Result<Self, ProofRefusal> {
        Self::with_entropy(network, inputs, OsEntropy)
    }
}

impl<E: Entropy> ProofJob<E> {
    /// THE TEST SEAM: [`ProofJob::new`] drawing from `entropy` instead of the OS, so a replay can
    /// feed the bytes a recorded SDK job read. A wallet never calls this: the note salts and
    /// randoms it draws are what keep a proof's notes unlinkable.
    ///
    /// `InvalidRoute` when the catalog's chain id does not spell `NetworkId::chain_id` (a
    /// wrong-chain catalog must not sign), the route has OHTTP on, is another network's, names a
    /// pool its spelling does not parse to, or either URL is not a service URL: nothing is
    /// requested.
    pub fn with_entropy(
        network: &NetworkInfo,
        mut inputs: ProofInputs,
        entropy: E,
    ) -> Result<Self, ProofRefusal> {
        let route = &inputs.route;
        let chain_id = network.network.chain_id();
        if felt::parse(&network.chain_id) != Some(chain_id)
            || route.ohttp
            || route.network != network.network
            || !is_service_url(&route.discovery_url)
            || !is_service_url(&inputs.prover_url)
            || felt::parse(&route.pool_address) != Some(route.pool)
        {
            return Err(ProofRefusal::InvalidRoute);
        }
        inputs.limits = inputs.limits.clamped();
        inputs.retry = inputs.retry.clamped();
        Ok(Self {
            chain_id,
            scope_chain_id: network.chain_id.clone(),
            inputs,
            entropy,
            walks: WalkInputs {
                recipients: Vec::new(),
                tokens: Vec::new(),
            },
            restarted: false,
            stage: Stage::Fresh,
            next_id: 0,
        })
    }

    /// Feeds the last reply and returns the next step. A reply whose id is not the one
    /// outstanding, or a kind the current step does not expect, returns `OutOfOrder` and closes
    /// the job. `Finished` and `Failed` close it too.
    pub fn advance(
        &mut self,
        host: &dyn ProofHost,
        reply: PrivacyReply,
    ) -> Result<ProofStep, PrivacyError> {
        if matches!(self.stage, Stage::Closed) {
            return Err(PrivacyError::PlanClosed);
        }
        let result = self.step(host, reply);
        if !matches!(
            result,
            Ok(ProofStep::Post { .. } | ProofStep::Rpc { .. } | ProofStep::Wait { .. })
        ) {
            self.close();
        }
        result
    }

    /// Stops the job and drops everything it holds (walks, channels, calldata, invocation).
    /// Idempotent.
    pub fn close(&mut self) {
        self.stage = Stage::Closed;
    }

    fn step(
        &mut self,
        host: &dyn ProofHost,
        reply: PrivacyReply,
    ) -> Result<ProofStep, PrivacyError> {
        let stage = std::mem::replace(&mut self.stage, Stage::Closed);
        Ok(match (stage, reply) {
            (Stage::Fresh, PrivacyReply::Start { .. }) => {
                match walk_inputs(
                    &self.inputs.account,
                    &self.inputs.actions,
                    &self.contracts(),
                ) {
                    Ok(walks) => {
                        self.walks = walks;
                        self.start_walks(host)
                    }
                    Err(refusal) => fail(refusal.into()),
                }
            }
            (
                Stage::Outgoing { id, walk },
                PrivacyReply::Http {
                    id: got,
                    status,
                    body,
                    ..
                },
            ) if id == got => self.on_outgoing(host, walk, status, &body),
            (
                Stage::Incoming { id, walk, channels },
                PrivacyReply::Http {
                    id: got,
                    status,
                    body,
                    ..
                },
            ) if id == got => self.on_incoming(host, walk, channels, status, &body),
            (
                Stage::Nonce { id, calldata },
                PrivacyReply::Rpc {
                    id: got,
                    body,
                    now_ms,
                },
            ) if id == got => self.on_nonce(host, calldata, id, &body, now_ms),
            (
                Stage::Proving {
                    id,
                    rpc_id,
                    attempt,
                    invocation,
                },
                PrivacyReply::Http {
                    id: got,
                    status,
                    body,
                    ..
                },
            ) if id == got => self.on_prove(invocation, rpc_id, attempt, status, &body),
            (
                Stage::Waiting {
                    attempt,
                    invocation,
                },
                PrivacyReply::WaitElapsed { now_ms },
            ) => self.prove(host, invocation, attempt, now_ms),
            (stage, PrivacyReply::TransportFailure { id: got, .. }) => match stage {
                Stage::Outgoing { id, .. } | Stage::Incoming { id, .. } if id == got => fail(
                    ProofRefusal::Discovery(super::types::PrivacyFailure::ServiceUnavailable),
                ),
                Stage::Nonce { id, .. } if id == got => fail(ProofRefusal::NonceUnavailable),
                Stage::Proving { id, .. } if id == got => fail(ProofRefusal::ProverUnavailable),
                _ => return Err(PrivacyError::OutOfOrder),
            },
            _ => return Err(PrivacyError::OutOfOrder),
        })
    }

    /// The pool and anonymizer the invoke phase names.
    pub(super) fn contracts(&self) -> Contracts {
        Contracts {
            pool: self.inputs.route.pool,
            anonymizer: self.inputs.anonymizer,
        }
    }

    pub(super) fn next_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Asks the wallet for the key, scoped to this job's chain and pool; `None` once locked.
    pub(super) fn viewing_key(&self, host: &dyn ProofHost) -> Option<SecretFelt> {
        host.viewing_key(
            &self.scope_chain_id,
            &felt::canonical(&self.inputs.route.pool),
        )
    }
}

pub(super) fn fail(refusal: ProofRefusal) -> ProofStep {
    ProofStep::Failed { refusal }
}

#[cfg(test)]
#[path = "job_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "job_failure_tests.rs"]
mod failure_tests;
