//! The checks the extension's `importIntent` (`apps/extension/src/background/multisig/
//! multisig-tx-ops.ts`) makes between reading a co-signer's package and storing it, after
//! `IntentPackage::parse` and `SigningIntent::locally_derived_imported`: the chain still has the
//! intent's authority and nonce (`currentArgentIntentContext` in `multisig-intent-context.ts`,
//! with `markStale` false), the calls pass this wallet's own review, which replaces the remote
//! summary (`importedIntentReview`), a stored intent under the same id is the same transaction
//! and is merged into (`mergeImportedArgentMultisigIntent`), and the result is one the store
//! would accept (`upsertIntentIn` in `packages/features/multisig/src/repository.ts`).
//!
//! Sans-IO: the host reads the authority and nonce and passes them in. Nothing here signs or
//! holds a key. Left to the wallet: that the session is still current, that the record is on the
//! active network, and whether its intent store has room (`intentPlacement`).

use std::fmt;

use starknet_types_core::felt::Felt;

use super::call_decode::Known;
use super::multisig_intent::{ConfigSnapshot, IntentPackage, SigningIntent};
use super::multisig_invoke::StarkPrimitives;
use super::multisig_review::{review_calls, CallReviewError};
use crate::accounts::multisig::ArgentMultisigClass;
use crate::accounts::multisig_records::{authority, PublicConfig, Record};
use crate::felt;
use crate::js::felt::same_felt;

/// Why an import was refused. `Display` is the extension's sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportError {
    /// The intent's account is not the record passed in (`contextForIntent`).
    UnknownAccount,
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

/// `importIntent` from a parsed package to the intent to store. `threshold` and `signer_guids`
/// are the record's `MultisigAuthorityRead::authority` (status `Available`; any other status is
/// the wallet's refusal), and `nonce` its `starknet_getNonce` (`judge::nonce`), both read just
/// before this call.
///
/// The remote's lifecycle claims are dropped (`locally_derived_imported`), its summary is
/// replaced by this wallet's review of the frozen calls, and a stored intent is merged into
/// (`merge_imported`, which keeps the stored summary). Refusals come in the extension's order; a
/// stored intent `merge_imported` refuses (another snapshot) is `Conflict`, the answer the
/// extension's store gives after its looser merge.
pub fn import_intent(
    package: &IntentPackage,
    threshold: u32,
    signer_guids: &[Felt],
    nonce: &Felt,
    wallet: &ImportContext<'_>,
) -> Result<SigningIntent, ImportError> {
    let (record, class) = (wallet.record, wallet.class);
    let mut incoming = package.intent.locally_derived_imported();
    if record.id != incoming.account_id {
        return Err(ImportError::UnknownAccount);
    }
    if ConfigSnapshot::new(class, threshold, signer_guids) != incoming.config_snapshot {
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
