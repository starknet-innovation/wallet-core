//! `staking-decode.test.ts`: the decoders pinned to live Mainnet answers (read by the extension
//! on 2026-11-04 and kept verbatim), their refusals, and the pool verification rule.

use super::decode::*;
use super::StakingRefusal;
use crate::{felt, Felt};

const STAKER: &str = "0x36963c7b56f08105ffdd7f12560924bdc0cb29ce210417ecbc8bf3c7e4b9090";
const STAKING: &str = "0xca1702e64c81d9a07b86bd2c540188d92a2c73cf5cc0e508d949015e7e84a7";
const STRK: &str = "0x4718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d";
const STRK_POOL: &str = "0x362dc7da60bfddc8e3146028dfd94941c6e22403c98b5947104e637543b475d";

fn f(value: &str) -> Felt {
    felt::parse(value).unwrap()
}

fn v(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| s.to_string()).collect()
}

fn live_staker_pool_info() -> Vec<String> {
    v(&[
        "0x0",
        "0x3e8",
        "0x5",
        STRK_POOL,
        STRK,
        "0x624edf011c0d20f91b6bf4",
        "0x2ea6a4ffce4c5e779451e043a9a24c9c0b0f784d9cc0f393bc1c801e3f8f2e3",
        "0x3fe2b97c1fd336e750087d68b9b867997fd64a2661ff3ca5a7c771641e8e7ac",
        "0x1c0f10e1",
        "0x750b4ecd1eef45cc74db3538277a869ced288f813e2faa00e0a5ae46243bbaa",
        "0x4daa17763b286d1e59b97c283c0b8c949994c361e426a28f743c67bdfe9a32f",
        "0x6cc451630eaf",
        "0x262f5a1240bf2d61ca36c4eb3a57ff4ae0a183046d3dcab35d0880b7d1776dd",
        "0x593e034dda23eea82d2ba9a30960ed42cf4a01502cc2351dc9b9881f9931a68",
        "0x1e78f5074069ea",
        "0x7ce7161e46e796e6034e3ead311d431567654116f3cffacefe37a2e17464a8f",
        "0x36834a40984312f7f7de8d31e3f6305b325389eaeea5b1c0664b2fb936461a4",
        "0x79e",
    ])
}

fn live_staker_info() -> Vec<String> {
    v(&[
        "0x0",
        STAKER,
        "0x54f4784af6820c8feef75e87ede8c5dd53a65f33bcbe6714b5684448650a7db",
        "0x1",
        "0x43c33c1937564800000",
        "0x4ea0a66f000d7c59fcd",
        "0x0",
        STRK_POOL,
        "0x624edf011c0d20f91b6bf4",
        "0x3e8",
    ])
}

#[test]
fn staker_pool_info_commission_and_one_pool_per_token() {
    let info = decode_staker_pool_info(&live_staker_pool_info()).unwrap();
    assert_eq!(info.commission, Some(1000));
    assert_eq!(info.pools.len(), 5);
    assert_eq!(
        info.pools[0],
        StakerPool {
            amount: 0x624edf011c0d20f91b6bf4,
            pool: f(STRK_POOL),
            token: f(STRK),
        }
    );
    // No commission set: `None` and the span straight after.
    let unset = decode_staker_pool_info(&v(&["0x1", "0x1", "0xb", "0xc", "0x5"])).unwrap();
    assert_eq!(unset.commission, None);
    assert_eq!(unset.pools[0].pool, Felt::from(0xbu8));
}

#[test]
fn staker_info_an_active_staker() {
    assert_eq!(
        decode_staker_status(&live_staker_info()),
        Some(StakerStatus::Active)
    );
}

#[test]
fn pool_member_with_a_pending_exit() {
    // Pool 0x362dc7da…475d, member 0x7f278354…6748.
    let reward = "0x7f278354a8ca237b1e1f2276b5c11b8886ed1b8d6a09e12ad019d583f3d6748";
    let live = v(&[
        "0x0",
        reward,
        "0x0",
        "0x21093ea939d0f8",
        "0x3e8",
        "0xde0b6b3a7640000",
        "0x0",
        "0x6af4388e",
    ]);
    assert_eq!(
        decode_pool_member(&live),
        Some(Some(PoolMember {
            amount: 0,
            reward_address: f(reward),
            unclaimed_rewards: 0x21093ea939d0f8,
            unpool_amount: 10u128.pow(18),
            unpool_time: Some(0x6af4388e),
        }))
    );
}

#[test]
fn staking_parameters_the_seven_day_exit_window() {
    let live = v(&[
        "0x43c33c1937564800000",
        STRK,
        "0x10398fe631af9ab2311840432d507bf7ef4b959ae967f1507928f5afe888a99",
        "0x69565cd9bd273116d8829ba461298a04bfe09adb0c4ac470e7bb66bcae84ab5",
        "0x9035556d1ee136e7722ae4e78f92828553a45eed3bc9b2aba90788ec2ca112",
        "0x93a80",
    ]);
    assert_eq!(decode_exit_window(&live), Some(604_800));
    assert_eq!(decode_exit_window(&live[..5]), None);
    let mut trailing = live.clone();
    trailing.push("0x0".into());
    assert_eq!(decode_exit_window(&trailing), None);
    let mut big = live.clone();
    big[5] = "0x10000000000000000".into();
    assert_eq!(decode_exit_window(&big), None);
}

#[test]
fn a_pool_span_whose_length_disagrees_with_its_count() {
    let info = live_staker_pool_info();
    assert_eq!(decode_staker_pool_info(&info[..info.len() - 1]), None);
    let mut trailing = info.clone();
    trailing.push("0x0".into());
    assert_eq!(decode_staker_pool_info(&trailing), None);
    assert_eq!(decode_staker_pool_info(&v(&["0x2", "0x0"])), None);
    // A count at the span bound, and a zero pool or token, are refused.
    assert_eq!(decode_staker_pool_info(&v(&["0x1", "0x40"])), None);
    assert_eq!(
        decode_staker_pool_info(&v(&["0x1", "0x1", "0x0", "0xc", "0x5"])),
        None
    );
    assert_eq!(
        decode_staker_pool_info(&v(&["0x1", "0x1", "0xb", "0x0", "0x5"])),
        None
    );
    let over_u128 = format!("0x1{}", "0".repeat(32));
    assert_eq!(
        decode_staker_pool_info(&v(&["0x1", "0x1", "0xb", "0xc", &over_u128])),
        None
    );
}

#[test]
fn a_commission_above_u16_or_a_non_felt() {
    assert_eq!(
        decode_staker_pool_info(&v(&["0x0", "0x10000", "0x0"])),
        None
    );
    assert_eq!(decode_staker_pool_info(&v(&["0x0", "1000", "0x0"])), None);
    assert_eq!(
        decode_staker_pool_info(&v(&["0x0", "0xffff", "0x0"])).map(|i| i.commission),
        Some(Some(0xffff))
    );
}

#[test]
fn a_staker_that_is_leaving_unknown_or_malformed() {
    let mut leaving = live_staker_info();
    leaving.splice(3..4, v(&["0x0", "0x6a000000"]));
    assert_eq!(decode_staker_status(&leaving), Some(StakerStatus::Leaving));
    assert_eq!(
        decode_staker_status(&v(&["0x1"])),
        Some(StakerStatus::Unknown)
    );
    assert_eq!(decode_staker_status(&v(&["0x1", "0x2"])), None);
    assert_eq!(decode_staker_status(&v(&["0x2"])), None);
    // A zero staker, an unknown `unstake_time` variant, or a time past u64.
    let mut zero = live_staker_info();
    zero[1] = "0x0".into();
    assert_eq!(decode_staker_status(&zero), None);
    let mut variant = live_staker_info();
    variant[3] = "0x2".into();
    assert_eq!(decode_staker_status(&variant), None);
    let mut late = leaving.clone();
    late[4] = "0x10000000000000000".into();
    assert_eq!(decode_staker_status(&late), None);
}

#[test]
fn pool_membership_none_pending_exit_and_trailing_data() {
    assert_eq!(decode_pool_member(&v(&["0x1"])), Some(None));
    assert_eq!(decode_pool_member(&v(&["0x1", "0x0"])), None);
    assert_eq!(
        decode_pool_member(&v(&[
            "0x0", "0xa", "0x5", "0x1", "0x3e8", "0x2", "0x0", "0x64"
        ])),
        Some(Some(PoolMember {
            amount: 5,
            reward_address: Felt::from(0xau8),
            unclaimed_rewards: 1,
            unpool_amount: 2,
            unpool_time: Some(100),
        }))
    );
    assert_eq!(
        decode_pool_member(&v(&[
            "0x0", "0xa", "0x5", "0x1", "0x3e8", "0x2", "0x1", "0x64"
        ])),
        None
    );
    assert_eq!(
        decode_pool_member(&v(&["0x0", "0x0", "0x5", "0x1", "0x3e8", "0x2", "0x1"])),
        None
    );
    // No exit pending, with exactly seven fields.
    assert_eq!(
        decode_pool_member(&v(&["0x0", "0xa", "0x5", "0x1", "0x3e8", "0x2", "0x1"]))
            .flatten()
            .map(|m| m.unpool_time),
        Some(None)
    );
    // The commission slot is bounded too, though nothing reads it.
    assert_eq!(
        decode_pool_member(&v(&["0x0", "0xa", "0x5", "0x1", "0x10000", "0x2", "0x1"])),
        None
    );
}

#[test]
fn pool_parameters_and_active_tokens() {
    assert_eq!(
        decode_pool_parameters(&v(&[STAKER, "0x2", STAKING, STRK, "0x0"])),
        None
    );
    assert_eq!(
        decode_pool_parameters(&v(&[STAKER, "0x0", STAKING, STRK])),
        None
    );
    assert_eq!(
        decode_pool_parameters(&v(&[STAKER, "0x0", STAKING, STRK, "0x0", "0x0"])),
        None
    );
    assert_eq!(
        decode_pool_parameters(&v(&[STAKER, "0x1", STAKING, STRK, "0x3e8"])),
        Some(PoolParameters {
            commission: 1000,
            staker: f(STAKER),
            staker_removed: true,
            staking_contract: f(STAKING),
            token: f(STRK),
        })
    );
    assert_eq!(
        decode_active_tokens(&v(&["0x1", STRK])),
        Some(vec![f(STRK)])
    );
    assert_eq!(decode_active_tokens(&v(&["0x2", STRK])), None);
    assert_eq!(decode_active_tokens(&v(&["0x1", STRK, STRK])), None);
    assert_eq!(decode_active_tokens(&v(&["0x1", "0x0"])), None);
    // An entry that is not a felt in the extension's grammar.
    assert_eq!(decode_active_tokens(&v(&["0x1", "0xABC"])), None);
    assert_eq!(decode_active_tokens(&v(&["0x1", ""])), None);
}

fn parameters() -> PoolParameters {
    PoolParameters {
        commission: 1000,
        staker: f(STAKER),
        staker_removed: false,
        staking_contract: f(STAKING),
        token: f(STRK),
    }
}

fn reads(parameters: PoolParameters, status: StakerStatus) -> StakingPoolReads {
    StakingPoolReads {
        parameters: Some(parameters),
        pool_info: decode_staker_pool_info(&live_staker_pool_info()),
        status: Some(status),
    }
}

fn verify(reads: &StakingPoolReads, token: &str) -> Result<VerifiedPool, StakingRefusal> {
    verify_staking_pool(reads, &f(STAKER), &f(STAKING), &f(token))
}

#[test]
fn accepts_the_pool_the_staking_contract_lists_and_the_pool_confirms() {
    assert_eq!(
        verify(&reads(parameters(), StakerStatus::Active), STRK),
        Ok(VerifiedPool {
            accepting: true,
            commission: 1000,
            delegated: 0x624edf011c0d20f91b6bf4,
            pool: f(STRK_POOL),
        })
    );
    // The pool's own commission stands in when the staker set none.
    let mut unset = reads(parameters(), StakerStatus::Active);
    if let Some(info) = unset.pool_info.as_mut() {
        info.commission = None;
    }
    let pool_commission = PoolParameters {
        commission: 250,
        ..parameters()
    };
    unset.parameters = Some(pool_commission);
    assert_eq!(verify(&unset, STRK).map(|p| p.commission), Ok(250));
}

#[test]
fn refuses_a_pool_that_names_another_staker_staking_contract_or_token() {
    for changed in [
        PoolParameters {
            staker: Felt::ONE,
            ..parameters()
        },
        PoolParameters {
            staking_contract: Felt::ONE,
            ..parameters()
        },
        PoolParameters {
            token: Felt::ONE,
            ..parameters()
        },
    ] {
        assert_eq!(
            verify(&reads(changed, StakerStatus::Active), STRK),
            Err(StakingRefusal::Unverified)
        );
    }
    let mut missing = reads(parameters(), StakerStatus::Active);
    missing.parameters = None;
    assert_eq!(verify(&missing, STRK), Err(StakingRefusal::Unverified));
}

#[test]
fn refuses_a_token_without_a_pool_and_an_unreadable_staker() {
    assert_eq!(
        verify(&reads(parameters(), StakerStatus::Active), "0x1"),
        Err(StakingRefusal::NoPoolForToken)
    );
    assert_eq!(
        verify(&reads(parameters(), StakerStatus::Unknown), STRK),
        Err(StakingRefusal::Unverified)
    );
    let mut no_info = reads(parameters(), StakerStatus::Active);
    no_info.pool_info = None;
    assert_eq!(verify(&no_info, STRK), Err(StakingRefusal::Unverified));
    let mut no_status = reads(parameters(), StakerStatus::Active);
    no_status.status = None;
    assert_eq!(verify(&no_status, STRK), Err(StakingRefusal::Unverified));
}

#[test]
fn a_leaving_or_removed_validator_stays_verified_but_takes_no_new_stake() {
    assert_eq!(
        verify(&reads(parameters(), StakerStatus::Leaving), STRK).map(|p| p.accepting),
        Ok(false)
    );
    let removed = PoolParameters {
        staker_removed: true,
        ..parameters()
    };
    assert_eq!(
        verify(&reads(removed, StakerStatus::Active), STRK).map(|p| p.accepting),
        Ok(false)
    );
}
