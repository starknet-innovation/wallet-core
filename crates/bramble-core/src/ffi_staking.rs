//! Native staking on mobile (`wallet_core::staking`, the extension's `wallet.stakingDirectory`,
//! `wallet.stakingOverview` and `wallet.stakingPositions` in `background/staking/staking-ops.ts`):
//! Starkscan's validator directory and an account's positions as sans-IO HTTP steps, the chain
//! read behind every row as a sans-IO RPC step machine, and the rows. The actions are
//! `ffi_staking_action`.
//!
//! Mainnet only (`staking::STAKING_POLICY`): every entry point refuses another network with
//! "Staking isn't available on this network." Nothing Starkscan says is shown as an amount or
//! used to build a call: it only names candidates, which the chain read verifies (the staking
//! contract is the policy's, never the host's). Public on-chain data only.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use wallet_core::history::events::ScanStep;
pub use wallet_core::staking::calls::{PositionState, StakingAction};
use wallet_core::staking::directory::{directory_request, directory_view};
pub use wallet_core::staking::directory::{DirectoryEntry, DirectoryPoolEntry, DirectoryView};
use wallet_core::staking::positions::{positions_page, positions_request};
use wallet_core::staking::read::{StakingPair, StakingRead};
pub use wallet_core::staking::view::RowFigures;
use wallet_core::staking::view::{
    overview, overview_pairs, positions_pairs, positions_view, StakingRow,
};
use wallet_core::staking::{staker_address, StakingNetwork, StakingRefusal};
use wallet_core::{Felt, NetworkId};

use crate::networks::{self, TokenInfo};
use crate::portfolio::model::TokenConfig;
use crate::portfolio::tokens::TokenPreferences;
use crate::portfolio::wire::{HttpOutcome, HttpRequest, RpcReply, RpcRequest};
use crate::send::types::SendError;
use crate::send::wallet_ext::Use;
use crate::staking::STAKING_POLICY;
use crate::wallet::WalletCore;

/// Why a staking request was refused. `Refused` carries the extension's fixed sentence; none
/// carries an address, an amount or node text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum StakingError {
    #[error("wallet is locked")]
    Locked,
    #[error("unknown account")]
    UnknownAccount,
    /// The allowance log, contacts or multisig file could not be read.
    #[error("a wallet file could not be read")]
    Storage,
    #[error("{reason}")]
    Refused { reason: String },
}

impl From<StakingRefusal> for StakingError {
    fn from(refusal: StakingRefusal) -> Self {
        Self::Refused {
            reason: refusal.to_string(),
        }
    }
}

impl From<SendError> for StakingError {
    fn from(error: SendError) -> Self {
        match error {
            SendError::Locked => Self::Locked,
            SendError::UnknownAccount => Self::UnknownAccount,
            // `trusted_recipients` fails closed on an unreadable contacts or multisig file.
            _ => Self::Storage,
        }
    }
}

impl From<wallet_core::Error> for StakingError {
    fn from(_: wallet_core::Error) -> Self {
        Self::Storage
    }
}

/// `StakingAction`.
#[uniffi::remote(Enum)]
pub enum StakingAction {
    /// First stake or add stake (`approve` + enter or add).
    Stake,
    /// Claim the pool's rewards to the member's reward address.
    Claim,
    /// Start the exit window for an amount (`exit_delegation_pool_intent`).
    Unstake,
    /// Cancel a pending exit.
    CancelUnstake,
    /// Withdraw a pending exit once its window has passed (`exit_delegation_pool_action`).
    Withdraw,
}

/// Where the account stands in one pool.
#[uniffi::remote(Enum)]
pub enum PositionState {
    NotStaked,
    Staked,
    ExitPending,
    Withdrawable,
}

/// One tracked token's pool as a directory entry shows it.
#[uniffi::remote(Record)]
pub struct DirectoryPoolEntry {
    pub delegated: Option<String>,
    pub symbol: String,
}

/// One validator as the directory lists it: display strings only, never signed.
#[uniffi::remote(Record)]
pub struct DirectoryEntry {
    pub commission: Option<String>,
    pub name: Option<String>,
    pub pools: Vec<DirectoryPoolEntry>,
    pub staker: String,
    pub uptime: Option<String>,
    pub website: Option<String>,
}

/// A directory page past "off" (no request): an incomplete snapshot offers no validator.
#[uniffi::remote(Enum)]
pub enum DirectoryView {
    Incomplete,
    Ready {
        entries: Vec<DirectoryEntry>,
        next_cursor: Option<String>,
    },
}

/// A verified row's figures, formatted in the token's decimals (rewards in STRK's).
#[uniffi::remote(Record)]
pub struct RowFigures {
    pub accepting: bool,
    pub commission: String,
    pub delegated: String,
    pub rewards_to: Option<String>,
    pub staked: String,
    pub state: PositionState,
    pub unclaimed: String,
    pub unpool_amount: String,
    pub unpool_time: Option<u64>,
}

/// `StakingRowView`: one staker and tracked token. `refusal` replaces `figures` when the pool
/// could not be verified, and the row offers no action.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct StakingRowView {
    pub staker: String,
    pub symbol: String,
    pub token_id: String,
    pub figures: Option<RowFigures>,
    pub refusal: Option<String>,
}

impl From<StakingRow> for StakingRowView {
    fn from(row: StakingRow) -> Self {
        let (figures, refusal) = match row.figures {
            Ok(figures) => (Some(figures), None),
            Err(refusal) => (None, Some(refusal.to_string())),
        };
        Self {
            staker: row.staker,
            symbol: row.symbol,
            token_id: row.token_id,
            figures,
            refusal,
        }
    }
}

/// What a chain read shows: one validator's pools (the overview, always `complete`) or "Your
/// stakes" (`complete` false whenever the list may be missing something, so an empty list is
/// never read as "no stakes"). `exit_window_seconds` is the staking contract's.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct StakingView {
    pub exit_window_seconds: u64,
    pub complete: bool,
    pub rows: Vec<StakingRowView>,
}

/// `network`'s staking record: Mainnet only.
pub(crate) fn staking_network(network: NetworkId) -> Result<&'static StakingNetwork, StakingError> {
    Ok(STAKING_POLICY
        .network(network)
        .ok_or(StakingRefusal::Unavailable)?)
}

/// Every token the account tracks on `network` (`preferences.tokens`).
pub(crate) fn tracked_tokens(prefs: &TokenPreferences, network: NetworkId) -> Vec<TokenConfig> {
    prefs
        .tracked(network)
        .into_iter()
        .map(|tracked| tracked.token)
        .collect()
}

/// The token a staking read checks the rewards are paid in: the network catalog's STRK.
pub(crate) fn rewards_token(network: NetworkId) -> Result<&'static TokenInfo, StakingError> {
    let strk = networks::info(network)
        .tokens
        .iter()
        .find(|token| token.symbol == "STRK");
    Ok(strk.ok_or(StakingRefusal::Unavailable)?)
}

/// A read of `pairs` for `account` against the policy's staking contract.
fn chain_read(
    network: NetworkId,
    account: Felt,
    pairs: Vec<StakingPair>,
) -> Result<StakingRead, StakingError> {
    let (staking, rewards) = (staking_network(network)?, rewards_token(network)?);
    let read = StakingRead::new(staking, rewards, account, pairs);
    Ok(read.ok_or(StakingRefusal::Unavailable)?)
}

/// One directory page's request (`cursor`: the last page's `next_cursor`); `None` is "off".
#[uniffi::export]
pub fn staking_directory_request(
    network: NetworkId,
    cursor: Option<String>,
) -> Result<Option<HttpRequest>, StakingError> {
    let base = crate::HISTORY_POLICY.explorer_api_base;
    Ok(directory_request(
        network,
        &STAKING_POLICY,
        base,
        cursor.as_deref(),
    )?)
}

/// Judges a directory reply: active validators with an active pool for a token the person
/// tracks on `network`, in the directory's order.
#[uniffi::export]
pub fn staking_directory_view(
    network: NetworkId,
    outcome: HttpOutcome,
    tokens: Arc<TokenPreferences>,
) -> Result<DirectoryView, StakingError> {
    let tokens = tracked_tokens(&tokens, network);
    Ok(directory_view(&outcome, network, &tokens, &STAKING_POLICY)?)
}

enum Shows {
    Overview,
    Positions { complete: bool },
}

/// One chain read behind the overview or "Your stakes", pinned to one block. Ask `next_calls`
/// until it is empty, then `view`.
#[derive(uniffi::Object)]
pub struct StakingChainRead {
    account: Felt,
    network: NetworkId,
    tokens: Vec<TokenConfig>,
    shows: Shows,
    read: Mutex<StakingRead>,
}

impl StakingChainRead {
    fn read(&self) -> MutexGuard<'_, StakingRead> {
        self.read.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[uniffi::export]
impl StakingChainRead {
    /// The calls to send now to an endpoint of the account's network; empty while waiting for
    /// replies or when done.
    pub fn next_calls(&self) -> Vec<RpcRequest> {
        self.read().next_calls()
    }

    /// Judges one reply. `Transport`: `restart` on the next endpoint.
    pub fn accept(&self, reply: RpcReply) -> ScanStep {
        self.read().accept(reply)
    }

    /// Discards the read so it replays on another endpoint.
    pub fn restart(&self) {
        self.read().restart();
    }

    /// The rows at Unix second `now_seconds`; refused while reads remain or when the read failed.
    pub fn view(&self, now_seconds: u64) -> Result<StakingView, StakingError> {
        let reading = self.read().finish();
        let (account, network, tokens) = (&self.account, self.network, &self.tokens);
        let exit_window_seconds = reading
            .as_ref()
            .and_then(|reading| reading.facts())
            .map_or(0, |facts| facts.exit_window_seconds);
        let (complete, rows) = match self.shows {
            Shows::Overview => {
                let view = overview(reading.as_ref(), tokens, account, network, now_seconds)?;
                (true, view.rows)
            }
            Shows::Positions { complete } => {
                let view = positions_view(
                    reading.as_ref(),
                    tokens,
                    account,
                    network,
                    now_seconds,
                    complete,
                )?;
                (view.complete, view.rows)
            }
        };
        Ok(StakingView {
            exit_window_seconds,
            complete,
            rows: rows.into_iter().map(StakingRowView::from).collect(),
        })
    }
}

impl WalletCore {
    fn staking_read(
        &self,
        account_id: &str,
        tokens: &TokenPreferences,
        pairs: impl FnOnce(
            NetworkId,
            &Felt,
            &[TokenConfig],
        ) -> Result<(Vec<StakingPair>, Shows), StakingError>,
    ) -> Result<Arc<StakingChainRead>, StakingError> {
        let acting = self.plan_account_for(account_id, Use::Read)?;
        let (network, account) = (acting.network, acting.account.address);
        let tokens = tracked_tokens(tokens, network);
        let (pairs, shows) = pairs(network, &account, &tokens)?;
        Ok(Arc::new(StakingChainRead {
            read: Mutex::new(chain_read(network, account, pairs)?),
            account,
            network,
            tokens,
            shows,
        }))
    }
}

#[uniffi::export]
impl WalletCore {
    /// One validator's pools for `account_id` (`wallet.stakingOverview`): `staker` (from the
    /// directory or pasted) with each token the account tracks, verified on chain.
    pub fn staking_overview(
        &self,
        account_id: String,
        staker: String,
        tokens: Arc<TokenPreferences>,
    ) -> Result<Arc<StakingChainRead>, StakingError> {
        let staker = staker_address(&staker).ok_or(StakingRefusal::InvalidRequest)?;
        self.staking_read(&account_id, &tokens, |network, _, tokens| {
            Ok((overview_pairs(&staker, tokens, network), Shows::Overview))
        })
    }

    /// The positions request for `account_id` ("Your stakes"); `None` is "off": the person's
    /// explorer choice keeps this account's address from Starkscan.
    pub fn staking_positions_request(
        &self,
        account_id: String,
        tokens: Arc<TokenPreferences>,
    ) -> Result<Option<HttpRequest>, StakingError> {
        let acting = self.plan_account_for(&account_id, Use::Read)?;
        let network = acting.network;
        let allowed = self.explorer_fallback_allowed(network, tokens);
        let base = crate::HISTORY_POLICY.explorer_api_base;
        Ok(positions_request(
            network,
            &STAKING_POLICY,
            base,
            &acting.account.address,
            allowed,
        )?)
    }

    /// Judges the positions reply and starts the chain read of its candidates. A reply for
    /// another chain or address is refused ("Couldn't load your stakes.").
    pub fn staking_positions(
        &self,
        account_id: String,
        outcome: HttpOutcome,
        tokens: Arc<TokenPreferences>,
    ) -> Result<Arc<StakingChainRead>, StakingError> {
        self.staking_read(&account_id, &tokens, |network, account, tokens| {
            let page = positions_page(&outcome, network, account, &STAKING_POLICY)?;
            let (pairs, complete) = positions_pairs(&page, tokens, network, &STAKING_POLICY);
            Ok((pairs, Shows::Positions { complete }))
        })
    }
}
