//! The extension's two shadow-account reads (`official-sdk-reads.ts` `shadowAccountCommitment`,
//! `official-sdk-shadow-accounts.ts` `shadowAccounts`), as sans-IO pieces: bodies out, replies
//! in, nothing sent from here.
//!
//! - The commitment read runs `discoverNotes()` first: one unfiltered `incoming_state` walk at
//!   the service's block (`ShadowCommitmentRead`), whose notes the extension throws away. Only a
//!   completed walk lets the read answer; the answer is then `scope_commitment`, computed
//!   locally.
//! - The accounts read sends the partial commitment to the node: one `get_shadow_accounts`
//!   `starknet_call` per window of nonces (`nonce_windows`, `get_shadow_accounts_body`), each
//!   reply judged strictly (`judge_shadow_accounts`). The windows' bounds are the wallet's
//!   (`ShadowScanLimits`).

use super::discovery::Walked;
use super::incoming::IncomingWalk;
use super::shadow::{commitment, partial_commitment};
use super::types::PrivacyFailure;
use crate::send::judge::{answer, call_felts};
use crate::send::rpc_bodies;
use crate::{selector, Felt, SecretFelt};

/// The anonymizer's range view.
pub const GET_SHADOW_ACCOUNTS: &str = "get_shadow_accounts";

/// `shadowAccountCommitment`'s discovery: `transfers.discoverNotes()`, an `incoming_state` walk
/// with no block and no token filter, whose notes are dropped.
#[derive(Debug)]
pub struct ShadowCommitmentRead {
    walk: IncomingWalk,
}

impl ShadowCommitmentRead {
    /// The read for `account` in `pool`; at most `max_notes` nonzero notes per token are
    /// admitted on the way (and then dropped).
    pub fn new(pool: Felt, account: Felt, max_notes: usize) -> Self {
        Self {
            walk: IncomingWalk::unfiltered(pool, account, max_notes),
        }
    }

    /// The next `POST /v1/sync/incoming_state` body (`incoming::INCOMING_PATH`). It carries the
    /// viewing key.
    pub fn body(&self, viewing_key: &SecretFelt) -> zeroize::Zeroizing<Vec<u8>> {
        self.walk.body(viewing_key)
    }

    /// Folds the reply in: `Done` once the walk completes, judged as the proof job's walk is.
    pub fn on_reply(self, status: u16, body: &[u8]) -> Result<Walked<Self, ()>, PrivacyFailure> {
        Ok(match self.walk.on_reply(status, body)? {
            Walked::More(walk) => Walked::More(Self { walk }),
            Walked::Done(_) => Walked::Done(()),
        })
    }
}

/// The commitment read's answer: the scope's partial commitment, or with `nonce` the commitment
/// of that shadow account (`partialCommitment()` / `commitment(nonce)`).
pub fn scope_commitment(
    user: &Felt,
    viewing_key: &SecretFelt,
    anonymizer: &Felt,
    dapp_name: &Felt,
    nonce: Option<&Felt>,
) -> Felt {
    let partial = partial_commitment(user, viewing_key, anonymizer, dapp_name);
    match nonce {
        Some(nonce) => commitment(&partial, nonce),
        None => partial,
    }
}

/// The wallet's bounds on a `get_shadow_accounts` scan (Bramble's
/// `STRK20_SHADOW_VIEW_NONCE_LIMIT` and the anonymizer's `MAX_SCAN_RANGE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShadowScanLimits {
    /// Nonces at or above this are refused: no view window can include them.
    pub nonce_limit: u64,
    /// The most nonces one view call covers.
    pub max_scan_range: u64,
}

/// Why a shadow-account read failed. Carries nothing the node sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShadowReadRefusal {
    /// A nonce at or past the limit ("a nonce is outside the anonymizer's range").
    NonceOutOfRange,
    /// The node answered with an error, or not to this request.
    Unavailable,
    /// The reply is not the window's `Span<ShadowAccountInfo>`.
    Malformed,
}

/// One shadow account the anonymizer lists (`Strk20ShadowAccountInfo`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShadowAccountInfo {
    pub nonce: u64,
    pub address: Felt,
    pub deployed: bool,
}

/// `nonceWindows`: the distinct nonces, sorted, as contiguous `[start, end)` windows of at most
/// `max_scan_range` each, so a gap between nonces costs nothing. Refused if any nonce is at or
/// past `nonce_limit`.
pub fn nonce_windows(
    nonces: &[Felt],
    limits: &ShadowScanLimits,
) -> Result<Vec<(u64, u64)>, ShadowReadRefusal> {
    let mut sorted = nonces
        .iter()
        .map(|nonce| {
            u64::try_from(*nonce)
                .ok()
                .filter(|nonce| *nonce < limits.nonce_limit)
                .ok_or(ShadowReadRefusal::NonceOutOfRange)
        })
        .collect::<Result<Vec<u64>, _>>()?;
    sorted.sort_unstable();
    sorted.dedup();
    let mut windows: Vec<(u64, u64)> = Vec::new();
    for nonce in sorted {
        match windows.last_mut() {
            Some((start, end)) if nonce == *end && *end - *start < limits.max_scan_range => {
                *end = nonce + 1;
            }
            // `nonce < nonce_limit <= u64::MAX`, so `nonce + 1` does not overflow.
            _ => windows.push((nonce, nonce + 1)),
        }
    }
    Ok(windows)
}

/// `get_shadow_accounts(partial, start, end, 0)` on the anonymizer at `latest`, under JSON-RPC
/// `id`.
pub fn get_shadow_accounts_body(
    id: u64,
    anonymizer: &Felt,
    partial: &Felt,
    window: (u64, u64),
) -> String {
    rpc_bodies::call(
        id,
        anonymizer,
        &selector(GET_SHADOW_ACCOUNTS),
        &[
            *partial,
            Felt::from(window.0),
            Felt::from(window.1),
            Felt::ZERO,
        ],
    )
}

/// `decodeInfos`: the reply to `get_shadow_accounts_body` under `id` must be one
/// `(nonce, address, is_deployed)` per nonce of `window`, in order, each address a nonzero felt
/// (`isNonZeroStarkFieldElement`; every entry must parse as a felt) and each flag 0 or 1.
pub fn judge_shadow_accounts(
    body: &str,
    id: u64,
    window: (u64, u64),
) -> Result<Vec<ShadowAccountInfo>, ShadowReadRefusal> {
    let felts = call_felts(answer(body, id)).map_err(|node_error| {
        if node_error {
            ShadowReadRefusal::Unavailable
        } else {
            ShadowReadRefusal::Malformed
        }
    })?;
    let malformed = ShadowReadRefusal::Malformed;
    let (length, entries) = felts.split_first().ok_or(malformed)?;
    let expected = window.1.checked_sub(window.0).ok_or(malformed)?;
    if *length != Felt::from(expected) || entries.len() as u64 != expected.saturating_mul(3) {
        return Err(malformed);
    }
    entries
        .chunks_exact(3)
        .zip(window.0..window.1)
        .map(|(entry, nonce)| match entry {
            [got, address, deployed]
                if *got == Felt::from(nonce)
                    && *address != Felt::ZERO
                    && (*deployed == Felt::ZERO || *deployed == Felt::ONE) =>
            {
                Ok(ShadowAccountInfo {
                    nonce,
                    address: *address,
                    deployed: *deployed == Felt::ONE,
                })
            }
            _ => Err(malformed),
        })
        .collect()
}

#[cfg(test)]
#[path = "shadow_reads_tests.rs"]
mod tests;
