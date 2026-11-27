//! The proof invocation (the SDK's `ProofInvocationFactory.create` with
//! `getDefaultProofDetails`): the pool's `compile_actions` wrapped as `__execute__` calldata with
//! the pool as sender, in an `INVOKE` v3 transaction with the SDK's fixed details, and the pool's
//! nonce it is built at (`ProvingServiceProofProvider.getDefaultDetails`, `starknet_getNonce` at
//! `latest`).
//!
//! The `compile_actions` calldata carries the viewing key (and values derived from it), so the
//! invocation holds its calldata as `SecretFelt`s, zeroed on drop, in a buffer sized exactly up
//! front, and serialises into one exactly sized zeroizing buffer (as `bodies::with_key`), whose
//! hex is written straight in with no temporary strings, inside `scrubbed`. Its `Debug` is
//! redacted.

use std::fmt;

use zeroize::{Zeroize, Zeroizing};

use super::call_set::COMPILE_ACTIONS;
use crate::scrub::scrubbed;
use crate::send::calls::call_head;
use crate::send::{judge, rpc_bodies};
use crate::{felt, Felt, SecretFelt};

/// The SDK's fixed resource bounds (`DEFAULT_L2_GAS_MAX_AMOUNT`; L1 and L1 data gas at 1, every
/// price 0). The prover does not charge them; they only have to validate.
const L1_GAS_MAX_AMOUNT: u64 = 1;
const L2_GAS_MAX_AMOUNT: u64 = 100_000_000;
const L1_DATA_GAS_MAX_AMOUNT: u64 = 1;

/// One proof invocation: what `ProofInvocationFactory.create` returns, kept as data so a held
/// invocation (a multisig waiting on co-signers) can be serialised again unchanged.
#[derive(Clone, PartialEq)]
pub struct ProofInvocation {
    /// Rebuilt from a held envelope (`ProofInvocation::resumed`): the extension spreads the held
    /// envelope and then sets `calldata` and `signature`, so they come last in its JSON.
    resumed: bool,
    /// The pool: the sender, and the one call's contract.
    pub pool: Felt,
    /// The pool's nonce at `latest` ([`pool_nonce`]).
    pub nonce: Felt,
    /// The account's signature as the SDK's signer returned it (`stark.formatSignature`).
    pub signature: Vec<Felt>,
    /// `[1, pool, selector("compile_actions"), len, ...compile_actions]`.
    calldata: Vec<SecretFelt>,
}

impl ProofInvocation {
    /// Wraps `compile_actions` (`[user, viewing_key, ...actions]`, the pool ABI's encoding) as the
    /// pool's `__execute__` calldata: `calls::execute_calldata` of the one call, laid straight
    /// into secret storage, so no plain copy of the key is made.
    pub fn new(
        pool: Felt,
        compile_actions: &[SecretFelt],
        nonce: Felt,
        signature: Vec<Felt>,
    ) -> Self {
        let head = call_head(&pool, COMPILE_ACTIONS, compile_actions.len());
        let mut calldata = Vec::with_capacity(1 + head.len() + compile_actions.len());
        calldata.push(SecretFelt::new(Felt::ONE));
        calldata.extend(head.into_iter().map(SecretFelt::new));
        calldata.extend_from_slice(compile_actions);
        Self {
            resumed: false,
            pool,
            nonce,
            signature,
            calldata,
        }
    }

    /// [`ProofInvocation::new`] for a held multisig proof, as the extension rebuilds it
    /// (`multisig-strk20-held.ts` `signed`): the same members, with `calldata` and `signature`
    /// after the envelope in its JSON.
    pub fn resumed(
        pool: Felt,
        compile_actions: &[SecretFelt],
        nonce: Felt,
        signature: Vec<Felt>,
    ) -> Self {
        Self {
            resumed: true,
            ..Self::new(pool, compile_actions, nonce, signature)
        }
    }

    /// The `__execute__` calldata. Secret: it holds the viewing key.
    pub fn calldata(&self) -> &[SecretFelt] {
        &self.calldata
    }

    /// `JSON.stringify(invocation)`, byte for byte. MAY CONTAIN THE VIEWING KEY: never log,
    /// persist, display or inspect it.
    pub fn to_json(&self) -> Zeroizing<Vec<u8>> {
        self.json_within("", "")
    }

    /// The invocation's JSON between `before` and `after` (public text), in one buffer sized
    /// exactly up front, so the calldata is never left behind by a reallocation.
    pub(super) fn json_within(&self, before: &str, after: &str) -> Zeroizing<Vec<u8>> {
        // The calldata's hex passes through registers and stack temporaries: overwritten after.
        scrubbed(|| self.json_between(before, after))
    }

    fn json_between(&self, before: &str, after: &str) -> Zeroizing<Vec<u8>> {
        let sender = format!(
            "{{\"type\":\"INVOKE\",\"sender_address\":\"{}\"",
            felt::canonical(&self.pool)
        );
        let signature: Vec<String> = self
            .signature
            .iter()
            .map(|value| format!("\"{}\"", felt::canonical(value)))
            .collect();
        let bound = |amount: u64| {
            format!("{{\"max_amount\":\"{amount:#x}\",\"max_price_per_unit\":\"0x0\"}}")
        };
        let envelope = format!(
            "\"nonce\":\"{}\",\"resource_bounds\":{{\"l1_gas\":{},\
             \"l2_gas\":{},\"l1_data_gas\":{}}},\"tip\":\"0x0\",\"paymaster_data\":[],\
             \"account_deployment_data\":[],\"nonce_data_availability_mode\":\"L1\",\
             \"fee_data_availability_mode\":\"L1\",\"version\":\"0x3\"",
            felt::canonical(&self.nonce),
            bound(L1_GAS_MAX_AMOUNT),
            bound(L2_GAS_MAX_AMOUNT),
            bound(L1_DATA_GAS_MAX_AMOUNT),
        );
        let signature = format!("\"signature\":[{}]", signature.join(","));
        // The SDK's member order, or a held envelope's with `calldata` and `signature` last.
        let (head, tail) = if self.resumed {
            (
                format!("{sender},{envelope},\"calldata\":["),
                format!("],{signature}}}"),
            )
        } else {
            (
                format!("{sender},\"calldata\":["),
                format!("],{signature},{envelope}}}"),
            )
        };
        // Each felt is `"0x…"`, with a comma between.
        let calldata_len = self
            .calldata
            .iter()
            .map(|value| hex_digits(value.expose_secret()) + 4)
            .sum::<usize>()
            + self.calldata.len().saturating_sub(1);
        let len = before.len() + head.len() + calldata_len + tail.len() + after.len();
        let mut out = Zeroizing::new(Vec::with_capacity(len));
        out.extend_from_slice(before.as_bytes());
        out.extend_from_slice(head.as_bytes());
        for (at, value) in self.calldata.iter().enumerate() {
            if at > 0 {
                out.push(b',');
            }
            out.push(b'"');
            push_hex(&mut out, value.expose_secret());
            out.push(b'"');
        }
        out.extend_from_slice(tail.as_bytes());
        out.extend_from_slice(after.as_bytes());
        debug_assert_eq!(out.len(), len);
        debug_assert_eq!(out.capacity(), len);
        out
    }
}

impl fmt::Debug for ProofInvocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProofInvocation")
            .field("resumed", &self.resumed)
            .field("pool", &self.pool)
            .field("nonce", &self.nonce)
            .field("signature", &self.signature)
            .field(
                "calldata",
                &format_args!("[redacted {} felts]", self.calldata.len()),
            )
            .finish()
    }
}

/// Minimal lowercase hex digits of `value` (`felt::canonical` without the `0x`).
fn hex_digits(value: &Felt) -> usize {
    value.bits().div_ceil(4).max(1)
}

/// Writes `felt::canonical(value)` into `out` without an intermediate string, wiping the bytes it
/// read the value through.
fn push_hex(out: &mut Vec<u8>, value: &Felt) {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut bytes = value.to_bytes_be();
    out.extend_from_slice(b"0x");
    for nibble in (0..hex_digits(value)).rev() {
        let byte = bytes.get(31 - nibble / 2).copied().unwrap_or_default();
        let digit = if nibble % 2 == 0 {
            byte & 0xf
        } else {
            byte >> 4
        };
        out.push(DIGITS[usize::from(digit)]);
    }
    bytes.zeroize();
}

/// `starknet_getNonce` for the pool at `latest`, as `getNonceForAddress(pool, "latest")` (the
/// same members as starknet.js sends, in `rpc_bodies`' key order). Public data.
pub fn pool_nonce_request(id: u64, pool: &Felt) -> String {
    rpc_bodies::nonce(id, pool)
}

/// Judges the pool's nonce reply. Stricter than `send::judge::nonce`: the pool exists, so a
/// JSON-RPC error is no nonce rather than 0, and the invocation is not built.
pub fn pool_nonce(body: &str, id: u64) -> Option<Felt> {
    judge::deployed_nonce(judge::answer(body, id))
}

#[cfg(test)]
#[path = "invocation_tests.rs"]
pub(crate) mod tests;
