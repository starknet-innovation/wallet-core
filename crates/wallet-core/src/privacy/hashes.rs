//! The privacy SDK's domain-tagged Poseidon (`utils/crypto.ts` `hash`, `utils/hashes.ts`): one
//! `poseidonHashMany` over the tag, read as a Cairo short string, followed by the values.
//!
//! Only the hashes the client path uses are here. The two that consume the viewing key
//! (`compute_channel_key`, `compute_identity_key`) sit behind `kms.rs` and are reached through the
//! crate root; the nullifier, outgoing-channel-id, marker, subchannel and encryption hashes serve
//! only on-chain discovery and the SDK's test pool, so they are not ported.

use starknet_types_core::hash::{Poseidon, StarkHash};

use crate::{Felt, SecretFelt};

const NOTE_ID_TAG: &str = "NOTE_ID_TAG:V1";

/// `hash(tag, ...values)`. `tag` is one of this crate's tag constants, all within a short
/// string's 31 bytes.
pub(crate) fn tagged_hash(tag: &str, values: &[Felt]) -> Felt {
    let mut input = Vec::with_capacity(values.len() + 1);
    input.push(Felt::from_bytes_be_slice(tag.as_bytes()));
    input.extend_from_slice(values);
    Poseidon::hash_array(&input)
}

/// `compute_note_id(channel_key, token, index)`: `hash(NOTE_ID_TAG, channel_key, token, index, 0)`.
pub fn compute_note_id(channel_key: &SecretFelt, token: &Felt, index: u32) -> Felt {
    tagged_hash(
        NOTE_ID_TAG,
        &[
            *channel_key.expose_secret(),
            *token,
            Felt::from(index),
            Felt::ZERO,
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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
