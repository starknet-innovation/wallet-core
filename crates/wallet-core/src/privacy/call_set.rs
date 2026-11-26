//! The SNIP-12 `CallSet` the privacy pool verifies (the extension's `call-set-signer.ts`): the
//! digest over the calls the SDK asks to sign, recomputed here from the core's own account and
//! chain, and its signature, made by the wallet's signing path and checked before it is used.
//! The SDK's signer never sees a key; it is handed the signature this module returns.

use starknet_types_core::hash::{Poseidon, StarkHash};

use super::engine::{ProofLimits, ProofRefusal};
use crate::felt;
use crate::send::calls::Call;
use crate::send::types::SignRefusal;
use crate::{selector, stark_signature_is_valid, Felt, StarkSignature};

const STARKNET_DOMAIN_TYPE_HASH: Felt =
    Felt::from_hex_unchecked("0x1ff2f602e42168014d405a94f75e8a93d640751d71d16311266e140d8b0a210");
const CALL_TYPE_HASH: Felt =
    Felt::from_hex_unchecked("0x3635c7f2a7ba93844c0d064e18e487f35ab90f7c39d00f186a781fc3f0c2ca9");
const CALL_SET_TYPE_HASH: Felt =
    Felt::from_hex_unchecked("0x308b7462f924efba15fc992e6827eb3a748fcc79f091914156756437fa22909");

/// The one entrypoint the SDK's signer may be asked to sign for.
pub const COMPILE_ACTIONS: &str = "compile_actions";

fn hash_call(call: &Call) -> Felt {
    Poseidon::hash_array(&[
        CALL_TYPE_HASH,
        call.contract_address,
        selector(&call.entrypoint),
        Poseidon::hash_array(&call.calldata),
    ])
}

/// `computeStrk20CallSetHash(account, calls, chainId)`: the digest binding the account, the
/// chain and the calls, with no additional data (the SDK's signer passes none).
pub fn call_set_hash(account: &Felt, calls: &[Call], chain_id: &Felt) -> Felt {
    let domain = Poseidon::hash_array(&[
        STARKNET_DOMAIN_TYPE_HASH,
        Felt::from_bytes_be_slice(b"CallSet"),
        Felt::ONE,
        *chain_id,
        Felt::ONE,
    ]);
    let call_hashes: Vec<Felt> = calls.iter().map(hash_call).collect();
    let call_set = Poseidon::hash_array(&[
        CALL_SET_TYPE_HASH,
        Poseidon::hash_array(&call_hashes),
        Poseidon::hash_array(&[]),
    ]);
    Poseidon::hash_array(&[
        Felt::from_bytes_be_slice(b"StarkNet Message"),
        domain,
        *account,
        call_set,
    ])
}

/// Refuses calls to sign unless they are exactly one `compile_actions` on `pool` whose user
/// (its first argument) is `account`, within the calldata bound. The extension's single-signer
/// path signs whatever the SDK hands it; the core holds it to the one call the SDK makes
/// (PLAN.md, "P15 spike findings").
pub(super) fn bound_sign_request(
    calls: &[Call],
    pool: &Felt,
    account: &Felt,
    limits: &ProofLimits,
) -> Result<(), ProofRefusal> {
    match calls {
        [call]
            if call.contract_address == *pool
                && call.entrypoint == COMPILE_ACTIONS
                && call.calldata.len() <= limits.invoke_calldata_felts
                && call.calldata.first() == Some(account) =>
        {
            Ok(())
        }
        _ => Err(ProofRefusal::UnboundSignRequest),
    }
}

/// Signs `hash` through `sign` and returns the raw `[r, s]` the pool verifies. The signature
/// must come from `public_key` and verify over `hash`, so a wallet that signed something else,
/// or with another key, is refused before anything reaches the engine.
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
    use crate::privacy::config::testing::proof_limits;

    fn call(contract: u64, entrypoint: &str, calldata: &[u64]) -> Call {
        Call {
            contract_address: Felt::from(contract),
            entrypoint: entrypoint.into(),
            calldata: calldata.iter().copied().map(Felt::from).collect(),
        }
    }

    fn chain() -> Felt {
        Felt::from_bytes_be_slice(b"TEST")
    }

    /// `call-set-signer.test.ts`: the privacy protocol's cross-language golden vector. A drift
    /// here makes every private proof's authorization fail on chain.
    #[test]
    fn matches_the_privacy_contract_golden_vector() {
        let calls = [call(0x111, "approve", &[1, 2])];
        assert_eq!(
            call_set_hash(&Felt::from(0x1234u64), &calls, &chain()),
            Felt::from_hex_unchecked(
                "0x79d05f5b8993f5a0a18c6f7001e4d573c3eb97f322f6008cc1420f4f611501f"
            )
        );
    }

    #[test]
    fn binds_calls_account_and_chain() {
        let account = Felt::from(0x1234u64);
        let calls = [call(0x111, "approve", &[1, 2])];
        let baseline = call_set_hash(&account, &calls, &chain());
        assert_ne!(call_set_hash(&account, &[], &chain()), baseline);
        assert_ne!(
            call_set_hash(&Felt::from(0x999u64), &calls, &chain()),
            baseline
        );
        assert_ne!(
            call_set_hash(&account, &calls, &Felt::from_bytes_be_slice(b"OTHER")),
            baseline
        );
        let moved = [call(0x111, "approve", &[1, 3])];
        assert_ne!(call_set_hash(&account, &moved, &chain()), baseline);
    }

    #[test]
    fn only_the_pools_one_compile_actions_for_this_account_is_signed() {
        let (pool, account, limits) = (Felt::from(0x7u64), Felt::from(0x1234u64), proof_limits());
        let bound = |calls: &[Call]| bound_sign_request(calls, &pool, &account, &limits);
        let good = call(0x7, COMPILE_ACTIONS, &[0x1234, 0x99, 0]);
        assert_eq!(bound(std::slice::from_ref(&good)), Ok(()));
        for refused in [
            vec![],
            vec![good.clone(), good.clone()],
            vec![call(0x8, COMPILE_ACTIONS, &[0x1234, 0x99, 0])],
            vec![call(0x7, "apply_actions", &[0x1234, 0x99, 0])],
            vec![call(0x7, "approve", &[0x1234, 0x99, 0])],
            vec![call(0x7, COMPILE_ACTIONS, &[0x4321, 0x99, 0])],
            vec![call(0x7, COMPILE_ACTIONS, &[])],
        ] {
            assert_eq!(bound(&refused), Err(ProofRefusal::UnboundSignRequest));
        }
        let mut long = good;
        long.calldata
            .resize(limits.invoke_calldata_felts + 1, Felt::ZERO);
        assert_eq!(
            bound(std::slice::from_ref(&long)),
            Err(ProofRefusal::UnboundSignRequest)
        );
    }
}
