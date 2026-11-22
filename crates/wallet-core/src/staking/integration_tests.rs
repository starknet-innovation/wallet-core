//! A staking action through the batch review and the allowance log (`assetsFor` and
//! `recordStakingSubmission` in `staking-action.ts`): the verified pool is pinned for the review,
//! the stake's exact-amount approval reaches the plan's pending record, and the action names its
//! own activity row.

use super::calls::StakingAction;
use super::prepare::{PrepareInput, StakingPreparation};
use super::prepare_tests::{account, act_in, Keccak, SEND};
use super::test_chain::*;
use crate::allowances::calls::{AllowanceChange, ChangeKind};
use crate::allowances::record::AllowanceInput;
use crate::catalog::TokenInfo;
use crate::send::call_decode::Known;
use crate::send::invoke::{ActivityAsset, Invoke};
use crate::send::plan::PendingKind;
use crate::send::review_flags::{ReviewContext, RiskFlag};
use crate::send::u256::U256;
use crate::{felt, NetworkId};

/// The wallet trusts `tokens` and nothing else; no contract is pinned.
fn trusting(tokens: &[TokenInfo]) -> ReviewContext<'_> {
    ReviewContext {
        known: Known {
            tokens,
            nft_contracts: &[],
        },
        ..ReviewContext::default()
    }
}

fn prepared(
    chain: &mut Chain,
    action: StakingAction,
    amount: Option<&str>,
    review: &ReviewContext,
) -> StakingPreparation {
    let input = PrepareInput::new(action, amount, STAKER).unwrap();
    act_in(chain, &input, &strk_config(), &SEND, review).unwrap()
}

fn pending(invoke: &Invoke) -> (Option<ActivityAsset>, Vec<AllowanceChange>) {
    let PendingKind::Invoke { asset, allowances } = invoke.pending_kind() else {
        panic!("a staking action is an invoke");
    };
    (asset, allowances)
}

fn codes(invoke: &Invoke) -> Vec<&'static str> {
    invoke.flags().iter().map(|flag| flag.code()).collect()
}

#[test]
fn a_stake_carries_its_exact_approval_of_the_pool_and_names_its_row() {
    let tokens = [strk_info()];
    let stake = prepared(
        &mut chain(),
        StakingAction::Stake,
        Some("5"),
        &trusting(&tokens),
    );
    let (asset, allowances) = pending(&stake.invoke);
    assert_eq!(
        asset,
        Some(ActivityAsset {
            amount: Some("5".into()),
            symbol: "STRK".into(),
            token_address: f(STRK),
        })
    );
    assert_eq!(
        allowances,
        [AllowanceChange {
            kind: ChangeKind::Set,
            input: AllowanceInput {
                account_id: format!("SN_MAIN:{}", felt::canonical(&f(ACCOUNT))),
                network: NetworkId::Mainnet,
                token: felt::canonical(&f(STRK)),
                spender: felt::canonical(&f(STAKING_POOL)),
                amount: U256::from_u128(5 * 10u128.pow(18)),
                lower_bound: false,
            },
        }]
    );
    // The row is the staking line, not the review's "Approve ..." line.
    assert_eq!(
        stake.invoke.activity_summary(),
        "Stake 5 STRK with validator 0x5a20…beef"
    );
    assert_ne!(stake.invoke.activity_summary(), stake.invoke.summary());
}

#[test]
fn only_a_stake_grants_and_only_stake_and_unstake_name_an_amount() {
    let review = trusting(&[]);
    let staked = || {
        let mut chain = chain();
        chain.member = Some(member());
        chain
    };
    let exiting = |time| {
        let mut chain = chain();
        chain.member = Some(Member {
            amount: 0,
            unpool_amount: 5,
            unpool_time: Some(time),
            ..member()
        });
        chain
    };
    let mut rewarded = chain();
    rewarded.member = Some(Member {
        unclaimed: 5,
        ..member()
    });
    let cases = [
        (rewarded, StakingAction::Claim, None),
        (staked(), StakingAction::Unstake, Some("2")),
        (exiting(NOW + 60), StakingAction::CancelUnstake, None),
        (exiting(NOW), StakingAction::Withdraw, None),
    ];
    for (mut chain, action, amount) in cases {
        let preparation = prepared(&mut chain, action, amount, &review);
        let (asset, allowances) = pending(&preparation.invoke);
        assert_eq!(allowances, [], "{action:?}");
        let asset = asset.unwrap();
        assert_eq!(asset.amount.as_deref(), amount, "{action:?}");
        assert_eq!(asset.symbol, "STRK", "{action:?}");
        assert_eq!(asset.token_address, f(STRK), "{action:?}");
    }
}

#[test]
fn the_verified_pool_is_pinned_for_the_review() {
    let tokens = [strk_info()];
    let review = trusting(&tokens);
    let stake = prepared(&mut chain(), StakingAction::Stake, Some("5"), &review);
    assert_eq!(codes(&stake.invoke), Vec::<&str>::new());
    // The same calls reviewed without the pin warn about the pool.
    let unpinned = Invoke::reviewed(
        &account(),
        stake.invoke.calls.clone(),
        &SEND,
        &review,
        &Keccak,
    )
    .unwrap();
    assert_eq!(
        unpinned.flags(),
        [&RiskFlag::UnknownContract {
            contract: f(STAKING_POOL)
        }]
    );
    // A pin the caller already holds is kept beside the pool's: here the untrusted token's.
    let token = [f(STRK)];
    let pinned = ReviewContext {
        known_contracts: &token,
        ..trusting(&[])
    };
    let stake = prepared(&mut chain(), StakingAction::Stake, Some("5"), &pinned);
    assert_eq!(codes(&stake.invoke), Vec::<&str>::new());
}

#[test]
fn a_stake_of_an_untrusted_token_warns_only_that_the_token_is_unknown() {
    // `verifyCalls(calls, { tokens: [], knownContracts: [pool] })`: the approve's contract is no
    // trusted token, the pool is pinned, the exact amount is far below the unlimited threshold,
    // and neither call pays anyone.
    let stake = prepared(
        &mut chain(),
        StakingAction::Stake,
        Some("5"),
        &trusting(&[]),
    );
    assert_eq!(
        stake.invoke.flags(),
        [&RiskFlag::UnknownContract { contract: f(STRK) }]
    );
    assert_eq!(stake.invoke.calls()[1].flags, []);
    // The approval is still filed: the log lists what the wallet cannot name, to revoke it.
    assert_eq!(pending(&stake.invoke).1.len(), 1);
}
