//! `staking-preparations.ts` through `staking-ops.test.ts`: a preparation confirms once, only on
//! the account and network it was reviewed on, only while staking is on, and the oldest is
//! dropped past the cap.

use super::calls::StakingAction;
use super::prepare::StakingPreparations;
use super::prepare_tests::act;
use super::test_chain::*;
use super::{StakingNetwork, StakingPolicy, StakingRefusal};
use crate::{Felt, NetworkId};

/// Mainnet and Sepolia both on, for the network binding.
const BOTH: [StakingNetwork; 2] = [
    NETWORK,
    StakingNetwork {
        network: NetworkId::Sepolia,
        ..NETWORK
    },
];

#[test]
fn a_preparation_confirms_once_and_only_on_the_account_and_network_it_was_reviewed_on() {
    let policy = StakingPolicy {
        networks: &BOTH,
        ..POLICY
    };
    let mut store = StakingPreparations::default();
    let prepared = || act(&mut chain(), StakingAction::Stake, Some("5")).unwrap();
    let id = store.put(prepared(), &policy);
    let account = f(ACCOUNT);
    assert!(store
        .claim(id, &account, NetworkId::Mainnet, &policy)
        .is_ok());
    let again = Err(StakingRefusal::ReviewAgain);
    assert_eq!(
        store.claim(id, &account, NetworkId::Mainnet, &policy),
        again
    );
    assert_eq!(
        store.claim(id + 99, &account, NetworkId::Mainnet, &policy),
        again
    );
    // Another account or network: refused, and the preparation is spent either way.
    let other = store.put(prepared(), &policy);
    assert_eq!(
        store.claim(other, &Felt::from(7u8), NetworkId::Mainnet, &policy),
        again
    );
    assert_eq!(
        store.claim(other, &account, NetworkId::Mainnet, &policy),
        again
    );
    let sepolia = store.put(prepared(), &policy);
    assert_eq!(
        store.claim(sepolia, &account, NetworkId::Sepolia, &policy),
        again
    );
    assert_eq!(
        store.claim(sepolia, &account, NetworkId::Mainnet, &policy),
        again
    );
}

#[test]
fn a_preparation_is_refused_once_staking_is_switched_off() {
    let mut store = StakingPreparations::default();
    let id = store.put(
        act(&mut chain(), StakingAction::Stake, Some("5")).unwrap(),
        &POLICY,
    );
    let off = StakingPolicy {
        networks: &[],
        ..POLICY
    };
    let account = f(ACCOUNT);
    assert_eq!(
        store.claim(id, &account, NetworkId::Mainnet, &off),
        Err(StakingRefusal::Unavailable)
    );
    assert_eq!(
        store.claim(id, &account, NetworkId::Mainnet, &POLICY),
        Err(StakingRefusal::ReviewAgain)
    );
}

#[test]
fn the_oldest_preparation_is_dropped_past_the_policys_cap() {
    let small = StakingPolicy {
        max_preparations: 2,
        ..POLICY
    };
    let mut store = StakingPreparations::default();
    let ids: Vec<u64> = (0..3)
        .map(|_| {
            store.put(
                act(&mut chain(), StakingAction::Stake, Some("5")).unwrap(),
                &small,
            )
        })
        .collect();
    let account = f(ACCOUNT);
    assert_eq!(
        store.claim(ids[0], &account, NetworkId::Mainnet, &small),
        Err(StakingRefusal::ReviewAgain)
    );
    for id in &ids[1..] {
        assert!(store
            .claim(*id, &account, NetworkId::Mainnet, &small)
            .is_ok());
    }
}
