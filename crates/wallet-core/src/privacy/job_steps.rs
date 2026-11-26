//! Reply handling for `ProofJob`, one function per step. Each returns the next step and leaves
//! the job's stage at what that step's reply needs; a refusal leaves it `Closed`.

use super::bodies::BlockRef;
use super::call_set::{call_set_hash, sign_call_set, COMPILE_ACTIONS};
use super::client_actions::compile_actions_calldata;
use super::compiler::compile;
use super::discovery::{AddressMap, Note, Walked};
use super::entropy::Entropy;
use super::incoming::{IncomingWalk, INCOMING_PATH};
use super::invocation::{pool_nonce, pool_nonce_request, ProofInvocation};
use super::job::{fail, ProofHost, ProofJob, ProofPurpose, ProofStep, Stage};
use super::outgoing::{OutgoingChannels, OutgoingWalk, OUTGOING_PATH};
use super::pool_actions::bound_sign_request;
use super::proof::ProofRefusal;
use super::prover::{judge_prove_reply, prove_body, ProveOutcome};
use super::types::PrivacyFailure;
use crate::SecretFelt;

impl<E: Entropy> ProofJob<E> {
    fn block(&self) -> BlockRef {
        BlockRef::Number(self.inputs.proving_block)
    }

    fn discovery_post(
        &mut self,
        path: &str,
        purpose: ProofPurpose,
        body: zeroize::Zeroizing<Vec<u8>>,
    ) -> (u64, ProofStep) {
        let id = self.next_id();
        let step = ProofStep::Post {
            id,
            url: format!("{}{path}", self.inputs.route.discovery_url),
            // Moved, not copied: the step owns the one zeroizing buffer.
            body,
            purpose,
        };
        (id, step)
    }

    /// Step 1 from its first page: the start, and the one reorg restart.
    pub(super) fn start_walks(&mut self, host: &dyn ProofHost) -> ProofStep {
        let walk = OutgoingWalk::new(
            self.inputs.route.pool,
            self.inputs.account,
            self.walks.recipients.clone(),
            self.block(),
        );
        self.post_outgoing(host, walk)
    }

    fn post_outgoing(&mut self, host: &dyn ProofHost, walk: OutgoingWalk) -> ProofStep {
        let Some(key) = self.viewing_key(host) else {
            return fail(ProofRefusal::Locked);
        };
        let (id, step) =
            self.discovery_post(OUTGOING_PATH, ProofPurpose::OutgoingState, walk.body(&key));
        self.stage = Stage::Outgoing { id, walk };
        step
    }

    fn post_incoming(
        &mut self,
        host: &dyn ProofHost,
        walk: IncomingWalk,
        channels: OutgoingChannels,
    ) -> ProofStep {
        let Some(key) = self.viewing_key(host) else {
            return fail(ProofRefusal::Locked);
        };
        let (id, step) =
            self.discovery_post(INCOMING_PATH, ProofPurpose::IncomingState, walk.body(&key));
        self.stage = Stage::Incoming { id, walk, channels };
        step
    }

    /// A walk's failure: the first reorg restarts the job from step 1, anything else ends it.
    fn on_walk_failure(&mut self, host: &dyn ProofHost, failure: PrivacyFailure) -> ProofStep {
        if failure == PrivacyFailure::Reorg && !self.restarted {
            self.restarted = true;
            return self.start_walks(host);
        }
        fail(ProofRefusal::Discovery(failure))
    }

    pub(super) fn on_outgoing(
        &mut self,
        host: &dyn ProofHost,
        walk: OutgoingWalk,
        status: u16,
        body: &[u8],
    ) -> ProofStep {
        match walk.on_reply(status, body) {
            Ok(Walked::More(walk)) => self.post_outgoing(host, walk),
            // No token in deficit: the SDK skips `discoverNotes`.
            Ok(Walked::Done(channels)) if self.walks.tokens.is_empty() => {
                self.compile(host, &channels, &AddressMap::default())
            }
            Ok(Walked::Done(channels)) => {
                let walk = IncomingWalk::new(
                    self.inputs.route.pool,
                    self.inputs.account,
                    self.walks.tokens.clone(),
                    self.block(),
                    self.inputs.max_notes,
                );
                self.post_incoming(host, walk, channels)
            }
            Err(failure) => self.on_walk_failure(host, failure),
        }
    }

    pub(super) fn on_incoming(
        &mut self,
        host: &dyn ProofHost,
        walk: IncomingWalk,
        channels: OutgoingChannels,
        status: u16,
        body: &[u8],
    ) -> ProofStep {
        match walk.on_reply(status, body) {
            Ok(Walked::More(walk)) => self.post_incoming(host, walk, channels),
            Ok(Walked::Done(notes)) => self.compile(host, &channels, &notes),
            Err(failure) => self.on_walk_failure(host, failure),
        }
    }

    /// Step 3: compile, refuse linkage, build and bind the calldata, then read the nonce. The key
    /// is asked for once here and dropped on return; only the bound calldata is kept.
    fn compile(
        &mut self,
        host: &dyn ProofHost,
        channels: &OutgoingChannels,
        notes: &AddressMap<Vec<Note>>,
    ) -> ProofStep {
        let Some(key) = self.viewing_key(host) else {
            return fail(ProofRefusal::Locked);
        };
        let contracts = self.contracts();
        let inputs = &self.inputs;
        let compiled = match compile(
            &inputs.account,
            &key,
            &inputs.actions,
            &contracts,
            channels,
            notes,
            &mut self.entropy,
        ) {
            Ok(compiled) => compiled,
            Err(refusal) => return fail(refusal.into()),
        };
        if compiled.user_linkage {
            return fail(ProofRefusal::PrivacyLeak);
        }
        let Some(calldata) = compile_actions_calldata(&inputs.account, &key, &compiled.actions)
        else {
            return fail(ProofRefusal::UnboundSignRequest);
        };
        // Our own compiler's output, checked as a hostile engine's was: nothing is signed unless
        // it moves exactly what was reviewed.
        if let Err(refusal) = bound_sign_request(
            &calldata,
            &inputs.account,
            &key,
            &inputs.actions,
            &contracts,
            &inputs.limits,
        ) {
            return fail(refusal);
        }
        let id = self.next_id();
        let body = pool_nonce_request(id, &self.inputs.route.pool);
        self.stage = Stage::Nonce { id, calldata };
        ProofStep::Rpc {
            id,
            body,
            purpose: ProofPurpose::PoolNonce,
        }
    }

    /// Step 5, then the first prove attempt under the nonce reply's clock (the SDK's
    /// `Date.now()` id).
    pub(super) fn on_nonce(
        &mut self,
        host: &dyn ProofHost,
        calldata: Vec<SecretFelt>,
        id: u64,
        body: &str,
        now_ms: u64,
    ) -> ProofStep {
        let Some(nonce) = pool_nonce(body, id) else {
            return fail(ProofRefusal::NonceUnavailable);
        };
        let (pool, account) = (self.inputs.route.pool, self.inputs.account);
        // S7 cuts here: a multisig stores the calldata, nonce and proving block with this digest,
        // gathers the threshold signature, and resumes at `prove` with the invocation.
        let digest = call_set_hash(&account, &pool, COMPILE_ACTIONS, &calldata, &self.chain_id);
        let signature =
            match sign_call_set(&digest, &self.inputs.public_key, |hash| host.sign(hash)) {
                Ok(signature) => signature,
                Err(refusal) => return fail(refusal),
            };
        let invocation = ProofInvocation::new(pool, &calldata, nonce, signature);
        self.prove(host, invocation, 0, now_ms)
    }

    /// Step 6: prove attempt `attempt` (from 0). The JSON-RPC id is the host clock of the reply
    /// that started it, as the SDK's is `Date.now()`. The body re-posts the viewing key, so the
    /// wallet is asked first, every attempt: a session that ended (during a wait, say) ends the
    /// job `Locked`. The key it hands back is dropped unread.
    pub(super) fn prove(
        &mut self,
        host: &dyn ProofHost,
        invocation: ProofInvocation,
        attempt: u32,
        now_ms: u64,
    ) -> ProofStep {
        if self.viewing_key(host).is_none() {
            return fail(ProofRefusal::Locked);
        }
        let id = self.next_id();
        let body = prove_body(now_ms, self.inputs.proving_block, &invocation);
        self.stage = Stage::Proving {
            id,
            rpc_id: now_ms,
            attempt,
            invocation,
        };
        ProofStep::Post {
            id,
            url: self.inputs.prover_url.clone(),
            body,
            purpose: ProofPurpose::Prove,
        }
    }

    pub(super) fn on_prove(
        &mut self,
        invocation: ProofInvocation,
        rpc_id: u64,
        attempt: u32,
        status: u16,
        body: &[u8],
    ) -> ProofStep {
        let pool = self.inputs.route.pool;
        match judge_prove_reply(status, body, rpc_id, &pool, &self.inputs.limits) {
            ProveOutcome::Proved(proof) => ProofStep::Finished { proof },
            ProveOutcome::Refused(refusal) => fail(refusal),
            ProveOutcome::Busy => match self.inputs.retry.delay_ms(attempt) {
                Some(delay_ms) => {
                    self.stage = Stage::Waiting {
                        attempt: attempt.saturating_add(1),
                        invocation,
                    };
                    ProofStep::Wait { delay_ms }
                }
                None => fail(ProofRefusal::ProverFailed),
            },
        }
    }
}
