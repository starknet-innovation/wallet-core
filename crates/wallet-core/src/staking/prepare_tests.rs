//! `staking-ops.test.ts` "staking actions" and its malformed requests, over the extension's
//! staking chain: the pool is verified before anything is built, a stake approves exactly what
//! it stakes, and a preparation confirms once, only on the account and network it was reviewed
//! on.

use super::calls::StakingAction;
use super::prepare::*;
use super::read::StakingReading;
use super::test_chain::*;
use super::{StakingNetwork, StakingRefusal};
use crate::portfolio::model::TokenConfig;
use crate::send::call_decode::Known;
use crate::send::calls::Call;
use crate::send::fees::Headroom;
use crate::send::invoke::{ActivityAsset, InvokeRefusal};
use crate::send::review_flags::ReviewContext;
use crate::send::types::{Account, Authorization, Policy};
use crate::{Felt, NetworkId};

/// Bramble's `SEND_POLICY` numbers.
pub(super) const SEND: Policy = Policy {
    fee_margin_percent: 50,
    reviewed_fee_headroom: Headroom {
        numerator: 3,
        denominator: 2,
    },
    confirmation_delays_ms: &[0],
    max_invoke_calls: 16,
    max_call_calldata_felts: 4_096,
};

pub(super) fn account() -> Account {
    Account {
        network: NetworkId::Mainnet,
        address: f(ACCOUNT),
        public_key: Felt::from(3u8),
        authorization: Authorization::StarkPair,
        deployment: None,
    }
}

const VALIDATOR: &str =
    "validator 0x05a200000000000000000000000000000000000000000000000000000000beef";

/// A wallet that knows nothing on the network: no token, contact or pinned contract.
const NOTHING_KNOWN: ReviewContext<'static> = ReviewContext {
    known: Known {
        tokens: &[],
        nft_contracts: &[],
    },
    trusted: &[],
    known_contracts: &[],
    token_contracts: &[],
    standing: &[],
};

fn act_with(
    chain: &mut Chain,
    input: &PrepareInput,
    token: &TokenConfig,
    policy: &Policy,
) -> Result<StakingPreparation, StakingRefusal> {
    act_in(chain, input, token, policy, &NOTHING_KNOWN)
}

/// Reads and prepares one action from `input` against `chain`, as `prepareStakingResponse`,
/// reviewed with what the wallet knows in `review`.
pub(super) fn act_in(
    chain: &mut Chain,
    input: &PrepareInput,
    token: &TokenConfig,
    policy: &Policy,
    review: &ReviewContext,
) -> Result<StakingPreparation, StakingRefusal> {
    let account = account();
    let read = input.read(&NETWORK, &strk_info(), &account, token)?;
    let reading = run(read, chain);
    let context = PrepareContext {
        review,
        ..context(&account, token, policy)
    };
    prepare(input, reading.as_ref(), &context)
}

fn context<'a>(
    account: &'a Account,
    token: &'a TokenConfig,
    policy: &'a Policy,
) -> PrepareContext<'a> {
    PrepareContext {
        account,
        staking: &POLICY,
        token,
        now: NOW,
        policy,
        review: &NOTHING_KNOWN,
    }
}

pub(super) fn act(
    chain: &mut Chain,
    action: StakingAction,
    amount: Option<&str>,
) -> Result<StakingPreparation, StakingRefusal> {
    let input = PrepareInput::new(action, amount, STAKER).unwrap();
    act_with(chain, &input, &strk_config(), &SEND)
}

fn members_chain() -> Chain {
    let mut chain = chain();
    chain.member = Some(member());
    chain
}

#[test]
fn a_first_stake_approves_exactly_the_amount_and_enters_the_verified_pool() {
    let preparation = act(&mut chain(), StakingAction::Stake, Some("5")).unwrap();
    assert_eq!(
        preparation.review,
        format!("Stake 5 STRK with {VALIDATOR}. This approves exactly 5 STRK for its pool.")
    );
    assert_eq!(
        preparation.invoke.activity_summary(),
        "Stake 5 STRK with validator 0x05a2…beef"
    );
    assert_eq!(preparation.pool, f(STAKING_POOL));
    let amount = Felt::from(5 * 10u128.pow(18));
    assert_eq!(
        preparation.invoke.calls,
        [
            Call {
                contract_address: f(STRK),
                entrypoint: "approve".into(),
                calldata: vec![f(STAKING_POOL), amount, Felt::ZERO],
            },
            Call {
                contract_address: f(STAKING_POOL),
                entrypoint: "enter_delegation_pool".into(),
                calldata: vec![f(ACCOUNT), amount],
            },
        ]
    );
    assert_eq!(preparation.invoke.sender, f(ACCOUNT));
    assert_eq!(preparation.invoke.network, NetworkId::Mainnet);
}

#[test]
fn an_existing_member_adds_to_the_pool_instead_of_entering_it_again() {
    let preparation = act(&mut members_chain(), StakingAction::Stake, Some("1.5")).unwrap();
    assert_eq!(
        preparation.invoke.calls[1].entrypoint,
        "add_to_delegation_pool"
    );
}

#[test]
fn nothing_is_built_when_the_pool_cannot_be_verified() {
    let mut chain = chain();
    chain.pool_staker = Felt::from(0x999u32);
    assert_eq!(
        act(&mut chain, StakingAction::Stake, Some("5")),
        Err(StakingRefusal::Unverified)
    );
}

#[test]
fn a_leaving_validator_takes_no_new_stake_but_still_lets_members_exit() {
    let mut chain = members_chain();
    chain.staker_leaving = true;
    assert_eq!(
        act(&mut chain, StakingAction::Stake, Some("1")),
        Err(StakingRefusal::Leaving)
    );
    let exit = act(&mut chain, StakingAction::Unstake, Some("2")).unwrap();
    assert_eq!(
        exit.invoke.calls[0].entrypoint,
        "exit_delegation_pool_intent"
    );
}

#[test]
fn an_unstake_larger_than_the_stake_is_refused() {
    assert_eq!(
        act(&mut members_chain(), StakingAction::Unstake, Some("6")),
        Err(StakingRefusal::MoreThanStaked)
    );
    // An amount with more decimals than the token has reads as no amount.
    let precise = format!("1.{}", "0".repeat(19));
    assert_eq!(
        act(&mut members_chain(), StakingAction::Unstake, Some(&precise)),
        Err(StakingRefusal::UnstakeAmount)
    );
}

#[test]
fn withdraw_waits_for_the_exit_window_the_chain_reports() {
    let mut chain = chain();
    chain.member = Some(Member {
        amount: 0,
        unpool_amount: 5,
        unpool_time: Some(NOW + 60),
        ..member()
    });
    assert_eq!(
        act(&mut chain, StakingAction::Withdraw, None),
        Err(StakingRefusal::NotWithdrawableYet)
    );
    chain.member = Some(Member {
        unpool_time: Some(NOW),
        ..chain.member.unwrap()
    });
    let withdraw = act(&mut chain, StakingAction::Withdraw, None).unwrap();
    assert_eq!(
        withdraw.invoke.calls,
        [Call {
            contract_address: f(STAKING_POOL),
            entrypoint: "exit_delegation_pool_action".into(),
            calldata: vec![f(ACCOUNT)],
        }]
    );
    assert_eq!(
        withdraw.invoke.activity_summary(),
        "Withdraw unstaked STRK from validator 0x05a2…beef"
    );
}

#[test]
fn claim_needs_rewards_to_claim() {
    assert_eq!(
        act(&mut members_chain(), StakingAction::Claim, None),
        Err(StakingRefusal::NoRewards)
    );
}

#[test]
fn a_token_the_staking_contract_does_not_accept_is_refused() {
    let mut chain = chain();
    chain.active_tokens = vec![Felt::from(0x777u32)];
    assert_eq!(
        act(&mut chain, StakingAction::Stake, Some("1")),
        Err(StakingRefusal::TokenNotStakeable)
    );
    // Facts the wallet cannot trust read the same way (`!facts?.activeTokens.some(...)`).
    let mut chain = super::test_chain::chain();
    chain.reward_token = Felt::from(0x777u32);
    assert_eq!(
        act(&mut chain, StakingAction::Stake, Some("1")),
        Err(StakingRefusal::TokenNotStakeable)
    );
}

#[test]
fn a_pasted_address_the_staking_contract_does_not_list_is_refused_before_anything_is_built() {
    let mut chain = chain();
    let input = PrepareInput::new(StakingAction::Stake, Some("1"), "0x5a4").unwrap();
    assert_eq!(
        act_with(&mut chain, &input, &strk_config(), &SEND),
        Err(StakingRefusal::Unverified)
    );
    assert!(chain.reads.contains(&"staker_pool_info"));
    // The same staker in another spelling is the same validator.
    let padded = format!("0x0{}", &STAKER[2..]);
    let input = PrepareInput::new(StakingAction::Stake, Some("1"), &padded).unwrap();
    assert!(act_with(&mut chain, &input, &strk_config(), &SEND).is_ok());
}

#[test]
fn a_claim_discloses_rewards_paid_to_another_address_and_records_strk() {
    let mut chain = chain();
    chain.member = Some(Member {
        reward_address: Felt::from(0xabc123u32),
        unclaimed: 5,
        ..member()
    });
    // A token tracked under another symbol: the claim still records the rewards token.
    let staked = TokenConfig {
        symbol: "xSTRK".into(),
        ..strk_config()
    };
    let input = PrepareInput::new(StakingAction::Claim, None, STAKER).unwrap();
    let claim = act_with(&mut chain, &input, &staked, &SEND).unwrap();
    assert_eq!(
        claim.review,
        format!(
            "Claim your STRK rewards from {VALIDATOR}. Rewards are paid to 0x0000…c123, not this account."
        )
    );
    assert_eq!(
        claim.invoke.activity_summary(),
        "Claim staking rewards from validator 0x05a2…beef"
    );
    let asset = claim.invoke.activity_asset.unwrap();
    assert_eq!(
        (asset.symbol.as_str(), asset.token_address),
        ("STRK", f(STRK))
    );
    let input = PrepareInput::new(StakingAction::Stake, Some("1"), STAKER).unwrap();
    let stake = act_with(&mut chain, &input, &staked, &SEND).unwrap();
    assert_eq!(stake.invoke.activity_asset.unwrap().symbol, "xSTRK");
}

#[test]
fn a_claim_records_the_rewards_token_not_the_staked_one() {
    // A chain whose rewards token is not the staked token's address.
    let rewards = crate::catalog::TokenInfo {
        address: "0x5157".into(),
        ..strk_info()
    };
    let mut chain = chain();
    chain.reward_token = Felt::from(0x5157u32);
    chain.member = Some(Member {
        unclaimed: 5,
        ..member()
    });
    let account = account();
    let token = strk_config();
    let input = PrepareInput::new(StakingAction::Claim, None, STAKER).unwrap();
    let read = input.read(&NETWORK, &rewards, &account, &token).unwrap();
    let reading = run(read, &mut chain);
    let claim = prepare(&input, reading.as_ref(), &context(&account, &token, &SEND)).unwrap();
    assert_eq!(
        claim.invoke.activity_asset,
        Some(ActivityAsset {
            amount: None,
            symbol: "STRK".into(),
            token_address: Felt::from(0x5157u32),
        })
    );
}

#[test]
fn the_batch_review_can_still_refuse() {
    let one_call = Policy {
        max_invoke_calls: 1,
        ..SEND
    };
    let input = PrepareInput::new(StakingAction::Stake, Some("1"), STAKER).unwrap();
    assert_eq!(
        act_with(&mut chain(), &input, &strk_config(), &one_call),
        Err(StakingRefusal::Invoke(InvokeRefusal::TooManyCalls))
    );
}

#[test]
fn malformed_requests_are_refused_before_any_read() {
    let invalid = Err(StakingRefusal::InvalidRequest);
    let cases: [(StakingAction, Option<&str>, &str); 12] = [
        (StakingAction::Claim, Some("1"), STAKER),
        (StakingAction::Stake, None, STAKER),
        (StakingAction::Unstake, None, STAKER),
        (StakingAction::Stake, Some("-1"), STAKER),
        (StakingAction::Stake, Some("1."), STAKER),
        (StakingAction::Stake, Some(".5"), STAKER),
        (StakingAction::Stake, Some("1e5"), STAKER),
        (StakingAction::Stake, Some(" 1"), STAKER),
        (StakingAction::Stake, Some(""), STAKER),
        (StakingAction::Claim, None, "0x0"),
        (StakingAction::Claim, None, "0xZZ"),
        (StakingAction::Withdraw, Some("0"), STAKER),
    ];
    for (action, amount, staker) in cases {
        assert_eq!(
            PrepareInput::new(action, amount, staker),
            invalid,
            "{action:?} {amount:?} {staker}"
        );
    }
    let long = "1".repeat(81);
    assert_eq!(
        PrepareInput::new(StakingAction::Stake, Some(&long), STAKER),
        invalid
    );
    assert!(PrepareInput::new(StakingAction::Stake, Some(&"1".repeat(80)), STAKER).is_ok());
    assert!(PrepareInput::new(StakingAction::Unstake, Some("0.25"), STAKER).is_ok());
    assert!(PrepareInput::new(StakingAction::CancelUnstake, None, STAKER).is_ok());
}

#[test]
fn the_read_is_for_the_accounts_own_network_and_a_token_it_tracks_there() {
    let input = PrepareInput::new(StakingAction::Stake, Some("1"), STAKER).unwrap();
    let account = account();
    let off = StakingNetwork {
        enabled: false,
        ..NETWORK
    };
    let elsewhere = StakingNetwork {
        network: NetworkId::Sepolia,
        ..NETWORK
    };
    for network in [off, elsewhere] {
        assert!(matches!(
            input.read(&network, &strk_info(), &account, &strk_config()),
            Err(StakingRefusal::Unavailable)
        ));
    }
    let sepolia_token = TokenConfig {
        network: NetworkId::Sepolia,
        ..strk_config()
    };
    let bad_address = TokenConfig {
        address: "not a felt".into(),
        ..strk_config()
    };
    for token in [sepolia_token, bad_address] {
        assert!(matches!(
            input.read(&NETWORK, &strk_info(), &account, &token),
            Err(StakingRefusal::OptionUnavailable)
        ));
    }
}

fn reading() -> Option<StakingReading> {
    run(read_of(vec![strk_pair()]), &mut chain())
}

#[test]
fn prepare_takes_only_a_reading_of_this_pair_for_this_account_and_network() {
    let input = PrepareInput::new(StakingAction::Stake, Some("1"), STAKER).unwrap();
    let account = account();
    let token = strk_config();
    let context = context(&account, &token, &SEND);
    assert!(prepare(&input, reading().as_ref(), &context).is_ok());
    assert_eq!(
        prepare(&input, None, &context),
        Err(StakingRefusal::Unreadable)
    );
    for change in [
        |r: &mut StakingReading| r.account = Felt::from(7u8),
        |r: &mut StakingReading| r.network = NetworkId::Sepolia,
        |r: &mut StakingReading| r.staking_contract = Felt::from(7u8),
        |r: &mut StakingReading| r.pools[0].pair.staker = Felt::from(7u8),
        |r: &mut StakingReading| r.pools[0].pair.token = Felt::from(7u8),
    ] {
        let mut other = reading().unwrap();
        change(&mut other);
        assert_eq!(
            prepare(&input, Some(&other), &context),
            Err(StakingRefusal::Unreadable)
        );
    }
    // Staking switched off for the account's network: no contract to check the reading against.
    let off = super::StakingPolicy {
        networks: &[],
        ..POLICY
    };
    let switched_off = PrepareContext {
        staking: &off,
        ..self::context(&account, &token, &SEND)
    };
    assert_eq!(
        prepare(&input, reading().as_ref(), &switched_off),
        Err(StakingRefusal::Unavailable)
    );
    let elsewhere = TokenConfig {
        network: NetworkId::Sepolia,
        ..strk_config()
    };
    let context = self::context(&account, &elsewhere, &SEND);
    assert_eq!(
        prepare(&input, reading().as_ref(), &context),
        Err(StakingRefusal::OptionUnavailable)
    );
}
