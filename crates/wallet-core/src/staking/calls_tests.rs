//! `staking-calls.test.ts` and `staking-review.test.ts`: exact-amount approvals, each action
//! allowed only in the state the pool accepts it in, and the sentences an action is reviewed
//! and recorded under.

use super::calls::*;
use super::decode::{PoolMember, VerifiedPool};
use super::review::review_sentence;
use super::StakingRefusal;
use crate::send::calls::Call;
use crate::send::u256::U256;
use crate::Felt;

const NOW: u64 = 1_000;

fn account() -> Felt {
    Felt::from(0xaccu32)
}

fn pool(accepting: bool) -> VerifiedPool {
    VerifiedPool {
        accepting,
        commission: 500,
        delegated: 0,
        pool: Felt::from(0xb001u32),
    }
}

fn member() -> PoolMember {
    PoolMember {
        amount: 10,
        reward_address: account(),
        unclaimed_rewards: 0,
        unpool_amount: 0,
        unpool_time: None,
    }
}

fn build(
    action: StakingAction,
    amount: Option<u128>,
    member: Option<PoolMember>,
    accepting: bool,
) -> Result<Vec<Call>, StakingRefusal> {
    build_staking_calls(&BuildInput {
        account: account(),
        action,
        amount: amount.map(U256::from_u128),
        member: member.as_ref(),
        now: NOW,
        pool: &pool(accepting),
        token: Felt::from(0x7e4u32),
    })
}

fn call(contract: u32, entrypoint: &str, calldata: &[Felt]) -> Call {
    Call {
        contract_address: Felt::from(contract),
        entrypoint: entrypoint.into(),
        calldata: calldata.to_vec(),
    }
}

fn exiting(at: u64) -> Option<PoolMember> {
    Some(PoolMember {
        unpool_time: Some(at),
        ..member()
    })
}

#[test]
fn position_state_follows_the_exit_window_with_the_injected_clock() {
    assert_eq!(position_state(None, NOW), PositionState::NotStaked);
    assert_eq!(position_state(Some(&member()), NOW), PositionState::Staked);
    let empty = PoolMember {
        amount: 0,
        ..member()
    };
    assert_eq!(position_state(Some(&empty), NOW), PositionState::NotStaked);
    let pending = exiting(NOW + 1).unwrap();
    assert_eq!(
        position_state(Some(&pending), NOW),
        PositionState::ExitPending
    );
    let due = exiting(NOW).unwrap();
    assert_eq!(position_state(Some(&due), NOW), PositionState::Withdrawable);
}

#[test]
fn a_first_stake_is_an_exact_approval_then_enter_delegation_pool() {
    let five = Felt::from(5u8);
    assert_eq!(
        build(StakingAction::Stake, Some(5), None, true),
        Ok(vec![
            call(0x7e4, "approve", &[Felt::from(0xb001u32), five, Felt::ZERO]),
            call(0xb001, "enter_delegation_pool", &[account(), five]),
        ])
    );
}

#[test]
fn a_members_stake_uses_add_to_delegation_pool() {
    let calls = build(StakingAction::Stake, Some(5), Some(member()), true).unwrap();
    assert_eq!(calls[1].entrypoint, "add_to_delegation_pool");
}

#[test]
fn stake_refusals() {
    for amount in [Some(0), None] {
        assert_eq!(
            build(StakingAction::Stake, amount, None, true),
            Err(StakingRefusal::StakeAmount)
        );
    }
    let over = build_staking_calls(&BuildInput {
        account: account(),
        action: StakingAction::Stake,
        amount: Some(U256 { high: 1, low: 5 }),
        member: None,
        now: NOW,
        pool: &pool(true),
        token: Felt::from(0x7e4u32),
    });
    assert_eq!(over, Err(StakingRefusal::StakeAmount), "2^128");
    assert!(build(StakingAction::Stake, Some(u128::MAX), None, true).is_ok());
    assert_eq!(
        build(StakingAction::Stake, Some(5), None, false),
        Err(StakingRefusal::Leaving)
    );
    // The pool accepts more stake while an exit is pending.
    let calls = build(StakingAction::Stake, Some(5), exiting(NOW + 5), true).unwrap();
    assert_eq!(calls[1].entrypoint, "add_to_delegation_pool");
}

#[test]
fn claim_unstake_cancel_and_withdraw() {
    let pool_address = 0xb001;
    let rewarded = Some(PoolMember {
        unclaimed_rewards: 3,
        ..member()
    });
    assert_eq!(
        build(StakingAction::Claim, None, rewarded, true),
        Ok(vec![call(pool_address, "claim_rewards", &[account()])])
    );
    assert_eq!(
        build(StakingAction::Claim, None, Some(member()), true),
        Err(StakingRefusal::NoRewards)
    );
    assert_eq!(
        build(StakingAction::Claim, None, None, true),
        Err(StakingRefusal::NoRewards)
    );
    assert_eq!(
        build(StakingAction::Unstake, Some(10), Some(member()), true),
        Ok(vec![call(
            pool_address,
            "exit_delegation_pool_intent",
            &[Felt::from(10u8)]
        )])
    );
    assert_eq!(
        build(StakingAction::Unstake, Some(11), Some(member()), true),
        Err(StakingRefusal::MoreThanStaked)
    );
    assert_eq!(
        build(StakingAction::Unstake, Some(0), Some(member()), true),
        Err(StakingRefusal::UnstakeAmount)
    );
    assert_eq!(
        build(StakingAction::Unstake, Some(1), None, true),
        Err(StakingRefusal::NothingStaked)
    );
    for pending in [exiting(NOW + 5), exiting(NOW)] {
        assert_eq!(
            build(StakingAction::Unstake, Some(1), pending, true),
            Err(StakingRefusal::PendingExit)
        );
        assert_eq!(
            build(StakingAction::CancelUnstake, None, pending, true),
            Ok(vec![call(
                pool_address,
                "exit_delegation_pool_intent",
                &[Felt::ZERO]
            )])
        );
    }
    assert_eq!(
        build(StakingAction::CancelUnstake, None, Some(member()), true),
        Err(StakingRefusal::NoUnstake)
    );
    assert_eq!(
        build(StakingAction::Withdraw, None, exiting(NOW), true),
        Ok(vec![call(
            pool_address,
            "exit_delegation_pool_action",
            &[account()]
        )])
    );
    assert_eq!(
        build(StakingAction::Withdraw, None, exiting(NOW + 1), true),
        Err(StakingRefusal::NotWithdrawableYet)
    );
    assert_eq!(
        build(StakingAction::Withdraw, None, Some(member()), true),
        Err(StakingRefusal::NothingToWithdraw)
    );
}

#[test]
fn activity_summaries_name_the_verb_the_activity_classifier_reads() {
    let summary = |action, amount| activity_summary(action, amount, "STRK", "Val");
    assert_eq!(
        summary(StakingAction::Stake, Some("5")),
        "Stake 5 STRK with Val"
    );
    assert_eq!(
        summary(StakingAction::Claim, None),
        "Claim staking rewards from Val"
    );
    assert_eq!(
        summary(StakingAction::Unstake, Some("5")),
        "Unstake 5 STRK from Val"
    );
    assert_eq!(
        summary(StakingAction::CancelUnstake, None),
        "Keep STRK staked with Val"
    );
    assert_eq!(
        summary(StakingAction::Withdraw, None),
        "Withdraw unstaked STRK from Val"
    );
}

fn rewarded_to(reward: Felt) -> PoolMember {
    PoolMember {
        amount: 1,
        reward_address: reward,
        unclaimed_rewards: 1,
        ..member()
    }
}

fn sentence(action: StakingAction, amount: Option<&str>, member: Option<&PoolMember>) -> String {
    review_sentence(&account(), action, amount, member, "STRK", "Val")
}

#[test]
fn the_review_states_the_exact_approval_and_the_unstake_wait() {
    assert_eq!(
        sentence(StakingAction::Stake, Some("5"), None),
        "Stake 5 STRK with Val. This approves exactly 5 STRK for its pool."
    );
    let own = rewarded_to(account());
    assert_eq!(
        sentence(StakingAction::Unstake, Some("5"), Some(&own)),
        "Unstake 5 STRK from Val. It earns no rewards until you withdraw it."
    );
}

#[test]
fn a_claim_names_strk_whatever_the_staked_token() {
    let own = rewarded_to(account());
    assert_eq!(
        review_sentence(
            &account(),
            StakingAction::Claim,
            None,
            Some(&own),
            "WBTC",
            "Val"
        ),
        "Claim your STRK rewards from Val."
    );
}

#[test]
fn the_review_names_a_reward_address_that_is_not_this_account() {
    let elsewhere = rewarded_to(
        crate::felt::parse("0x0000beef00000000000000000000000000000000000000000000000000000001")
            .unwrap(),
    );
    assert_eq!(
        sentence(StakingAction::Claim, None, Some(&elsewhere)),
        "Claim your STRK rewards from Val. Rewards are paid to 0x0000…0001, not this account."
    );
    assert!(sentence(StakingAction::Stake, Some("5"), Some(&elsewhere))
        .ends_with("Rewards are paid to 0x0000…0001, not this account."));
    assert_eq!(
        sentence(StakingAction::Withdraw, None, Some(&elsewhere)),
        "Withdraw your unstaked STRK from Val."
    );
    assert_eq!(
        sentence(StakingAction::Unstake, Some("1"), Some(&elsewhere)),
        "Unstake 1 STRK from Val. It earns no rewards until you withdraw it."
    );
    let own = rewarded_to(account());
    assert_eq!(
        sentence(StakingAction::Claim, None, Some(&own)),
        "Claim your STRK rewards from Val."
    );
    assert_eq!(
        sentence(StakingAction::CancelUnstake, None, Some(&own)),
        "Cancel your unstake and keep the STRK staked with Val."
    );
}
