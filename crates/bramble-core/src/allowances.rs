//! ERC-20 allowances on mobile (`wallet_core::allowances`, the extension's allowance manager in
//! `background/portfolio/allowance-{ops,recording}.ts`): the log beside the activity log, the
//! ranking a write evicts by, and the listing as one sans-IO step machine. The revoke is
//! `send::revoke`; a submitted batch's grants are filed by the plan's host (`send::plan`).
//!
//! The listing is the extension's `wallet.allowances` handler, whole: the core reads the log, the
//! account and the revoke outcomes itself, so nothing the host hands back is re-validated. The
//! host only moves the RPC bytes. Public on-chain data only.

use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use wallet_core::allowances::calls::AllowanceOwner;
use wallet_core::allowances::discovery::{
    listing_notices, merge_allowance_views, outstanding_allowances, AllowanceReading,
};
pub use wallet_core::allowances::discovery::{AllowanceNotice, AllowanceSource};
use wallet_core::allowances::record::AllowanceRecord;
use wallet_core::allowances::retention::{RankToken, Ranking};
use wallet_core::allowances::revoke::revoke_outcomes;
use wallet_core::allowances::scan::{ExternalAllowanceScan, ExternalAllowanceScanResult};
use wallet_core::allowances::store::{retention_full, AllowanceLog, RevokeOutcome};
use wallet_core::allowances::tracked::{tracked_pairs, TrackedAllowanceRead};
use wallet_core::allowances::AllowancePolicy;
use wallet_core::catalog::TokenInfo;
use wallet_core::history::events::ScanStep;

use crate::activity::with_log;
use crate::files::Directory;
use crate::networks::NetworkId;
use crate::portfolio::model::TokenConfig;
use crate::portfolio::tokens::TokenPreferences;
use crate::portfolio::wire::{RpcReply, RpcRequest};
use crate::send::types::SendError;
use crate::send::wallet_ext::Use;
use crate::wallet::WalletCore;
use crate::{felt, multisig_store, CoreError};

/// The log's file in the wallet directory, beside `activity.v1.json`. Mobile keeps Bramble's
/// numbers (`ALLOWANCE_POLICY`) under its own name: the extension's `bramble:allowances` is a
/// local-storage key, and no record crosses between the two.
pub(crate) const ALLOWANCE_FILE: &str = "allowances.v1.json";

const POLICY: AllowancePolicy = AllowancePolicy {
    store_key: ALLOWANCE_FILE,
    ..crate::ALLOWANCE_POLICY
};

/// Serialises every read-modify-write of the file within the process (the extension's mutation
/// queue).
static FILE_LOCK: Mutex<()> = Mutex::new(());

/// Runs `f` over the log in `dir` under the process-wide lock, the one way any code here reaches
/// it.
pub(crate) fn with_allowance_log<T>(dir: &Path, f: impl FnOnce(&mut AllowanceLog<'_>) -> T) -> T {
    let _guard = FILE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    let storage = Directory(dir);
    f(&mut AllowanceLog::new(&storage, POLICY))
}

/// The tracked tokens the review and the log trust on `network` (`trustedTokens`): every one the
/// spam heuristic does not flag.
pub(crate) fn trusted_tokens(prefs: &TokenPreferences, network: NetworkId) -> Vec<TokenConfig> {
    prefs
        .tracked(network)
        .into_iter()
        .filter(|tracked| !tracked.likely_spam)
        .map(|tracked| tracked.token)
        .collect()
}

/// `tokens` as the review's `Known::tokens` reads them.
pub(crate) fn token_infos(tokens: &[TokenConfig]) -> Vec<TokenInfo> {
    tokens
        .iter()
        .map(|token| TokenInfo {
            address: token.address.clone(),
            decimals: token.decimals,
            name: token.name.clone(),
            symbol: token.symbol.clone(),
        })
        .collect()
}

/// The ranking a write evicts by: the trusted tokens' decimals (`rankTokens`).
pub(crate) fn ranking(tokens: &[TokenConfig]) -> Ranking {
    let ranked: Vec<RankToken> = tokens
        .iter()
        .map(|token| RankToken {
            network: token.network,
            address: token.address.clone(),
            decimals: token.decimals.into(),
        })
        .collect();
    Ranking::new(&ranked)
}

/// One row of the allowance manager (`AllowanceView`): a grant this account made, or one found
/// on chain. `amount` is base-10 base units of `token`; when `unconfirmed_token`, the wallet
/// cannot say the contract is fungible, so neither the amount nor the revoke may be offered.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AllowanceView {
    pub id: String,
    pub account_id: String,
    pub network: NetworkId,
    pub token: String,
    pub spender: String,
    pub amount: String,
    /// The spender may hold at least `amount`, not exactly.
    pub lower_bound: bool,
    /// A revoke was submitted and the chain has not confirmed it.
    pub revoking: bool,
    pub source: AllowanceSource,
    pub unconfirmed_token: bool,
}

/// What the manager lists, and what it could not vouch for.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AllowanceList {
    pub allowances: Vec<AllowanceView>,
    pub notices: Vec<AllowanceNotice>,
}

/// Where a listed allowance came from (`AllowanceSource`).
#[uniffi::remote(Enum)]
pub enum AllowanceSource {
    Wallet,
    /// A watched multisig's grant (read-only). Mobile lists none yet.
    Multisig,
    External,
}

/// What a listing could not vouch for (`ALLOWANCE_NOTICES`), so the manager never says "none
/// found" over a list it knows is partial.
#[uniffi::remote(Enum)]
pub enum AllowanceNotice {
    /// On-chain discovery stopped early or could not run.
    ScanIncomplete,
    /// A multisig on this network was not read, so what it granted is not listed.
    MultisigUnread,
    /// The log is at a retention cap, so smaller approvals may have been dropped.
    RetentionFull,
}

enum Phase {
    External,
    Tracked,
    Done,
}

struct Listing {
    owner: AllowanceOwner,
    trusted: Vec<String>,
    stored: Vec<AllowanceRecord>,
    scoped: Vec<AllowanceRecord>,
    outcomes: Vec<RevokeOutcome>,
    multisig_unread: bool,
    phase: Phase,
    external: ExternalAllowanceScan,
    scanned: ExternalAllowanceScanResult,
    tracked: TrackedAllowanceRead,
    readings: Vec<AllowanceReading>,
}

impl Listing {
    // Moves past every read that has finished; stops at one still waiting.
    fn next_calls(&mut self) -> Vec<RpcRequest> {
        loop {
            match self.phase {
                Phase::External => {
                    let calls = self.external.next_calls();
                    if !calls.is_empty() {
                        return calls;
                    }
                    let Ok(result) = self.external.finish() else {
                        return calls;
                    };
                    self.scanned = result;
                    self.phase = Phase::Tracked;
                }
                Phase::Tracked => {
                    let calls = self.tracked.next_calls();
                    if !calls.is_empty() {
                        return calls;
                    }
                    let Ok(readings) = self.tracked.finish() else {
                        return calls;
                    };
                    self.readings = readings;
                    self.phase = Phase::Done;
                }
                Phase::Done => return Vec::new(),
            }
        }
    }

    // `handleListAllowances` after its reads: settle, re-read what settled, merge, and say what
    // the list cannot vouch for.
    fn finish(&self, dir: &Path) -> AllowanceList {
        let owner = &self.owner;
        let settled = with_allowance_log(dir, |log| {
            // Best effort: the listing does not depend on it.
            let _ = log.settle_revocations(&self.readings, &self.outcomes);
            if self.outcomes.is_empty() {
                return self.scoped.clone();
            }
            log.list()
                .unwrap_or_else(|_| self.stored.clone())
                .into_iter()
                .filter(|entry| self.scoped.iter().any(|scoped| scoped.id == entry.id))
                .collect()
        });
        let views = merge_allowance_views(
            &owner.account_id,
            owner.network,
            &settled,
            &[],
            &self.scanned.allowances,
            &self.readings,
        );
        let fungible = |token: &str| self.trusted.iter().any(|trusted| same_felt(trusted, token));
        let allowances = outstanding_allowances(views, fungible, &self.readings)
            .into_iter()
            .map(|view| AllowanceView {
                id: view.id,
                account_id: view.account_id,
                network: view.network,
                token: view.token,
                spender: view.spender,
                amount: view.amount.to_decimal(),
                lower_bound: view.lower_bound,
                revoking: view.revoking,
                source: view.source,
                unconfirmed_token: view.unconfirmed_token,
            })
            .collect();
        let notices = listing_notices(
            self.scanned.complete,
            self.multisig_unread,
            retention_full(&self.stored, &owner.account_id, &POLICY),
        );
        AllowanceList {
            allowances,
            notices,
        }
    }
}

fn same_felt(left: &str, right: &str) -> bool {
    matches!((felt::parse(left), felt::parse(right)), (Some(a), Some(b)) if a == b)
}

/// One listing of the allowance manager for one account on its network: the discovery scan of
/// its trusted tokens' `Approval` events, then the live amount of every grant the log tracks,
/// one call at a time. The revoke outcomes are read from the wallet's own activity rows when
/// the listing is made, before any reading, as the extension orders them.
#[derive(uniffi::Object)]
pub struct AllowanceListing {
    dir: std::path::PathBuf,
    listing: Mutex<Listing>,
}

impl AllowanceListing {
    fn lock(&self) -> MutexGuard<'_, Listing> {
        self.listing.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[uniffi::export]
impl AllowanceListing {
    /// The calls to send now (one at a time) to an endpoint of the account's network; empty while
    /// waiting for a reply, or when `finish` may be called.
    pub fn next_calls(&self) -> Vec<RpcRequest> {
        self.lock().next_calls()
    }

    /// Judges one reply. `Transport`: replay the current read on the next endpoint (`restart`),
    /// or, with none left, `give_up` on it.
    pub fn accept(&self, reply: RpcReply) -> ScanStep {
        let mut listing = self.lock();
        match listing.phase {
            Phase::External => listing.external.accept(reply),
            Phase::Tracked => listing.tracked.accept(reply),
            Phase::Done => ScanStep::Transport,
        }
    }

    /// Discards the current read's progress so it replays on another endpoint.
    pub fn restart(&self) {
        let mut listing = self.lock();
        match listing.phase {
            Phase::External => listing.external.restart(),
            Phase::Tracked => listing.tracked.restart(),
            Phase::Done => {}
        }
    }

    /// No endpoint is left for the current read: discovery lists nothing and says so
    /// (`ScanIncomplete`); the tracked reads list the stored amounts.
    pub fn give_up(&self) {
        let mut listing = self.lock();
        match listing.phase {
            Phase::External => {
                listing.scanned = ExternalAllowanceScanResult::unavailable();
                listing.phase = Phase::Tracked;
            }
            Phase::Tracked => {
                listing.readings = Vec::new();
                listing.phase = Phase::Done;
            }
            Phase::Done => {}
        }
    }

    /// Settles pending revokes (a zero reading, or the revoke's own outcome), then lists. Only
    /// once every read was answered or given up.
    pub fn finish(&self) -> Result<AllowanceList, CoreError> {
        let mut listing = self.lock();
        listing.next_calls();
        match listing.phase {
            Phase::Done => Ok(listing.finish(&self.dir)),
            _ => Err(CoreError::Rpc {
                reason: "allowance reads are not finished".into(),
            }),
        }
    }
}

#[uniffi::export]
impl WalletCore {
    /// The allowance manager's listing for `account_id` (a vault record id) over the person's
    /// trusted tokens on its network. Requires an unlocked wallet; any family may be read.
    pub fn allowance_listing(
        &self,
        account_id: String,
        tokens: Arc<TokenPreferences>,
    ) -> Result<Arc<AllowanceListing>, CoreError> {
        let acting =
            self.plan_account_for(&account_id, Use::Read)
                .map_err(|error| match error {
                    SendError::Locked => CoreError::Locked,
                    _ => CoreError::InvalidInput {
                        reason: "unknown account".into(),
                    },
                })?;
        let network = acting.network;
        let address = acting.account.address;
        let owner = AllowanceOwner::of(network, &address).ok_or(CoreError::InvalidInput {
            reason: "unknown account".into(),
        })?;
        let trusted: Vec<String> = trusted_tokens(&tokens, network)
            .into_iter()
            .map(|token| token.address)
            .collect();
        let stored = with_allowance_log(&self.data_dir, |log| log.list())?;
        let scoped: Vec<AllowanceRecord> = stored
            .iter()
            .filter(|r| r.network == network && r.account_id == owner.account_id)
            .cloned()
            .collect();
        let rows = with_log(&self.data_dir, |log| log.scope(&acting.account_id));
        let outcomes = revoke_outcomes(&scoped, network, &rows);
        let catalog_id = network.catalog_id();
        let multisig_unread = multisig_store::load(&self.data_dir).map_or(true, |file| {
            file.records.iter().any(|r| r.network_id == catalog_id)
        });
        let address = felt::canonical(&address);
        let pairs = tracked_pairs(&scoped, |id| {
            (id == owner.account_id).then(|| address.clone())
        });
        Ok(Arc::new(AllowanceListing {
            dir: self.data_dir.clone(),
            listing: Mutex::new(Listing {
                external: ExternalAllowanceScan::new(&address, &trusted, POLICY),
                tracked: TrackedAllowanceRead::new(&pairs, POLICY),
                owner,
                trusted,
                stored,
                scoped,
                outcomes,
                multisig_unread,
                phase: Phase::External,
                scanned: ExternalAllowanceScanResult::unavailable(),
                readings: Vec::new(),
            }),
        }))
    }
}
