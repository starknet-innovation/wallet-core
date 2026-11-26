//! A multisig's proof held while its co-owners sign (#182, S7; the extension's `held-proof.ts`
//! and `multisig/redaction.ts`). The SDK draws fresh randomness every time it compiles, and that
//! randomness is inside the digest the co-owners sign, so the job that compiled the request ends
//! holding it (`ProofStep::Held`) and a later job resumes exactly that request
//! (`ProofJob::resumed`).
//!
//! What is held: the `CallSet` digest, the pool nonce the invocation was fixed at, the proving
//! block, and the bound `compile_actions` calldata, which carries the viewing key at index 1. The
//! calldata leaves the core only redacted: the key slot, and only that slot, replaced by the
//! wallet's marker felt. Redacting refuses a calldata where the key or the marker appears anywhere
//! else (the extension replaces every occurrence, so a felt equal to the key in, say, a recipient
//! field would come back as the key: starknet-innovation/bramble#1073); restoring puts the key
//! back at that one slot and refuses a marker anywhere else.
//!
//! The digest covers the calldata, but not the nonce or the block, so a redacted record carries a
//! seal: the tagged Poseidon hash of the viewing key, the digest, the nonce and the block. A
//! record edited in any of them is refused on resume before anything is sent. The extension has
//! no seal; it relies on its store.

use std::fmt;

use super::hashes::tagged_hash;
use crate::scrub::scrubbed;
use crate::{Felt, SecretFelt};

/// Where `compile_actions(user, viewing_key, actions)` keeps the key.
const KEY_SLOT: usize = 1;

const SEAL_TAG: &str = "STRK20_HELD_PROOF:V1";

/// A held proof as the job ends with it. The calldata HOLDS THE VIEWING KEY: a wallet keeps only
/// [`HeldProof::redact`]'s output. Its `Debug` is redacted.
#[derive(Clone, PartialEq, Eq)]
pub struct HeldProof {
    /// The `CallSet` digest the co-owners sign.
    pub digest: Felt,
    /// The pool nonce the invocation is built at.
    pub nonce: Felt,
    /// The block the proof is made against.
    pub proving_block: u64,
    calldata: Vec<SecretFelt>,
}

/// A held proof as a wallet stores and shares it: the calldata with the key slot replaced by the
/// wallet's marker, and the seal over the rest. Public: the viewing key is not in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactedHeldProof {
    pub digest: Felt,
    pub nonce: Felt,
    pub proving_block: u64,
    /// `compile_actions` with `marker` at index 1.
    pub calldata: Vec<Felt>,
    /// Binds `digest`, `nonce` and `proving_block` under the viewing key.
    pub seal: Felt,
}

fn seal(key: &SecretFelt, digest: &Felt, nonce: &Felt, proving_block: u64) -> Felt {
    scrubbed(|| {
        tagged_hash(
            SEAL_TAG,
            &[
                *key.expose_secret(),
                *digest,
                *nonce,
                Felt::from(proving_block),
            ],
        )
    })
}

impl HeldProof {
    pub(super) fn new(
        digest: Felt,
        nonce: Felt,
        proving_block: u64,
        calldata: Vec<SecretFelt>,
    ) -> Self {
        Self {
            digest,
            nonce,
            proving_block,
            calldata,
        }
    }

    /// The record with the key slot replaced by `marker`. `None` when the calldata has no key
    /// slot, `marker` is the key, or the key or `marker` appears anywhere but that slot.
    pub fn redact(&self, marker: &Felt) -> Option<RedactedHeldProof> {
        scrubbed(|| {
            let key = self.calldata.get(KEY_SLOT)?;
            let clean = self.calldata.iter().enumerate().all(|(at, value)| {
                value != marker && (at == KEY_SLOT || value != key.expose_secret())
            });
            if !clean {
                return None;
            }
            let calldata = self
                .calldata
                .iter()
                .enumerate()
                .map(|(at, value)| {
                    if at == KEY_SLOT {
                        *marker
                    } else {
                        *value.expose_secret()
                    }
                })
                .collect();
            Some(RedactedHeldProof {
                digest: self.digest,
                nonce: self.nonce,
                proving_block: self.proving_block,
                calldata,
                seal: seal(key, &self.digest, &self.nonce, self.proving_block),
            })
        })
    }

    /// The bound `compile_actions` calldata. Secret: it holds the viewing key.
    pub fn calldata(&self) -> &[SecretFelt] {
        &self.calldata
    }
}

impl RedactedHeldProof {
    /// The held proof with `key` back at the key slot ([`restore_calldata`]), if the seal is this
    /// record's under `key`.
    pub(super) fn restore(&self, key: &SecretFelt, marker: &Felt) -> Option<HeldProof> {
        let sealed = scrubbed(|| seal(key, &self.digest, &self.nonce, self.proving_block));
        if sealed != self.seal {
            return None;
        }
        let calldata = restore_calldata(&self.calldata, key, marker)?;
        Some(HeldProof::new(
            self.digest,
            self.nonce,
            self.proving_block,
            calldata,
        ))
    }
}

/// Redacted `compile_actions` calldata (a held record's, or the calls a co-owner shares) with
/// `key` back at the key slot. `None` unless that slot holds `marker`, no other felt is `marker`
/// or `key`, and `marker` is not `key`.
pub fn restore_calldata(
    redacted: &[Felt],
    key: &SecretFelt,
    marker: &Felt,
) -> Option<Vec<SecretFelt>> {
    scrubbed(|| {
        let clean = redacted.get(KEY_SLOT) == Some(marker)
            && key != marker
            && redacted
                .iter()
                .enumerate()
                .all(|(at, value)| key != value && (at == KEY_SLOT || value != marker));
        if !clean {
            return None;
        }
        let mut calldata = Vec::with_capacity(redacted.len());
        calldata.extend(redacted.iter().enumerate().map(|(at, value)| {
            if at == KEY_SLOT {
                key.clone()
            } else {
                SecretFelt::new(*value)
            }
        }));
        Some(calldata)
    })
}

impl fmt::Debug for HeldProof {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HeldProof")
            .field("digest", &self.digest)
            .field("nonce", &self.nonce)
            .field("proving_block", &self.proving_block)
            .field(
                "calldata",
                &format_args!("[redacted {} felts]", self.calldata.len()),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: Felt = Felt::from_hex_unchecked("0xabc");
    const MARKER: Felt = Felt::from_hex_unchecked("0x7265646163746564");

    fn held(calldata: &[Felt]) -> HeldProof {
        HeldProof::new(
            Felt::from(0xd1u64),
            Felt::from(0x2au64),
            100,
            calldata.iter().copied().map(SecretFelt::new).collect(),
        )
    }

    fn plain(held: &HeldProof) -> Vec<Felt> {
        held.calldata().iter().map(|v| *v.expose_secret()).collect()
    }

    #[test]
    fn redacts_only_the_key_slot_and_restores_it() {
        let calldata = [Felt::from(7u64), KEY, Felt::TWO, Felt::THREE];
        let redacted = held(&calldata).redact(&MARKER).unwrap();
        assert_eq!(
            redacted.calldata,
            [Felt::from(7u64), MARKER, Felt::TWO, Felt::THREE]
        );
        let restored = redacted.restore(&SecretFelt::new(KEY), &MARKER).unwrap();
        assert_eq!(plain(&restored), calldata);
        assert_eq!(restored, held(&calldata));
        assert!(!format!("{restored:?}").contains("abc"));
    }

    #[test]
    fn redact_refuses_the_key_or_marker_outside_the_slot() {
        let key_twice = [Felt::from(7u64), KEY, KEY];
        let marker_present = [Felt::from(7u64), KEY, MARKER];
        let no_slot = [Felt::from(7u64)];
        for calldata in [&key_twice[..], &marker_present, &no_slot] {
            assert_eq!(held(calldata).redact(&MARKER), None, "{calldata:?}");
        }
        assert_eq!(held(&[Felt::ONE, KEY]).redact(&KEY), None);
    }

    #[test]
    fn restore_refuses_a_marker_outside_the_slot_or_the_key_anywhere() {
        let key = SecretFelt::new(KEY);
        let good = held(&[Felt::from(7u64), KEY, Felt::TWO])
            .redact(&MARKER)
            .unwrap();
        let mut marker_twice = good.clone();
        marker_twice.calldata[2] = MARKER;
        let mut key_elsewhere = good.clone();
        key_elsewhere.calldata[2] = KEY;
        let mut slot_moved = good.clone();
        slot_moved.calldata.swap(1, 2);
        for record in [marker_twice, key_elsewhere, slot_moved] {
            assert_eq!(record.restore(&key, &MARKER), None, "{record:?}");
        }
        assert_eq!(good.restore(&SecretFelt::new(MARKER), &MARKER), None);
        assert_eq!(good.restore(&key, &Felt::from(9u64)), None);
        assert_eq!(
            good.restore(&SecretFelt::new(Felt::from(0xabdu64)), &MARKER),
            None
        );
    }

    #[test]
    fn the_seal_binds_digest_nonce_and_block_under_the_key() {
        let key = SecretFelt::new(KEY);
        let good = held(&[Felt::from(7u64), KEY]).redact(&MARKER).unwrap();
        let mut digest = good.clone();
        digest.digest += Felt::ONE;
        let mut nonce = good.clone();
        nonce.nonce += Felt::ONE;
        let mut block = good.clone();
        block.proving_block += 1;
        let mut sealed = good.clone();
        sealed.seal += Felt::ONE;
        for record in [digest, nonce, block, sealed] {
            assert_eq!(record.restore(&key, &MARKER), None, "{record:?}");
        }
        assert!(good.restore(&key, &MARKER).is_some());
    }
}
