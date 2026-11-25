//! A stake approves funds to its pool, so `prepare` takes the pool only when a second reading
//! from another source (`PrepareContext::check`) verifies the same one: a lying node can list its
//! own contract and answer for it with matching parameters.

use super::calls::StakingAction;
use super::prepare::{prepare, PrepareContext, PrepareInput};
use super::prepare_tests::{account, context, reading, SEND};
use super::read::{PoolOutcome, StakingReading};
use super::test_chain::*;
use super::StakingRefusal;
use crate::Felt;

#[test]
fn a_stake_needs_a_second_reading_naming_the_same_pool() {
    let input = PrepareInput::new(StakingAction::Stake, Some("1"), STAKER).unwrap();
    let account = account();
    let token = strk_config();
    let honest = reading();
    let stake = |check: Option<&StakingReading>| {
        let context = PrepareContext {
            check,
            ..context(&account, &token, &SEND)
        };
        prepare(&input, reading().as_ref(), &context).map(|p| p.pool)
    };
    assert_eq!(stake(honest.as_ref()), Ok(f(STAKING_POOL)));
    // No finished second reading, or one for another account or contract: nothing to compare.
    assert_eq!(stake(None), Err(StakingRefusal::Unreadable));
    let mut elsewhere = honest.clone().unwrap();
    elsewhere.account = Felt::from(7u8);
    assert_eq!(stake(Some(&elsewhere)), Err(StakingRefusal::Unreadable));
    // A node that names its own contract as the pool, with parameters to match.
    let mut lying = honest.clone().unwrap();
    let PoolOutcome::Verified { pool, .. } = &mut lying.pools[0].outcome else {
        panic!("the honest reading verifies the pool");
    };
    pool.pool = Felt::from(0xbadu16);
    assert_eq!(stake(Some(&lying)), Err(StakingRefusal::Unverified));
    let mut refused = honest.clone().unwrap();
    refused.pools[0].outcome = PoolOutcome::Refused(StakingRefusal::NoPoolForToken);
    assert_eq!(stake(Some(&refused)), Err(StakingRefusal::Unverified));
    let mut no_facts = honest.clone().unwrap();
    no_facts.facts = None;
    assert_eq!(stake(Some(&no_facts)), Err(StakingRefusal::Unverified));
    // The same lie as the first reading, with the honest one as the check.
    let context = PrepareContext {
        check: honest.as_ref(),
        ..context(&account, &token, &SEND)
    };
    assert_eq!(
        prepare(&input, Some(&lying), &context).map(|p| p.pool),
        Err(StakingRefusal::Unverified)
    );
}
