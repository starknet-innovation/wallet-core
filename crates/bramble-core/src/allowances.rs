//! ERC-20 allowances on mobile (`wallet_core::allowances`, the extension's allowance manager in
//! `background/portfolio/allowance-{ops,recording}.ts`): the log beside the activity log and the
//! ranking a write evicts by. The revoke is `send::revoke`; a submitted batch's grants are filed
//! by the plan's host (`send::plan`). Public on-chain data only.

use std::path::Path;
use std::sync::{Mutex, PoisonError};

use wallet_core::allowances::retention::{RankToken, Ranking};
use wallet_core::allowances::store::AllowanceLog;
use wallet_core::allowances::AllowancePolicy;
use wallet_core::catalog::TokenInfo;

use crate::files::Directory;
use crate::networks::NetworkId;
use crate::portfolio::model::TokenConfig;
use crate::portfolio::tokens::TokenPreferences;

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
