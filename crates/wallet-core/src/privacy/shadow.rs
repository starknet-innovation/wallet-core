//! Shadow accounts (the SDK's `internal/shadow-account-address.ts` and `shadow-accounts.ts`): the
//! account's per-dapp identity commitments, the address the anonymizer deploys each one to, and
//! the `ComputeAndInvoke` data a shadow-account invoke compiles to.
//!
//! Secrets. The identity key (`compute_identity_key(user, viewing_key, anonymizer)`, from the
//! crate root) is derived from the viewing key and held as a `SecretFelt`; it is hashed in place,
//! inside `scrubbed`, by the allocation-free sponge, and dropped (wiped) on return. The partial
//! commitment, `hash(identity_key, dapp_name)`, is not kept secret: the extension sends it to the
//! node as `get_shadow_accounts`' argument, and Poseidon does not give the key back. It does link
//! every shadow account of one scope, so only the wallet's own reads send it, when the user asks.
//!
//! The reads around them (the commitment read with the discovery the extension runs first, and
//! `get_shadow_accounts`) are `shadow_reads`.

use super::hashes::poseidon_hash_many;
use super::proof::CollectPolicy;
use crate::scrub::scrubbed;
use crate::send::calls::Call;
use crate::{calculate_contract_address, compute_identity_key, selector, Felt, SecretFelt};

/// `PRIMER_CLASS_HASH`: the class every shadow account is first deployed from, which its address
/// is derived from (the anonymizer's `shadow_account_anonymizer.cairo`).
pub const PRIMER_CLASS_HASH: Felt =
    Felt::from_hex_unchecked("0x00123e6bc1c14ae9934e933d3f64916a6116dd6b036a922b2b1f0815e0d1d300");

/// `shadowAccountPartialCommitment`: `hash(compute_identity_key(user, viewing_key, anonymizer),
/// dapp_name)`, the SDK's bare `hash` (one `poseidonHashMany` over the two, no tag). `dapp_name`
/// is the felt: a short string is encoded by the caller.
pub fn partial_commitment(
    user: &Felt,
    viewing_key: &SecretFelt,
    anonymizer: &Felt,
    dapp_name: &Felt,
) -> Felt {
    scrubbed(|| {
        let identity_key = compute_identity_key(user, viewing_key, anonymizer);
        poseidon_hash_many([*identity_key.expose_secret(), *dapp_name].into_iter())
    })
}

/// `shadowAccountCommitment`: `hash(partial, nonce)`.
pub fn commitment(partial: &Felt, nonce: &Felt) -> Felt {
    poseidon_hash_many([*partial, *nonce].into_iter())
}

/// `shadowAccountAddress`: `calculateContractAddressFromHash(commitment, PRIMER_CLASS_HASH, [],
/// anonymizer)`. `None` only if Krusty refuses.
pub fn address(commitment: &Felt, anonymizer: &Felt) -> Option<Felt> {
    calculate_contract_address(commitment, &PRIMER_CLASS_HASH, &[], anonymizer).ok()
}

/// One open note of the batch, as the invoke-phase builders get it (`InvokeCalldataBuilderArgs`
/// `openNotes`): `compute_note_id(channel key, token, index)` and its token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenNoteRef {
    pub note_id: Felt,
    pub token: Felt,
}

/// `computeAdditionalData`: `[dapp_name, nonce]`.
pub fn compute_additional_data(dapp_name: &Felt, nonce: &Felt) -> Vec<Felt> {
    vec![*dapp_name, *nonce]
}

/// `invokeAdditionalData`: the anonymizer's `privacy_invoke_with_computation(0, calls,
/// open_notes)` calldata without its first felt, as `CallData.compile` writes it. `calls` is an
/// `Array<Call>`: its length, then each call's `to`, `selector(entrypoint)` and calldata span
/// (length, felts). `open_notes` is a `Span<OpenNote>`: its length, then each note's `note_id`,
/// `token` and `collect_policy` (`All` is `0`, `Diff` `1`, `Exact` `2` then the `u128`); one
/// policy applies to every open note of the batch.
pub fn invoke_additional_data(
    calls: &[Call],
    open_notes: &[OpenNoteRef],
    policy: CollectPolicy,
) -> Vec<Felt> {
    let mut out = vec![Felt::from(calls.len())];
    for call in calls {
        out.push(call.contract_address);
        out.push(selector(&call.entrypoint));
        out.push(Felt::from(call.calldata.len()));
        out.extend_from_slice(&call.calldata);
    }
    out.push(Felt::from(open_notes.len()));
    for note in open_notes {
        out.push(note.note_id);
        out.push(note.token);
        match policy {
            CollectPolicy::All => out.push(Felt::ZERO),
            CollectPolicy::Diff => out.push(Felt::ONE),
            CollectPolicy::Exact(amount) => {
                out.push(Felt::TWO);
                out.push(Felt::from(amount));
            }
        }
    }
    out
}
