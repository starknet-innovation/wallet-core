//! `staking-review.ts`: the sentence a staking action is confirmed under, stating the facts the
//! calls cannot show: a stake also approves exactly its amount for the pool; an unstake earns
//! nothing while it waits; and a membership whose on-chain reward address is not this account
//! pays its rewards there (another app may have set it). Rewards are STRK for every pool, BTC
//! pools included, so a claim names STRK. Signing text: edit only with the reviewed wording.

use super::calls::StakingAction;
use super::decode::PoolMember;
use crate::js::num::{js_trim, short_label};
use crate::{felt, Felt};

/// `stakingReviewSentence`. `member` is the membership read at the block the calls were built
/// from; `validator` names the staker as the review shows it.
pub fn review_sentence(
    account: &Felt,
    action: StakingAction,
    amount: Option<&str>,
    member: Option<&PoolMember>,
    symbol: &str,
    validator: &str,
) -> String {
    let figure = format!("{} {symbol}", amount.unwrap_or_default());
    let figure = js_trim(&figure);
    let what = match action {
        StakingAction::CancelUnstake => {
            format!("Cancel your unstake and keep the {symbol} staked with {validator}.")
        }
        StakingAction::Claim => format!("Claim your STRK rewards from {validator}."),
        StakingAction::Stake => {
            format!("Stake {figure} with {validator}. This approves exactly {figure} for its pool.")
        }
        StakingAction::Unstake => {
            format!("Unstake {figure} from {validator}. It earns no rewards until you withdraw it.")
        }
        StakingAction::Withdraw => format!("Withdraw your unstaked {symbol} from {validator}."),
    };
    match member.map(|member| member.reward_address) {
        Some(reward)
            if reward != *account
                && matches!(action, StakingAction::Claim | StakingAction::Stake) =>
        {
            format!(
                "{what} Rewards are paid to {}, not this account.",
                short_label(&felt::canonical(&reward))
            )
        }
        _ => what,
    }
}
