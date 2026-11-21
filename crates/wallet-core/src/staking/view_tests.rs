//! The overview (`staking-ops.test.ts` "staking overview") and "Your stakes"
//! (`staking-positions-ops.test.ts`) over the extension's staking chain: rows come only from a
//! verified on-chain reading, and anything short of a whole, verified list is incomplete.

use super::calls::PositionState;
use super::positions::PositionsPage;
use super::read::{StakingPair, StakingReading};
use super::test_chain::*;
use super::view::*;
use super::{StakingPolicy, StakingRefusal};
use crate::portfolio::model::TokenConfig;
use crate::{Felt, NetworkId};

fn tokens() -> Vec<TokenConfig> {
    let sepolia = TokenConfig {
        id: "SN_SEPOLIA:strk".into(),
        network: NetworkId::Sepolia,
        ..strk_config()
    };
    vec![strk_config(), sepolia]
}

fn overview_of(chain: &mut Chain) -> Result<StakingOverview, StakingRefusal> {
    let pairs = overview_pairs(&f(STAKER), &tokens(), NetworkId::Mainnet);
    let reading = run(read_of(pairs), chain);
    overview(
        reading.as_ref(),
        &tokens(),
        &f(ACCOUNT),
        NetworkId::Mainnet,
        NOW,
    )
}

#[test]
fn a_row_takes_the_tracked_token_of_its_own_network() {
    // The same address tracked on Sepolia first must not name the Mainnet row.
    let mut reversed = tokens();
    reversed.reverse();
    let pairs = overview_pairs(&f(STAKER), &reversed, NetworkId::Mainnet);
    let reading = run(read_of(pairs), &mut chain());
    let view = overview(
        reading.as_ref(),
        &reversed,
        &f(ACCOUNT),
        NetworkId::Mainnet,
        NOW,
    );
    assert_eq!(view.unwrap().rows[0].token_id, "SN_MAIN:strk");
}

#[test]
fn overview_pairs_are_the_tracked_tokens_on_the_network() {
    assert_eq!(
        overview_pairs(&f(STAKER), &tokens(), NetworkId::Mainnet),
        [strk_pair()]
    );
}

#[test]
fn the_overview_lists_the_verified_pool_with_the_accounts_position_and_the_exit_window() {
    let mut chain = chain();
    chain.member = Some(Member {
        unclaimed: 25 * 10u128.pow(16),
        ..member()
    });
    let overview = overview_of(&mut chain).unwrap();
    assert_eq!(overview.exit_window_seconds, 604_800);
    assert_eq!(
        overview.rows,
        [StakingRow {
            staker: STAKER.into(),
            symbol: "STRK".into(),
            token_id: "SN_MAIN:strk".into(),
            figures: Ok(RowFigures {
                accepting: true,
                commission: "10%".into(),
                delegated: "1000".into(),
                rewards_to: None,
                staked: "5".into(),
                state: PositionState::Staked,
                unclaimed: "0.25".into(),
                unpool_amount: "0".into(),
                unpool_time: None,
            }),
        }]
    );
}

#[test]
fn a_staking_contract_paying_anything_but_strk_reads_as_unreadable() {
    let mut chain = chain();
    chain.member = Some(Member {
        unclaimed: 10u128.pow(18),
        ..member()
    });
    let rows = overview_of(&mut chain).unwrap().rows;
    assert_eq!(
        rows[0].figures.as_ref().map(|f| f.unclaimed.as_str()),
        Ok("1")
    );
    chain.reward_token = Felt::from(0x777u32);
    assert_eq!(overview_of(&mut chain), Err(StakingRefusal::Unreadable));
}

#[test]
fn the_overview_shows_a_refusal_not_figures_when_the_pool_names_another_staker() {
    let mut chain = chain();
    chain.pool_staker = Felt::from(0x999u32);
    let rows = overview_of(&mut chain).unwrap().rows;
    assert_eq!(rows[0].figures, Err(StakingRefusal::Unverified));
}

#[test]
fn the_overview_names_rewards_paid_elsewhere_and_leaves_out_inactive_tokens() {
    let mut chain = chain();
    chain.member = Some(Member {
        reward_address: Felt::from(0xabc123u32),
        unclaimed: 5,
        ..member()
    });
    let rows = overview_of(&mut chain).unwrap().rows;
    let rewards_to = rows[0].figures.as_ref().unwrap().rewards_to.clone();
    assert_eq!(rewards_to.as_deref(), Some("0xabc123"));
    chain.active_tokens = vec![Felt::from(0x777u32)];
    assert_eq!(overview_of(&mut chain).map(|o| o.rows.len()), Ok(0));
}

fn reading() -> StakingReading {
    let mut chain = chain();
    chain.member = Some(member());
    run(read_of(vec![strk_pair()]), &mut chain).unwrap()
}

#[test]
fn a_view_refuses_a_reading_made_for_another_account_or_network() {
    let reading = reading();
    let tokens = tokens();
    let account = f(ACCOUNT);
    assert!(overview(Some(&reading), &tokens, &account, NetworkId::Mainnet, NOW).is_ok());
    for (account, network) in [
        (Felt::from(7u8), NetworkId::Mainnet),
        (account, NetworkId::Sepolia),
    ] {
        assert_eq!(
            overview(Some(&reading), &tokens, &account, network, NOW),
            Err(StakingRefusal::Unreadable)
        );
        assert_eq!(
            positions_view(Some(&reading), &tokens, &account, network, NOW, true),
            Err(StakingRefusal::PositionsUnreadable)
        );
    }
    assert_eq!(
        overview(None, &tokens, &account, NetworkId::Mainnet, NOW),
        Err(StakingRefusal::Unreadable)
    );
}

fn page(stakers: &[Felt], complete: bool) -> PositionsPage {
    PositionsPage {
        complete,
        positions: stakers
            .iter()
            .map(|staker| StakingPair {
                staker: *staker,
                token: f(STRK),
            })
            .collect(),
    }
}

fn stakes(chain: &mut Chain, page: &PositionsPage) -> Result<PositionsView, StakingRefusal> {
    let (pairs, complete) = positions_pairs(page, &tokens(), NetworkId::Mainnet, &POLICY);
    let reading = run(read_of(pairs), chain);
    positions_view(
        reading.as_ref(),
        &tokens(),
        &f(ACCOUNT),
        NetworkId::Mainnet,
        NOW,
        complete,
    )
}

fn staked() -> Chain {
    let mut chain = chain();
    chain.member = Some(Member {
        unclaimed: 25 * 10u128.pow(16),
        ..member()
    });
    chain
}

#[test]
fn a_position_is_shown_only_once_the_chain_confirms_it_with_the_chains_figures() {
    let mut chain = staked();
    let view = stakes(&mut chain, &page(&[f(STAKER)], true)).unwrap();
    assert!(view.complete);
    let figures = view.rows[0].figures.as_ref().unwrap();
    assert_eq!(
        (figures.staked.as_str(), figures.unclaimed.as_str()),
        ("5", "0.25")
    );
    assert!(chain.reads.contains(&"get_pool_member_info_v1"));
}

#[test]
fn a_listed_position_the_chain_does_not_confirm_is_not_shown_as_a_stake() {
    let view = stakes(&mut chain(), &page(&[f(STAKER)], true)).unwrap();
    assert_eq!(
        view,
        PositionsView {
            complete: true,
            rows: vec![]
        }
    );
}

#[test]
fn a_pending_unstake_or_unclaimed_rewards_alone_is_still_a_position() {
    let mut chain = chain();
    chain.member = Some(Member {
        amount: 0,
        unpool_amount: 10u128.pow(18),
        unpool_time: Some(1_900_000_000),
        ..member()
    });
    let view = stakes(&mut chain, &page(&[f(STAKER)], true)).unwrap();
    let figures = view.rows[0].figures.as_ref().unwrap();
    assert_eq!(figures.state, PositionState::ExitPending);
    assert_eq!(figures.unpool_amount, "1");
    assert_eq!(figures.unpool_time, Some(1_900_000_000));
    chain.member = Some(Member {
        amount: 0,
        unclaimed: 1,
        ..member()
    });
    assert_eq!(
        stakes(&mut chain, &page(&[f(STAKER)], true)).map(|v| v.rows.len()),
        Ok(1)
    );
}

#[test]
fn an_unverifiable_candidate_is_not_shown_and_makes_the_list_incomplete() {
    let mut chain = staked();
    chain.pool_staker = Felt::from(0x999u32);
    let incomplete = Ok(PositionsView {
        complete: false,
        rows: vec![],
    });
    assert_eq!(stakes(&mut chain, &page(&[f(STAKER)], true)), incomplete);
    let unknown = page(&[Felt::from(0x777u32)], true);
    assert_eq!(stakes(&mut staked(), &unknown), incomplete);
}

#[test]
fn an_incomplete_or_truncated_page_is_incomplete_never_no_stakes() {
    let view = stakes(&mut staked(), &page(&[f(STAKER)], false)).unwrap();
    assert!(!view.complete);
    assert_eq!(view.rows.len(), 1);
    assert_eq!(
        stakes(&mut staked(), &page(&[], false)),
        Ok(PositionsView {
            complete: false,
            rows: vec![]
        })
    );
}

#[test]
fn a_token_the_account_does_not_track_or_the_contract_does_not_take_is_incomplete() {
    let untracked = PositionsPage {
        complete: true,
        positions: vec![StakingPair {
            staker: f(STAKER),
            token: Felt::from(0xb7cu32),
        }],
    };
    let (pairs, complete) = positions_pairs(&untracked, &tokens(), NetworkId::Mainnet, &POLICY);
    assert!(pairs.is_empty() && !complete);
    assert_eq!(
        stakes(&mut staked(), &untracked),
        Ok(PositionsView {
            complete: false,
            rows: vec![]
        })
    );
    let mut inactive = staked();
    inactive.active_tokens = vec![Felt::from(0x777u32)];
    let view = stakes(&mut inactive, &page(&[f(STAKER)], true)).unwrap();
    assert!(!view.complete && view.rows.is_empty());
}

#[test]
fn more_candidates_than_the_wallet_reads_is_incomplete() {
    let mut chain = staked();
    chain.extra_stakers = (0..16u32).map(|i| Felt::from(0x700 + i)).collect();
    let mut stakers = vec![f(STAKER)];
    stakers.extend(chain.extra_stakers.clone());
    let at_cap = stakes(&mut chain, &page(&stakers[..16], true)).unwrap();
    assert!(at_cap.complete);
    assert_eq!(at_cap.rows.len(), 16);
    chain.reads.clear();
    let over = stakes(&mut chain, &page(&stakers, true)).unwrap();
    assert!(!over.complete);
    assert_eq!(over.rows.len(), 16);
    let asked = chain
        .reads
        .iter()
        .filter(|r| **r == "staker_pool_info")
        .count();
    assert_eq!(asked, 16);
    // The cap is the policy's.
    let tight = StakingPolicy {
        max_checked_positions: 1,
        ..POLICY
    };
    let (pairs, complete) = positions_pairs(
        &page(&stakers[..2], true),
        &tokens(),
        NetworkId::Mainnet,
        &tight,
    );
    assert_eq!((pairs.len(), complete), (1, false));
}

#[test]
fn a_chain_that_cannot_be_read_is_unreadable() {
    let mut chain = staked();
    chain.reward_token = Felt::from(0x123u32);
    assert_eq!(
        stakes(&mut chain, &page(&[f(STAKER)], true)),
        Err(StakingRefusal::PositionsUnreadable)
    );
    assert_eq!(
        positions_view(None, &tokens(), &f(ACCOUNT), NetworkId::Mainnet, NOW, true),
        Err(StakingRefusal::PositionsUnreadable)
    );
}
