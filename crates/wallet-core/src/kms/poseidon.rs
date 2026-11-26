//! The privacy SDK's domain-tagged Poseidon (`utils/crypto.ts` `hash`): one `poseidonHashMany`
//! over the tag, read as a Cairo short string, followed by the values. Here, beside the
//! viewing-key hashes that consume it, so `kms` depends on nothing in `privacy`; `privacy::hashes`
//! reaches it through the crate root.

use starknet_types_core::felt::Felt;
use starknet_types_core::hash::Poseidon;

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
}
