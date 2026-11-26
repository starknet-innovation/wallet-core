//! The privacy SDK's domain-tagged Poseidon (`utils/crypto.ts` `hash`, `utils/hashes.ts`): one
//! `poseidonHashMany` over the tag, read as a Cairo short string, followed by the values.
//!
//! Only the hashes the client path uses are here. The two that consume the viewing key
//! (`compute_channel_key`, `compute_identity_key`) sit behind `kms.rs` and are reached through the
//! crate root; the nullifier, outgoing-channel-id, marker, subchannel and encryption hashes serve
//! only on-chain discovery and the SDK's test pool, so they are not ported.

use starknet_types_core::hash::Poseidon;

use crate::scrub::scrubbed;
use crate::{Felt, SecretFelt};

const NOTE_ID_TAG: &str = "NOTE_ID_TAG:V1";

/// `hash(tag, ...values)`. `tag` is one of this crate's tag constants, all within a short
/// string's 31 bytes. Keys pass through here, so nothing is copied to the heap: callers pass a
/// stack array, and the sponge runs over it in place (`Poseidon::hash_array` would copy the
/// input into a `Vec` it never wipes). Key-consuming callers run it inside `scrubbed`.
pub(crate) fn tagged_hash(tag: &str, values: &[Felt]) -> Felt {
    let tag = Felt::from_bytes_be_slice(tag.as_bytes());
    poseidon_hash_many(core::iter::once(tag).chain(values.iter().copied()))
}

/// `poseidonHashMany`: Starknet's Poseidon sponge (rate 2, the input padded with one, then zero
/// to a full block), as `Poseidon::hash_array` computes it, without allocating.
pub(crate) fn poseidon_hash_many(mut felts: impl Iterator<Item = Felt>) -> Felt {
    let mut state = [Felt::ZERO; 3];
    loop {
        let (a, b, last) = match (felts.next(), felts.next()) {
            (Some(a), Some(b)) => (a, b, false),
            (Some(a), None) => (a, Felt::ONE, true),
            (None, _) => (Felt::ONE, Felt::ZERO, true),
        };
        state[0] += a;
        state[1] += b;
        Poseidon::hades_permutation(&mut state);
        if last {
            return state[0];
        }
    }
}

/// `compute_note_id(channel_key, token, index)`: `hash(NOTE_ID_TAG, channel_key, token, index, 0)`.
pub fn compute_note_id(channel_key: &SecretFelt, token: &Felt, index: u32) -> Felt {
    scrubbed(|| {
        tagged_hash(
            NOTE_ID_TAG,
            &[
                *channel_key.expose_secret(),
                *token,
                Felt::from(index),
                Felt::ZERO,
            ],
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use starknet_types_core::hash::StarkHash;

    #[test]
    fn the_sponge_is_poseidon_hash_array() {
        let felts: Vec<Felt> = (1u64..=7)
            .map(|n| Felt::from(n * 0x1234_5678_9abc))
            .collect();
        for len in 0..=felts.len() {
            assert_eq!(
                poseidon_hash_many(felts[..len].iter().copied()),
                Poseidon::hash_array(&felts[..len]),
                "{len} felts"
            );
        }
    }

    #[test]
    fn the_tag_is_the_cairo_short_string() {
        // 'NOTE_ID_TAG:V1' as Cairo writes it, and the hash is one `hash_array` over tag and values.
        let tag = Felt::from_hex_unchecked("0x4e4f54455f49445f5441473a5631");
        let key = SecretFelt::new(Felt::from(7u8));
        assert_eq!(
            compute_note_id(&key, &Felt::from(9u8), 3),
            Poseidon::hash_array(&[
                tag,
                Felt::from(7u8),
                Felt::from(9u8),
                Felt::THREE,
                Felt::ZERO
            ])
        );
    }
}
