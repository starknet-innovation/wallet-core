//! Native Starknet staking (delegation pools), sans-IO: a port of the extension's
//! `packages/features/transactions/src/staking/` and the rules in
//! `apps/extension/src/background/staking/`.
//!
//! - `decode`: the staking contract's and a pool's replies, and the pool verification rule.
//! - `read`: the chain reads for one account, pinned to one accepted block (`staking-reads.ts`).
//! - `coverage`, `directory`, `positions`: Starkscan's validator directory and an address's
//!   positions, as requests and strict judges.
//! - `calls`, `review`: the calls an action builds and the sentences it is reviewed and recorded
//!   under.
//! - `view`: the rows a host shows (`staking-row-view.ts`, the overview and "Your stakes").
//! - `prepare`: an action, from the person's input and a verified reading to a reviewed
//!   `send::invoke::Invoke`, frozen until one confirm claims it (`staking-action.ts`,
//!   `staking-preparations.ts`, `staking-inputs.ts`).
//!
//! Trust boundary, as in the extension: the staking contract comes only from the wallet's own
//! `StakingNetwork`; a staker address (from the directory or pasted) is a candidate until the
//! staking contract lists a pool for it and the pool's own parameters agree, and before a stake
//! a second source names the same pool (`prepare::PrepareContext::check`); a pool address is
//! never taken from Starkscan or the host; and nothing Starkscan says is signed or shown as an
//! amount. Every number that is Bramble's choice is a `StakingPolicy`.

pub mod calls;
pub mod coverage;
pub mod decode;
pub mod directory;
pub mod positions;
pub mod prepare;
pub mod read;
pub mod review;
pub mod view;

#[cfg(test)]
mod calls_tests;
#[cfg(test)]
mod decode_tests;
#[cfg(test)]
mod directory_ops_tests;
#[cfg(test)]
mod directory_tests;
#[cfg(test)]
mod integration_tests;
#[cfg(test)]
mod pool_check_tests;
#[cfg(test)]
mod positions_tests;
#[cfg(test)]
mod preparations_tests;
#[cfg(test)]
mod prepare_tests;
#[cfg(test)]
mod read_tests;
#[cfg(test)]
pub(crate) mod test_chain;
#[cfg(test)]
mod view_tests;

use crate::send::invoke::InvokeRefusal;
use crate::{felt, Felt, NetworkId};

/// One network's staking record (`StakingNetwork` in `staking-networks.ts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StakingNetwork {
    pub network: NetworkId,
    /// Whether staking is offered here at all; off, every staking request is refused.
    pub enabled: bool,
    /// The one staking contract the wallet builds calls for.
    pub staking_contract: &'static str,
    /// Whether Starkscan's validator directory and positions are read here (the extension reads
    /// them on Mainnet only; a pasted validator always works).
    pub starkscan: bool,
}

/// A wallet's staking choices (Bramble's: `bramble-core`'s `STAKING_POLICY`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StakingPolicy {
    pub networks: &'static [StakingNetwork],
    /// Validators asked for per directory page; a page with more is refused.
    pub directory_page_size: usize,
    /// Starkscan positions read on chain per request (`MAX_CHECKED_POSITIONS`); more makes the
    /// list incomplete.
    pub max_checked_positions: usize,
    /// How far a `catching_up` Starkscan snapshot may trail and still be offered whole.
    pub max_catching_up_lag_blocks: u64,
    /// Preparations held at once; the oldest is dropped past it.
    pub max_preparations: usize,
}

impl StakingPolicy {
    /// `stakingNetwork`: the network's record when staking is switched on there.
    pub fn network(&self, network: NetworkId) -> Option<&StakingNetwork> {
        self.networks
            .iter()
            .find(|entry| entry.network == network && entry.enabled)
    }
}

/// `stakerAddress`: a validator's staker address, when `value` is a non-zero felt in the
/// extension's grammar. Only a candidate: the staking contract decides whether it is a validator.
pub fn staker_address(value: &str) -> Option<Felt> {
    felt::parse_non_zero(value)
}

/// Why a staking request was refused. `Display` is the extension's sentence; none carries an
/// address, an amount or node text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StakingRefusal {
    #[error("Staking isn't available on this network.")]
    Unavailable,
    #[error("Couldn't read staking on this network. Try again.")]
    Unreadable,
    #[error("Couldn't load the validator list. Paste a validator address instead.")]
    DirectoryUnreadable,
    #[error("Couldn't load your stakes.")]
    PositionsUnreadable,
    #[error("invalid staking request")]
    InvalidRequest,
    #[error("Staking option unavailable.")]
    OptionUnavailable,
    #[error("This token can't be staked.")]
    TokenNotStakeable,
    #[error("Couldn't verify this validator on chain.")]
    Unverified,
    #[error("This validator has no pool for this token.")]
    NoPoolForToken,
    #[error("Couldn't read your stake with this validator.")]
    MemberUnreadable,
    #[error("This validator is leaving staking.")]
    Leaving,
    #[error("Enter an amount to stake.")]
    StakeAmount,
    #[error("No rewards to claim.")]
    NoRewards,
    #[error("Withdraw or cancel your unstake first.")]
    PendingExit,
    #[error("Nothing staked to unstake.")]
    NothingStaked,
    #[error("Enter an amount to unstake.")]
    UnstakeAmount,
    #[error("That's more than you have staked.")]
    MoreThanStaked,
    #[error("No unstake to cancel.")]
    NoUnstake,
    #[error("Not withdrawable yet.")]
    NotWithdrawableYet,
    #[error("Nothing to withdraw.")]
    NothingToWithdraw,
    #[error("Review this staking action again.")]
    ReviewAgain,
    /// The batch review refused the calls (a poisoned payee, for one).
    #[error("{0}")]
    Invoke(#[from] InvokeRefusal),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_staker_address_is_a_non_zero_felt_or_nothing() {
        // `staking-calls.test.ts` "a staker address is a canonical non-zero felt or nothing".
        assert_eq!(
            staker_address("0x0abc").map(|f| felt::canonical(&f)),
            Some("0xabc".into())
        );
        let too_big = format!("0x{}", "f".repeat(64));
        for bad in ["0x0", "0xABC", "abc", "0x", "", too_big.as_str()] {
            assert_eq!(staker_address(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_switched_off_network_has_no_record() {
        // "a switched-on network needs no validator list", and the off switch.
        const ON: StakingNetwork = StakingNetwork {
            network: NetworkId::Mainnet,
            enabled: true,
            staking_contract: "0x1",
            starkscan: false,
        };
        let policy = |networks: &'static [StakingNetwork]| StakingPolicy {
            networks,
            ..test_chain::POLICY
        };
        assert_eq!(policy(&[ON]).network(NetworkId::Mainnet), Some(&ON));
        assert_eq!(policy(&[ON]).network(NetworkId::Sepolia), None);
        const OFF: StakingNetwork = StakingNetwork {
            enabled: false,
            ..ON
        };
        assert_eq!(policy(&[OFF]).network(NetworkId::Mainnet), None);
    }

    // As `allowances_never_touch_key_material`: staking reads public chain state and builds calls
    // for the plan to sign; it never holds a key.
    #[test]
    fn staking_never_touches_key_material() {
        let sources = [
            include_str!("calls.rs"),
            include_str!("coverage.rs"),
            include_str!("decode.rs"),
            include_str!("directory.rs"),
            include_str!("positions.rs"),
            include_str!("prepare.rs"),
            include_str!("read.rs"),
            include_str!("review.rs"),
            include_str!("view.rs"),
        ];
        for source in sources {
            for forbidden in [
                "crate::kms",
                "Zeroizing",
                "SecretFelt",
                "mnemonic",
                "private_key",
                "viewing_key",
            ] {
                assert!(
                    !source.contains(forbidden),
                    "staking code must not reference {forbidden}"
                );
            }
        }
    }
}
