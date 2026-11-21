//! The checks the extension's `importIntent` (`apps/extension/src/background/multisig/
//! multisig-tx-ops.ts`) makes between reading a co-signer's package and storing it, after
//! `IntentPackage::parse` and `SigningIntent::locally_derived_imported`: the chain still has the
//! intent's authority and nonce (`currentArgentIntentContext` in `multisig-intent-context.ts`,
//! with `markStale` false), the calls pass this wallet's own review, which replaces the remote
//! summary (`importedIntentReview`), a stored intent under the same id is the same transaction
//! and is merged into (`mergeImportedArgentMultisigIntent`), and the result is one the store
//! would accept (`upsertIntentIn` in `packages/features/multisig/src/repository.ts`).
//!
//! Sans-IO: the host drives the authority read and reads the nonce, and passes both in. Nothing
//! here signs or holds a key. Left to the wallet: that the session is still current, that the
//! record is on the active network, and whether its intent store has room (`intentPlacement`).

use std::fmt;

use starknet_types_core::felt::Felt;

use super::call_decode::Known;
use super::multisig_intent::{ConfigSnapshot, IntentPackage, SigningIntent};
use super::multisig_invoke::StarkPrimitives;
use super::multisig_review::{review_calls, CallReviewError};
use crate::accounts::multisig::ArgentMultisigClass;
use crate::accounts::multisig_read::MultisigAuthorityRead;
use crate::accounts::multisig_records::{authority, PublicConfig, Record};
use crate::felt;
use crate::js::felt::same_felt;
use crate::kms::NetworkId;

/// Why an import was refused. `Display` is the extension's sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportError {
    /// The intent's account is not the record passed in (`contextForIntent`), by id or by the
    /// frozen transaction's sender and chain (the latter two the extension's record lookup
    /// guarantees).
    UnknownAccount,
    /// The authority read targets another address, network or class than the record's. No
    /// extension counterpart: its read is built from the record, so cannot miss it.
    ReadForAnotherAccount,
    /// The authority read did not end `Available`. The extension shows whatever its read threw;
    /// this is its fixed fallback sentence (`currentArgentIntentContext`'s catch).
    AuthorityUnavailable,
    /// The chain's authority is not the intent's snapshot (`sameConfig`).
    ConfigChanged,
    /// The chain's nonce is not the prepared one (`NONCE_NOT_CURRENT`).
    NonceNotCurrent,
    /// This wallet's review refused the calls (`reviewArgentMultisigCalls`).
    Review(CallReviewError),
    /// A stored intent under this id froze another transaction (`importedIntentReview`).
    ConflictingId,
    /// The intent to store fails the validator (`upsertIntentIn`'s "invalid").
    Invalid,
    /// The stored intent may not be replaced by it (`upsertIntentIn`'s "conflict").
    Conflict,
}

impl ImportError {
    pub fn message(self) -> &'static str {
        match self {
            Self::UnknownAccount => "Argent multisig not found",
            Self::ReadForAnotherAccount => "Argent multisig authority read is for another account",
            Self::AuthorityUnavailable => "Argent multisig config unavailable",
            Self::ConfigChanged => "Argent multisig intent is stale after a configuration change",
            Self::NonceNotCurrent => "Argent multisig intent nonce is no longer current",
            Self::Review(error) => error.message(),
            Self::ConflictingId => "conflicting Argent multisig intent id",
            Self::Invalid => "Argent multisig intent invalid",
            Self::Conflict => "Argent multisig intent conflict",
        }
    }
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for ImportError {}

/// What the wallet holds for an import: the watched `record` whose id is the intent's account
/// (validated with `is_record`), the intent it already stores under the incoming id, and its
/// `review_calls` arguments (`known`, `trusted`, `summary_length`, as for a proposal).
pub struct ImportContext<'a> {
    pub record: &'a Record,
    pub stored: Option<&'a SigningIntent>,
    pub class: &'a ArgentMultisigClass,
    pub known: &'a Known<'a>,
    pub trusted: &'a [Felt],
    pub summary_length: usize,
    pub primitives: &'a dyn StarkPrimitives,
}

/// `importIntent` from a parsed package to the intent to store. `read` is the record's
/// authority read, driven to its end just before this call: it must target the record's address
/// and network and `class`, and be `Available`. Since the read refuses an address running any
/// other class, the snapshot's class hash is then the one on chain. `nonce` is the record's
/// `starknet_getNonce` (`judge::nonce`); a bare felt, since no read binds it to an address here.
///
/// The remote's lifecycle claims are dropped (`locally_derived_imported`), its summary is
/// replaced by this wallet's review of the frozen calls, and a stored intent is merged into
/// (`merge_imported`, which keeps the stored summary). Refusals come in the extension's order; a
/// stored intent `merge_imported` refuses (another snapshot) is `Conflict`, the answer the
/// extension's store gives after its looser merge.
pub fn import_intent(
    package: &IntentPackage,
    read: &MultisigAuthorityRead,
    nonce: &Felt,
    wallet: &ImportContext<'_>,
) -> Result<SigningIntent, ImportError> {
    let (record, class) = (wallet.record, wallet.class);
    let mut incoming = package.intent.locally_derived_imported();
    // The record is the frozen transaction's account, not only by id: the review below and the
    // authority read both use its address and network, which `is_valid` never compares.
    let prepared = &incoming.prepared;
    let on_chain = NetworkId::from_catalog_id(&record.network_id).is_some_and(|network| {
        same_felt(&felt::canonical(&network.chain_id()), &prepared.chain_id)
    });
    if record.id != incoming.account_id
        || !same_felt(&record.address, &prepared.sender_address)
        || !on_chain
    {
        return Err(ImportError::UnknownAccount);
    }
    if !same_felt(&read.address(), &record.address)
        || read.network().catalog_id() != record.network_id
        || read.class_hash() != class.hash()
    {
        return Err(ImportError::ReadForAnotherAccount);
    }
    let (threshold, signer_guids) = read.authority().ok_or(ImportError::AuthorityUnavailable)?;
    if ConfigSnapshot::new(class, threshold, &signer_guids) != incoming.config_snapshot {
        return Err(ImportError::ConfigChanged);
    }
    if !same_felt(&felt::canonical(nonce), &incoming.prepared.nonce) {
        return Err(ImportError::NonceNotCurrent);
    }
    let review = review_calls(
        &record.address,
        &incoming.prepared.calls,
        held_config(record, &incoming.config_snapshot, class).as_ref(),
        class,
        wallet.known,
        wallet.trusted,
        wallet.summary_length,
    )
    .map_err(ImportError::Review)?;
    incoming.summary = review.summary;
    let next = match wallet.stored {
        Some(stored) if stored.prepared != incoming.prepared => {
            return Err(ImportError::ConflictingId)
        }
        Some(stored) => stored
            .merge_imported(&incoming)
            .ok_or(ImportError::Conflict)?,
        None => incoming,
    };
    let (length, primitives) = (wallet.summary_length, wallet.primitives);
    if !next.is_valid(class, length, primitives) {
        return Err(ImportError::Invalid);
    }
    if wallet
        .stored
        .is_some_and(|stored| !stored.can_replace(&next, class, length, primitives))
    {
        return Err(ImportError::Conflict);
    }
    Ok(next)
}

/// `storedArgentPublicConfig` when `matchesArgentPublicConfig` holds against the chain (here the
/// snapshot, already checked equal to it), else `None`: a signer change needs the keys behind
/// the GUIDs, and only the record has them.
fn held_config(
    record: &Record,
    chain: &ConfigSnapshot,
    class: &ArgentMultisigClass,
) -> Option<PublicConfig> {
    authority(record, class).filter(|held| {
        held.threshold == chain.threshold && held.signer_guids == chain.signer_guids
    })?;
    record.current_config.clone().or_else(|| {
        record.deployment.as_ref().map(|deployment| PublicConfig {
            signer_public_keys: deployment.signer_public_keys.clone(),
            threshold: deployment.threshold,
        })
    })
}
