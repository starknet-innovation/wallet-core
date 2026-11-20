//! The shared signing intent around a frozen multisig transaction, mirroring the extension's
//! `packages/features/multisig/src/repository-schema.ts` (`ArgentMultisigSigningIntent`,
//! `argentMultisigIntentId`, `isArgentMultisigSigningIntent`,
//! `canReplaceArgentMultisigSigningIntent`) and the background's
//! `multisig-inputs.ts` (`argentMultisigIntentPackage`, `readImportIntentInput`,
//! `mergeImportedArgentMultisigIntent`) and
//! `multisig-ops.ts` (`configSnapshot`). Field order is the extension's, so an intent and its
//! package serialise to the bytes a co-signer's extension writes.
//!
//! Who may co-sign, the summary's length and how an intent travels (Nostr) are the wallet's:
//! the class and limit come in as arguments.

use serde::{Deserialize, Serialize};
use starknet_types_core::felt::Felt;

use super::multisig_invoke::{
    canonical_non_zero, is_calls, MultisigSignature, PreparedInvoke, StarkPrimitives,
};
use crate::accounts::multisig::{signer_guid, ArgentMultisigClass};
use crate::accounts::multisig_records::{is_authority, not_null};
use crate::felt;
use crate::history::explorer::canonical_account_id;
use crate::js::felt::{canonical_felt, same_felt};
use crate::js::num::js_trim;
use crate::kms::{utf16_length, NetworkId};

/// `ArgentMultisigConfigSnapshot`: the class and signer authority read fresh from the chain when
/// the intent was proposed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigSnapshot {
    pub class_hash: String,
    pub signer_guids: Vec<String>,
    pub threshold: u32,
}

impl ConfigSnapshot {
    /// `configSnapshot`: a chain authority read (`MultisigAuthorityRead::authority`) for `class`.
    pub fn new(class: &ArgentMultisigClass, threshold: u32, signer_guids: &[Felt]) -> Self {
        Self {
            class_hash: class.class_hash().to_string(),
            signer_guids: signer_guids.iter().map(felt::canonical).collect(),
            threshold,
        }
    }
}

/// `ArgentMultisigIntentState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IntentState {
    Accepted,
    Collecting,
    Ready,
    Rejected,
    Stale,
    Submitted,
}

/// `ArgentMultisigStaleOutcome`: what the chain said about a stale version's hash (local only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StaleOutcome {
    Rejected,
    RejectedNonceUnused,
    Reverted,
}

/// `ArgentMultisigStaleReason`: why an intent went stale (local only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StaleReason {
    Config,
    Fee,
    Nonce,
    Proof,
}

/// `ArgentMultisigSigningIntent`, in the extension's key order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SigningIntent {
    pub account_id: String,
    #[serde(
        default,
        deserialize_with = "not_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub broadcast_transaction_hash: Option<String>,
    pub config_snapshot: ConfigSnapshot,
    pub id: String,
    pub prepared: PreparedInvoke,
    pub signatures: Vec<MultisigSignature>,
    #[serde(
        default,
        deserialize_with = "not_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub stale_outcome: Option<StaleOutcome>,
    #[serde(
        default,
        deserialize_with = "not_null",
        skip_serializing_if = "Option::is_none"
    )]
    pub stale_reason: Option<StaleReason>,
    pub state: IntentState,
    pub summary: String,
}

/// The package kind and the only version this core writes or reads.
pub const INTENT_PACKAGE_KIND: &str = "bramble.argent-multisig-intent";
pub const INTENT_PACKAGE_VERSION: u32 = 1;

/// `ArgentMultisigIntentPackage`: what co-signers exchange. Read one only through
/// [`IntentPackage::parse`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IntentPackage {
    pub intent: SigningIntent,
    pub kind: String,
    pub version: u32,
}

/// The package as it arrives, before its envelope is checked.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Arrived {
    intent: SigningIntent,
    kind: String,
    version: u32,
}

impl IntentPackage {
    /// `readImportIntentInput`'s package checks: only `intent`, `kind` and `version`, exactly
    /// this kind and version 1, and an intent [`SigningIntent::is_valid`] accepts. `None`
    /// otherwise.
    pub fn parse(
        text: &str,
        class: &ArgentMultisigClass,
        max_summary_length: usize,
        primitives: &dyn StarkPrimitives,
    ) -> Option<Self> {
        let arrived: Arrived = serde_json::from_str(text).ok()?;
        (arrived.kind == INTENT_PACKAGE_KIND
            && arrived.version == INTENT_PACKAGE_VERSION
            && arrived
                .intent
                .is_valid(class, max_summary_length, primitives))
        .then_some(Self {
            intent: arrived.intent,
            kind: arrived.kind,
            version: arrived.version,
        })
    }

    /// Compact JSON, as `JSON.stringify` writes it.
    pub fn to_json(&self) -> String {
        // Plain strings, numbers and arrays: serialising this cannot fail.
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// `argentMultisigIntentId`: `<account id>:<canonical hash>`, `None` for a malformed account id
/// or hash.
pub fn intent_id(account_id: &str, transaction_hash: &str) -> Option<String> {
    let hash = canonical_felt(transaction_hash)?;
    (canonical_account_id(account_id).as_deref() == Some(account_id))
        .then(|| format!("{account_id}:{hash}"))
}

fn guid_of(public_key: &str) -> Option<Felt> {
    signer_guid(&canonical_non_zero(public_key)?).ok()
}

impl SigningIntent {
    /// The collecting intent the background's `prepareIntent` stores for `prepared`: no
    /// signatures yet. `None` when the account id or hash is malformed.
    pub fn proposed(
        account_id: &str,
        config_snapshot: ConfigSnapshot,
        prepared: PreparedInvoke,
        summary: &str,
    ) -> Option<Self> {
        Some(Self {
            account_id: account_id.to_string(),
            broadcast_transaction_hash: None,
            config_snapshot,
            id: intent_id(account_id, &prepared.transaction_hash)?,
            prepared,
            signatures: Vec::new(),
            stale_outcome: None,
            stale_reason: None,
            state: IntentState::Collecting,
            summary: summary.to_string(),
        })
    }

    /// `argentMultisigIntentPackage`: the stale reason and outcome are local and stay behind
    /// (older co-signers refuse unknown keys).
    pub fn package(&self) -> IntentPackage {
        IntentPackage {
            intent: Self {
                stale_outcome: None,
                stale_reason: None,
                ..self.clone()
            },
            kind: INTENT_PACKAGE_KIND.into(),
            version: INTENT_PACKAGE_VERSION,
        }
    }

    /// `mergeImportedArgentMultisigIntent`: a re-import of the same frozen hash. A terminal or
    /// stale local intent is kept as it is; an open one gains the incoming signers it lacks, in
    /// GUID order, and keeps its own summary. Both must already have passed [`is_valid`].
    ///
    /// [`is_valid`]: SigningIntent::is_valid
    pub fn merge_imported(&self, incoming: &SigningIntent) -> SigningIntent {
        if !matches!(self.state, IntentState::Collecting | IntentState::Ready) {
            return self.clone();
        }
        let mut signatures = self.signatures.clone();
        for signature in &incoming.signatures {
            if !signatures
                .iter()
                .any(|stored| stored.public_key == signature.public_key)
            {
                signatures.push(signature.clone());
            }
        }
        signatures.sort_by_key(|signature| guid_of(&signature.public_key));
        let ready = signatures.len() == incoming.config_snapshot.threshold as usize;
        SigningIntent {
            signatures,
            state: if ready {
                IntentState::Ready
            } else {
                IntentState::Collecting
            },
            summary: self.summary.clone(),
            ..incoming.clone()
        }
    }

    /// `isArgentMultisigSigningIntent`: the snapshot is `class`'s, the prepared transaction
    /// re-validates (calldata and hash recomputed), the id, account and chain agree with it, the
    /// summary is trimmed and at most `max_summary_length` UTF-16 units, every signature is a
    /// snapshot member's over the hash in ascending GUID order, and the count fits the state.
    pub fn is_valid(
        &self,
        class: &ArgentMultisigClass,
        max_summary_length: usize,
        primitives: &dyn StarkPrimitives,
    ) -> bool {
        let snapshot = &self.config_snapshot;
        let summary_ok = (1..=max_summary_length).contains(&utf16_length(&self.summary))
            && js_trim(&self.summary) == self.summary;
        (self.stale_outcome.is_none() || self.state == IntentState::Stale)
            && canonical_account_id(&self.account_id).as_deref() == Some(self.account_id.as_str())
            && snapshot.class_hash == class.class_hash()
            && is_authority(&snapshot.signer_guids, snapshot.threshold, class)
            && self.prepared.is_valid(primitives)
            && summary_ok
            && intent_id(&self.account_id, &self.prepared.transaction_hash).as_ref()
                == Some(&self.id)
            && self.scope_matches_prepared()
            && self.signatures_valid(primitives)
            && self.authority_fits_state()
    }

    /// `canReplaceArgentMultisigSigningIntent`: whether `next` may overwrite this stored intent.
    /// Both must be valid, every reviewed and hash-bound field unchanged, no signature dropped,
    /// and the state move one the lifecycle allows (`validStateTransition`).
    pub fn can_replace(
        &self,
        next: &SigningIntent,
        class: &ArgentMultisigClass,
        max_summary_length: usize,
        primitives: &dyn StarkPrimitives,
    ) -> bool {
        use IntentState::{Accepted, Collecting, Ready, Rejected, Stale, Submitted};
        let allowed: &[IntentState] = match self.state {
            Accepted => &[Accepted],
            Collecting => &[Collecting, Ready, Stale],
            Ready => &[Accepted, Ready, Rejected, Stale, Submitted],
            Rejected => &[Rejected],
            Stale => &[Stale, Submitted],
            Submitted => &[Accepted, Rejected, Submitted],
        };
        self.is_valid(class, max_summary_length, primitives)
            && next.is_valid(class, max_summary_length, primitives)
            && self.account_id == next.account_id
            && self.id == next.id
            && self.summary == next.summary
            && self.config_snapshot == next.config_snapshot
            && self.prepared == next.prepared
            && self
                .signatures
                .iter()
                .all(|signature| next.signatures.contains(signature))
            && allowed.contains(&next.state)
    }

    /// `accountScopeMatchesPrepared`.
    fn scope_matches_prepared(&self) -> bool {
        let Some((network, address)) = self.account_id.split_once(':') else {
            return false;
        };
        NetworkId::from_catalog_id(network).is_some_and(|network| {
            same_felt(address, &self.prepared.sender_address)
                && same_felt(
                    &felt::canonical(&network.chain_id()),
                    &self.prepared.chain_id,
                )
                && is_calls(&self.prepared.calls)
        })
    }

    /// `areValidSignatures`.
    fn signatures_valid(&self, primitives: &dyn StarkPrimitives) -> bool {
        let snapshot = &self.config_snapshot;
        let Some(hash) = felt::parse(&self.prepared.transaction_hash) else {
            return false;
        };
        if self.signatures.len() > snapshot.threshold as usize {
            return false;
        }
        let mut prior = Felt::ZERO;
        for signature in &self.signatures {
            let (Some(key), Some(r), Some(s)) = (
                canonical_non_zero(&signature.public_key),
                canonical_non_zero(&signature.r),
                canonical_non_zero(&signature.s),
            ) else {
                return false;
            };
            let Ok(guid) = signer_guid(&key) else {
                return false;
            };
            if !snapshot.signer_guids.contains(&felt::canonical(&guid))
                || guid <= prior
                || !primitives.verify(&hash, &key, &r, &s)
            {
                return false;
            }
            prior = guid;
        }
        true
    }

    /// `hasStateConsistentAuthority`.
    fn authority_fits_state(&self) -> bool {
        let count = self.signatures.len();
        let threshold = self.config_snapshot.threshold as usize;
        let broadcast = self.broadcast_transaction_hash.as_deref();
        match self.state {
            IntentState::Collecting => count < threshold && broadcast.is_none(),
            IntentState::Ready => count == threshold && broadcast.is_none(),
            IntentState::Stale => count <= threshold && broadcast.is_none(),
            _ => {
                count == threshold
                    && broadcast.is_some_and(|hash| {
                        canonical_non_zero(hash).is_some()
                            && same_felt(hash, &self.prepared.transaction_hash)
                    })
            }
        }
    }
}
