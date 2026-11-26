//! The SNIP-12 `CallSet` the privacy pool verifies (the extension's `call-set-signer.ts`): the
//! digest over the one `compile_actions` call the account signs, computed here from the core's own
//! account and chain, and its signature, made by the wallet's signing path and checked before it
//! is used.
//!
//! The call's calldata holds the viewing key, so it is hashed as the secrets it is held in: the
//! sponge streams over them without a plain copy (`hashes::poseidon_hash_many`), inside
//! `scrubbed`.

use starknet_types_core::hash::{Poseidon, StarkHash};

use super::hashes::poseidon_hash_many;
use super::proof::ProofRefusal;
use crate::felt;
use crate::scrub::scrubbed;
use crate::send::types::SignRefusal;
use crate::{selector, stark_signature_is_valid, Felt, SecretFelt, StarkSignature};

const STARKNET_DOMAIN_TYPE_HASH: Felt =
    Felt::from_hex_unchecked("0x1ff2f602e42168014d405a94f75e8a93d640751d71d16311266e140d8b0a210");
const CALL_TYPE_HASH: Felt =
    Felt::from_hex_unchecked("0x3635c7f2a7ba93844c0d064e18e487f35ab90f7c39d00f186a781fc3f0c2ca9");
const CALL_SET_TYPE_HASH: Felt =
    Felt::from_hex_unchecked("0x308b7462f924efba15fc992e6827eb3a748fcc79f091914156756437fa22909");

/// The one entrypoint the SDK's signer may be asked to sign for.
pub const COMPILE_ACTIONS: &str = "compile_actions";

/// `computeStrk20CallSetHash(account, [call], chainId)`: the digest binding the account, the
/// chain and the one call `contract.entrypoint(calldata)`, with no additional data (the SDK's
/// signer passes none). The SDK only ever asks the account to sign one call.
pub fn call_set_hash(
    account: &Felt,
    contract: &Felt,
    entrypoint: &str,
    calldata: &[SecretFelt],
    chain_id: &Felt,
) -> Felt {
    let calldata_hash =
        scrubbed(|| poseidon_hash_many(calldata.iter().map(|value| *value.expose_secret())));
    let call_hash = Poseidon::hash_array(&[
        CALL_TYPE_HASH,
        *contract,
        selector(entrypoint),
        calldata_hash,
    ]);
    let domain = Poseidon::hash_array(&[
        STARKNET_DOMAIN_TYPE_HASH,
        Felt::from_bytes_be_slice(b"CallSet"),
        Felt::ONE,
        *chain_id,
        Felt::ONE,
    ]);
    let call_set = Poseidon::hash_array(&[
        CALL_SET_TYPE_HASH,
        Poseidon::hash_array(&[call_hash]),
        Poseidon::hash_array(&[]),
    ]);
    Poseidon::hash_array(&[
        Felt::from_bytes_be_slice(b"StarkNet Message"),
        domain,
        *account,
        call_set,
    ])
}

/// Signs `hash` through `sign` and returns the raw `[r, s]` the pool verifies. The signature
/// must come from `public_key` and verify over `hash`, so a wallet that signed something else,
/// or with another key, is refused before anything reaches the prover.
pub(super) fn sign_call_set(
    hash: &Felt,
    public_key: &Felt,
    sign: impl FnOnce(&Felt) -> Result<StarkSignature, SignRefusal>,
) -> Result<Vec<Felt>, ProofRefusal> {
    let signature = sign(hash)?;
    match (
        felt::parse(&signature.public_key),
        felt::parse(&signature.r),
        felt::parse(&signature.s),
    ) {
        (Some(key), Some(r), Some(s))
            if key == *public_key && stark_signature_is_valid(public_key, hash, &r, &s) =>
        {
            Ok(vec![r, s])
        }
        _ => Err(ProofRefusal::SigningFailed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secret(values: &[u64]) -> Vec<SecretFelt> {
        values
            .iter()
            .map(|value| SecretFelt::new(Felt::from(*value)))
            .collect()
    }

    fn hash(account: u64, calldata: &[u64], chain: &[u8]) -> Felt {
        call_set_hash(
            &Felt::from(account),
            &Felt::from(0x111u64),
            "approve",
            &secret(calldata),
            &Felt::from_bytes_be_slice(chain),
        )
    }

    /// `call-set-signer.test.ts`: the privacy protocol's cross-language golden vector. A drift
    /// here makes every private proof's authorization fail on chain.
    #[test]
    fn matches_the_privacy_contract_golden_vector() {
        assert_eq!(
            hash(0x1234, &[1, 2], b"TEST"),
            Felt::from_hex_unchecked(
                "0x79d05f5b8993f5a0a18c6f7001e4d573c3eb97f322f6008cc1420f4f611501f"
            )
        );
    }

    #[test]
    fn binds_call_account_and_chain() {
        let baseline = hash(0x1234, &[1, 2], b"TEST");
        assert_ne!(hash(0x999, &[1, 2], b"TEST"), baseline);
        assert_ne!(hash(0x1234, &[1, 2], b"OTHER"), baseline);
        assert_ne!(hash(0x1234, &[1, 3], b"TEST"), baseline);
        assert_ne!(hash(0x1234, &[], b"TEST"), baseline);
        let other_entrypoint = call_set_hash(
            &Felt::from(0x1234u64),
            &Felt::from(0x111u64),
            "transfer",
            &secret(&[1, 2]),
            &Felt::from_bytes_be_slice(b"TEST"),
        );
        assert_ne!(other_entrypoint, baseline);
    }

    fn key() -> SecretFelt {
        SecretFelt::new(Felt::from(0x1234567890abcdefu64))
    }

    fn public_key() -> Felt {
        Felt::from_hex(&crate::sign_stark_hash(&key(), "0x1").unwrap().public_key).unwrap()
    }

    fn wallet_sign(hash: &Felt) -> Result<StarkSignature, SignRefusal> {
        Ok(crate::sign_stark_hash(&key(), &felt::canonical(hash)).unwrap())
    }

    // `call-set-signer.test.ts`, the signing half: exactly the digest, as a raw Stark pair.
    #[test]
    fn signs_exactly_the_digest_as_a_raw_stark_pair() {
        let hash = hash(0x1234, &[1, 2], b"TEST");
        let mut asked = None;
        let signature = sign_call_set(&hash, &public_key(), |digest| {
            asked = Some(*digest);
            wallet_sign(digest)
        })
        .unwrap();
        assert_eq!(asked, Some(hash));
        assert_eq!(signature.len(), 2);
        assert!(stark_signature_is_valid(
            &public_key(),
            &hash,
            &signature[0],
            &signature[1]
        ));
    }

    #[test]
    fn refuses_a_signature_over_another_hash_or_key() {
        let hash = Felt::from(0xabcu64);
        let other_hash = |_: &Felt| Ok(crate::sign_stark_hash(&key(), "0xabd").unwrap());
        assert_eq!(
            sign_call_set(&hash, &public_key(), other_hash),
            Err(ProofRefusal::SigningFailed)
        );
        let other_key = |_: &Felt| {
            Ok(crate::sign_stark_hash(&SecretFelt::new(Felt::from(0x77u64)), "0xabc").unwrap())
        };
        assert_eq!(
            sign_call_set(&hash, &public_key(), other_key),
            Err(ProofRefusal::SigningFailed)
        );
        assert_eq!(
            sign_call_set(&hash, &public_key(), |_| Err(SignRefusal::Failed)),
            Err(ProofRefusal::SigningFailed)
        );
        assert_eq!(
            sign_call_set(&hash, &public_key(), |_| Err(SignRefusal::Locked)),
            Err(ProofRefusal::Locked)
        );
    }
}
