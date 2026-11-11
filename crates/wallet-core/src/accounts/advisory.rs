//! The discovery scan's advisory reads for a deployed candidate, reproducing the extension's
//! `argentSigningRestriction`, `braavosSigningRestriction` (issue 542) and
//! `strk20RegisteredElsewhere` (the privacy-pool note). Every read is at `latest`, as the
//! extension's scan reads.
//!
//! Advisory only: a read that fails or answers malformed leaves the account unlabelled and never
//! pauses the scan. A wrong or missing signer verdict can mislabel an account but never move
//! funds, because the send path (`send::plan_probe`) re-checks authoritatively.
//!
//! Which live classes this wallet signs for is the same `send::Authorization` data the send plan
//! takes, so a wallet's class lists are written once.

use std::collections::HashMap;

use serde_json::json;
use starknet_types_core::felt::Felt;

use super::scan::{Found, ScanConfig};
use super::{AccountFamily, Ladder};
use crate::kms::{self, NetworkId};
use crate::rpc::{self, Request};
use crate::send::judge;
use crate::send::plan_probe::{self as probe, braavos_signers, selector};
use crate::send::types::Authorization;
use crate::{felt, Error};

/// `RecoverySigningRestriction`: why this wallet could not sign for a deployed account.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Restriction {
    Guardian,
    OwnerChanged,
    OwnerType,
    Release,
    StrongSigner,
}

/// One advisory `starknet_call`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Read {
    Owner,
    OwnerType,
    Guardian,
    Signers,
    Deferred,
    PoolKey,
}

/// What the reads gathered for one deployed candidate: its live class from the class-hash round,
/// and each read's felts, `None` where the read did not answer.
#[derive(Debug, Default)]
pub struct Gathered {
    pub live_class: Option<Felt>,
    pub reads: HashMap<Read, Option<Vec<Felt>>>,
}

impl Gathered {
    fn get(&self, read: Read) -> Option<&[Felt]> {
        self.reads.get(&read)?.as_deref()
    }
}

/// Whether this wallet signs for an account of `family` running `live_class`, by the wallet's
/// `authorizations`: an Argent class whose authorization is the signer array
/// (`requireAuthorizableArgentClass`) or a Braavos implementation (`isBraavosSigningClass`).
fn signs_for(authorizations: &[Authorization], family: AccountFamily, live_class: &Felt) -> bool {
    authorizations
        .iter()
        .any(|authorization| match (family, authorization) {
            (AccountFamily::Argent, Authorization::ArgentSignerArray { classes }) => classes
                .iter()
                .any(|class| class.class_hash == *live_class && class.signer_array),
            (
                AccountFamily::Braavos,
                Authorization::Braavos {
                    signing_classes, ..
                },
            ) => signing_classes.contains(live_class),
            _ => false,
        })
}

/// The reads one deployed candidate needs: its signer views when its live class is one this
/// wallet signs for (otherwise the verdict is `Release` with no read), and the pool's key for its
/// address when the network has a pool.
pub fn reads(
    family: AccountFamily,
    live_class: &Felt,
    authorizations: &[Authorization],
    pool: bool,
) -> Vec<Read> {
    let mut reads = match family {
        _ if !signs_for(authorizations, family, live_class) => Vec::new(),
        AccountFamily::Argent => vec![Read::Owner, Read::OwnerType, Read::Guardian],
        AccountFamily::Braavos => vec![Read::Signers, Read::Deferred],
        AccountFamily::OpenZeppelin => Vec::new(),
    };
    if pool {
        reads.push(Read::PoolKey);
    }
    reads
}

/// Whether the Braavos verdict turns on the latest block's timestamp: this wallet's key is the
/// first Stark signer, a strong signer is active, and its removal was requested.
pub fn needs_block(
    family: AccountFamily,
    gathered: &Gathered,
    public_key: &Felt,
    authorizations: &[Authorization],
) -> bool {
    let removal_requested = || {
        let (first_stark, strong) = braavos_signers(gathered.get(Read::Signers)?)?;
        let expire_at = gathered.get(Read::Deferred)?.first().copied();
        Some(
            first_stark == Some(*public_key)
                && strong
                && expire_at.is_some_and(|at| at != Felt::ZERO),
        )
    };
    family == AccountFamily::Braavos
        && gathered
            .live_class
            .is_some_and(|class| signs_for(authorizations, family, &class))
        && removal_requested() == Some(true)
}

/// The `starknet_call` body for `read` at `latest`. `pool` is required for `Read::PoolKey`.
fn body(id: u64, read: Read, address: &Felt, pool: &Felt) -> String {
    let (contract, entry_point, calldata) = match read {
        Read::Owner => (address, probe::GET_OWNER, vec![]),
        Read::OwnerType => (address, probe::GET_OWNER_TYPE, vec![]),
        Read::Guardian => (address, probe::GET_GUARDIAN, vec![]),
        Read::Signers => (address, probe::GET_SIGNERS, vec![]),
        Read::Deferred => (address, probe::GET_DEFERRED_REMOVE_SIGNERS, vec![]),
        Read::PoolKey => (pool, probe::GET_PUBLIC_KEY, vec![felt::canonical(address)]),
    };
    rpc::envelope(
        id,
        "starknet_call",
        json!({
            "block_id": "latest",
            "request": {
                "calldata": calldata,
                "contract_address": felt::canonical(contract),
                "entry_point_selector": felt::canonical(&selector(entry_point)),
            },
        }),
    )
}

/// A block lists every transaction hash (about 70 bytes each), so a busy mainnet block passes the
/// default 64 KiB bound; 1 MiB holds roughly 15,000.
pub const BLOCK_RESPONSE_BYTES: usize = 1024 * 1024;

/// One advisory `starknet_call` for `read`. Optional: a failed POST answers empty, never pauses.
pub fn request(id: u64, read: Read, address: &Felt, pool: &Felt) -> Request {
    Request::optional(id, body(id, read, address, pool), rpc::MAX_RESPONSE_BYTES)
}

/// `starknet_getBlockWithTxHashes` at `latest`, for its timestamp, bounded at
/// [`BLOCK_RESPONSE_BYTES`] and optional like every advisory read.
pub fn block_request(id: u64) -> Request {
    let body = rpc::envelope(
        id,
        "starknet_getBlockWithTxHashes",
        json!({ "block_id": "latest" }),
    );
    Request::optional(id, body, BLOCK_RESPONSE_BYTES)
}

/// A call's felts, or `None` for an error, a malformed reply or a foreign id.
pub fn judge_call(body: &str, id: u64) -> Option<Vec<Felt>> {
    judge::call_felts(judge::answer(body, id)).ok()
}

/// A block's timestamp, or `None` when the reply is unusable.
pub fn judge_block(body: &str, id: u64) -> Option<u64> {
    judge::block_timestamp(judge::answer_within(body, id, BLOCK_RESPONSE_BYTES)).ok()
}

/// `argentSigningRestriction` / `braavosSigningRestriction`: the first reason this wallet could
/// not sign for the account, or `None` when it could or a read did not answer.
pub fn restriction(
    family: AccountFamily,
    gathered: &Gathered,
    public_key: &Felt,
    block_time: Option<u64>,
    authorizations: &[Authorization],
) -> Option<Restriction> {
    let live_class = gathered.live_class.as_ref()?;
    if family == AccountFamily::OpenZeppelin {
        return None;
    }
    if !signs_for(authorizations, family, live_class) {
        return Some(Restriction::Release);
    }
    if family == AccountFamily::Argent {
        let (owner, owner_type, guardian) = (
            gathered.get(Read::Owner)?,
            gathered.get(Read::OwnerType)?,
            gathered.get(Read::Guardian)?,
        );
        return if owner != [*public_key] {
            Some(Restriction::OwnerChanged)
        } else if owner_type != [Felt::ZERO] {
            Some(Restriction::OwnerType)
        } else if guardian != [Felt::ZERO] {
            Some(Restriction::Guardian)
        } else {
            None
        };
    }
    let (first_stark, strong) = braavos_signers(gathered.get(Read::Signers)?)?;
    if first_stark != Some(*public_key) {
        return Some(Restriction::OwnerChanged);
    }
    if !strong {
        return None;
    }
    // `braavosSignerVerdict`: a requested removal counts only once the block is strictly after
    // `expire_at`.
    let expire_at = gathered
        .get(Read::Deferred)?
        .first()
        .copied()
        .unwrap_or(Felt::ZERO);
    if expire_at == Felt::ZERO {
        return Some(Restriction::StrongSigner);
    }
    let expired = Felt::from(block_time?).to_biguint() > expire_at.to_biguint();
    (!expired).then_some(Restriction::StrongSigner)
}

/// `strk20RegisteredElsewhere`: the pool holds a non-zero key for the address that is not the
/// one this wallet would register. `own` is derived only then; a failed derivation answers false.
pub fn registered_elsewhere(gathered: &Gathered, own: impl FnOnce() -> Option<Felt>) -> bool {
    match gathered.get(Read::PoolKey) {
        Some([registered]) if *registered != Felt::ZERO => {
            own().is_some_and(|own| own != *registered)
        }
        _ => false,
    }
}

/// The public viewing key this wallet would register for the account at `index` on `ladder` in
/// `pool` (`previewStrk20ViewingPublicKey`): the account key and its viewing key scoped to
/// `"<chain_id>:<pool>"` are derived and dropped inside this call, and only the public
/// x-coordinate leaves it.
pub fn own_viewing_public_key(
    phrase: &str,
    passphrase: Option<&str>,
    ladder: Ladder,
    index: u32,
    network: NetworkId,
    pool: &Felt,
) -> Result<Felt, Error> {
    let key = kms::account_key(phrase, passphrase, ladder, index)?;
    let chain_id = felt::canonical(&network.chain_id());
    let viewing_key = kms::derive_strk20_viewing_key(&key, &chain_id, &felt::canonical(pool))?;
    let public = kms::stark_public_key(&viewing_key)?;
    felt::parse(&public).ok_or_else(|| Error::kms("viewing public key"))
}

/// Settles one deployed candidate's advisory verdicts. The own viewing key is derived only for
/// an account the pool holds a non-zero key for.
pub(crate) fn settle(
    found: &mut Found,
    gathered: &Gathered,
    public_key: &Felt,
    block_time: Option<u64>,
    phrase: &str,
    passphrase: Option<&str>,
    config: &ScanConfig,
) {
    found.restriction = restriction(
        found.family,
        gathered,
        public_key,
        block_time,
        &config.authorizations,
    );
    let Some(pool) = config.privacy_pool else {
        return;
    };
    let (ladder, account_index) = (found.ladder, found.account_index);
    found.privacy_elsewhere = registered_elsewhere(gathered, || {
        own_viewing_public_key(
            phrase,
            passphrase,
            ladder,
            account_index,
            config.network,
            &pool,
        )
        .ok()
    });
}

#[cfg(test)]
#[path = "advisory_tests.rs"]
mod tests;
