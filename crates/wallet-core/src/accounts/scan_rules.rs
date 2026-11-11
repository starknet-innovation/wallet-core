//! What the discovery scan walks and keeps: the wallet's `ScanConfig` (shapes in order, limits,
//! unusable indexes, tokens, pool and signing classes), the candidates it finds, and the keep and
//! gap-limit rules (the extension's `keepsRecoveryCandidate` and `recoveryScanTarget`). The state
//! machine that applies them is `scan.rs`, which re-exports these.

use starknet_types_core::felt::Felt;

use super::advisory::Restriction;
use super::{AccountFamily, ArgentShape, Ladder};
use crate::kms::NetworkId;
use crate::rpc::Request;
use crate::send::types::Authorization;

/// The deployment salt a shape places its key with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaltRule {
    Zero,
    PublicKey,
}

/// One scan shape: a family, the key tree it walks, the class (and Argent layout) it places the
/// key under, its salt, and whether the scan lists it with no history (`isCreatableCandidate`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanShape {
    pub family: AccountFamily,
    pub ladder: Ladder,
    pub class_hash: Felt,
    pub argent_shape: Option<ArgentShape>,
    pub salt: SaltRule,
    pub creatable: bool,
}

/// Discovery bounds (the extension's `RECOVERY_SCAN_LIMITS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanLimits {
    pub batch_size: u32,
    pub default_count: u32,
    pub max_index: u32,
    /// Ceiling on indices one scan covers, so an endpoint that reports every probe deployed
    /// cannot extend the gap-limit walk forever.
    pub max_scanned_indexes: u32,
}

/// Everything the scan reads that is the wallet's choice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanConfig {
    pub network: NetworkId,
    /// In scan order.
    pub shapes: Vec<ScanShape>,
    pub limits: ScanLimits,
    /// Indexes never derived, scanned or checked by hand (besides those above `max_index`).
    pub reserved_indexes: Vec<u32>,
    /// ERC-20s whose balance counts as funded.
    pub tokens: Vec<Felt>,
    /// The network's privacy pool, whose registered viewing key a deployed account is checked
    /// against; `None` when the network has none.
    pub privacy_pool: Option<Felt>,
    /// The live classes this wallet signs for: the send plan's `Authorization` data.
    pub authorizations: Vec<Authorization>,
}

impl ScanConfig {
    pub(crate) fn usable(&self, index: u32) -> bool {
        index <= self.limits.max_index && !self.reserved_indexes.contains(&index)
    }
}

/// A candidate found by the scan. Public data only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub family: AccountFamily,
    pub ladder: Ladder,
    pub class_hash: Felt,
    pub account_index: u32,
    pub address: Felt,
    pub deployed: bool,
    pub funded: bool,
    /// The advisory signability verdict for a deployed account; `None` when there is none or
    /// the reads did not answer. The send path re-checks authoritatively either way.
    pub restriction: Option<Restriction>,
    /// Another wallet's viewing key is registered for this account in the privacy pool.
    pub privacy_elsewhere: bool,
}

/// Where the scan stands after a call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanStep {
    /// Bodies to POST and return through `Scan::submit`; empty once a batch is judged.
    pub requests: Vec<Request>,
    pub candidates: Vec<Found>,
    /// Indices checked so far (the extension's `nextIndex`).
    pub checked: u32,
    pub complete: bool,
    /// How many indices the scan must cover (`scan_target`).
    pub target: u32,
}

/// Whether the scan keeps a candidate: on-chain evidence, or a creatable account at index 0 or at
/// a manually checked index.
pub fn keep(found: &Found, creatable: bool, manual: bool) -> bool {
    found.deployed || found.funded || (creatable && (found.account_index == 0 || manual))
}

/// How many indices the scan must cover given what it has kept (the extension's
/// `recoveryScanTarget`): at least `default_count`, and always `default_count` past the highest
/// index with on-chain evidence, so a ladder of real accounts keeps the search going and a quiet
/// tail ends it. Creatable defaults extend nothing. Bounded by `max_scanned_indexes`.
pub fn scan_target(candidates: &[Found], limits: &ScanLimits) -> u32 {
    let reach = candidates
        .iter()
        .filter(|found| found.deployed || found.funded)
        .map(|found| {
            found
                .account_index
                .saturating_add(1)
                .saturating_add(limits.default_count)
        })
        .max()
        .unwrap_or(0);
    reach
        .max(limits.default_count)
        .min(limits.max_scanned_indexes)
}
