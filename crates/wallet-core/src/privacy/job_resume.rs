//! Resuming a held multisig proof (#182, S7; the extension's `findResumableStrk20Request` and
//! `readMultisigStrk20Window`). Nothing is compiled, discovered, drawn or signed: the job restores
//! the viewing key into the held calldata, recomputes the digest and requires the held one,
//! binds the calldata to the reviewed actions again, takes the co-owners' aggregate (checked as
//! at the sign step), and reads the pool's proof window before the prove stage.

use super::call_set::{call_set_hash, COMPILE_ACTIONS};
use super::entropy::Entropy;
use super::held::RedactedHeldProof;
use super::invocation::ProofInvocation;
use super::job::{fail, ProofHost, ProofJob, ProofPurpose, ProofStep, Stage};
use super::pool_actions::bound_sign_request;
use super::proof::{AuthorizationRefusal, ProofRefusal, ProofSigner};
use crate::send::{judge, rpc_bodies};
use crate::{selector, Felt};

/// The pool's view of how many blocks past its base block a proof is accepted.
const PROOF_VALIDITY_BLOCKS: &str = "get_proof_validity_blocks";

/// `strk20ProofSubmittable`: the pool accepts a proof made at `proving_block` only while
/// `proving_block < current <= proving_block + validity` (so never with a zero window).
pub(super) fn within_proof_window(proving_block: u64, current: u64, validity: &Felt) -> bool {
    current
        .checked_sub(proving_block)
        .is_some_and(|age| age > 0 && Felt::from(age) <= *validity)
}

impl<E: Entropy> ProofJob<E> {
    /// `Start` on a resumed job: everything up to the first chain read, refused before anything
    /// is sent if the held record or its aggregate does not hold up.
    pub(super) fn start_resume(
        &mut self,
        host: &dyn ProofHost,
        held: &RedactedHeldProof,
        marker: &Felt,
    ) -> ProofStep {
        if !matches!(self.inputs.signer, ProofSigner::Multisig { .. }) {
            return fail(ProofRefusal::HeldMismatch);
        }
        let Some(key) = self.viewing_key(host) else {
            return fail(ProofRefusal::Locked);
        };
        let Some(restored) = held.restore(&key, marker) else {
            return fail(ProofRefusal::HeldMismatch);
        };
        let (pool, account) = (self.inputs.route.pool, self.inputs.account);
        let calldata = restored.calldata();
        let digest = call_set_hash(&account, &pool, COMPILE_ACTIONS, calldata, &self.chain_id);
        if digest != restored.digest {
            return fail(ProofRefusal::HeldMismatch);
        }
        let inputs = &self.inputs;
        if bound_sign_request(
            calldata,
            &account,
            &key,
            &inputs.actions,
            &self.contracts(),
            &inputs.limits,
        )
        .is_err()
        {
            return fail(ProofRefusal::HeldUnbound);
        }
        let signature = match self.authorize(host, &digest) {
            Ok(Some(signature)) => signature,
            Ok(None) => {
                return fail(ProofRefusal::Authorization(
                    AuthorizationRefusal::BelowThreshold,
                ))
            }
            Err(refusal) => return fail(refusal),
        };
        // The held block is the one the request was compiled and signed for.
        self.inputs.proving_block = restored.proving_block;
        let invocation = ProofInvocation::resumed(pool, calldata, restored.nonce, signature);
        let id = self.next_id();
        self.stage = Stage::BlockNumber { id, invocation };
        ProofStep::Rpc {
            id,
            body: rpc_bodies::block_number(id),
            purpose: ProofPurpose::BlockNumber,
        }
    }

    pub(super) fn on_block_number(
        &mut self,
        id: u64,
        invocation: ProofInvocation,
        body: &str,
    ) -> ProofStep {
        let Some(current) = judge::block_number(judge::answer(body, id)) else {
            return fail(ProofRefusal::WindowUnavailable);
        };
        let id = self.next_id();
        let pool = self.inputs.route.pool;
        self.stage = Stage::ProofValidity {
            id,
            current,
            invocation,
        };
        ProofStep::Rpc {
            id,
            body: rpc_bodies::call(id, &pool, &selector(PROOF_VALIDITY_BLOCKS), &[]),
            purpose: ProofPurpose::ProofValidity,
        }
    }

    /// The window, then the first prove attempt under this reply's clock.
    pub(super) fn on_validity(
        &mut self,
        host: &dyn ProofHost,
        id: u64,
        current: u64,
        invocation: ProofInvocation,
        body: &str,
        now_ms: u64,
    ) -> ProofStep {
        let validity = match judge::single_felt(judge::answer(body, id)) {
            judge::CallAnswer::Value(Some(validity)) => validity,
            _ => return fail(ProofRefusal::WindowUnavailable),
        };
        if !within_proof_window(self.inputs.proving_block, current, &validity) {
            return ProofStep::Expired;
        }
        self.prove(host, invocation, 0, now_ms)
    }
}
