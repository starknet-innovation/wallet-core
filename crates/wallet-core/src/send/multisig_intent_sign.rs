//! Adding one signer's signature to a collecting intent: the extension's `persistSignature`
//! (`apps/extension/src/background/multisig/multisig-tx-ops.ts`), shared by a local sign, a
//! Ledger sign and a co-signer's pasted signature (`importSignature`), so every signature joins
//! an intent under one rule before the store validates the result again.

use std::fmt;

use super::multisig_intent::{IntentState, SigningIntent};
use super::multisig_invoke::MultisigSignature;
use crate::accounts::multisig::signer_guid;
use crate::felt;
use crate::kms::stark_signature_is_valid;

/// Why a signature may not join an intent. `Display` is the extension's sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureError {
    /// The intent is not collecting signatures (the caller's check in the extension).
    NotCollecting,
    /// The key is not a snapshot member, a felt is not canonical and non-zero, or the signature
    /// does not verify over the intent's recomputed hash.
    Invalid,
    /// A signature by this signer (GUID) is already held.
    AlreadySigned,
    /// The intent already holds its threshold of signatures.
    ThresholdReached,
}

impl SignatureError {
    pub fn message(self) -> &'static str {
        match self {
            Self::NotCollecting => "Argent multisig intent is not collecting signatures",
            Self::Invalid => "signature is not valid for this exact Argent multisig intent",
            Self::AlreadySigned => "this Argent multisig signer already signed the intent",
            Self::ThresholdReached => "Argent multisig intent already has its threshold signatures",
        }
    }
}

impl fmt::Display for SignatureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for SignatureError {}

fn guid_of(public_key: &str) -> Option<starknet_types_core::felt::Felt> {
    signer_guid(&felt::parse_canonical_non_zero(public_key)?).ok()
}

impl SigningIntent {
    /// `persistSignature`: this collecting intent with `signature` added in GUID order, ready at
    /// the threshold. Checks in the extension's order: the signer is a snapshot member and the
    /// signature verifies over the hash the prepared transaction recomputes to (never the stored
    /// hash alone), the signer has not signed, and the threshold is not reached.
    pub fn with_signature(
        &self,
        signature: MultisigSignature,
    ) -> Result<SigningIntent, SignatureError> {
        if self.state != IntentState::Collecting {
            return Err(SignatureError::NotCollecting);
        }
        let (Some(key), Some(r), Some(s), Some(hash)) = (
            felt::parse_canonical_non_zero(&signature.public_key),
            felt::parse_canonical_non_zero(&signature.r),
            felt::parse_canonical_non_zero(&signature.s),
            felt::parse(&self.prepared.transaction_hash),
        ) else {
            return Err(SignatureError::Invalid);
        };
        let guid = signer_guid(&key).map_err(|_| SignatureError::Invalid)?;
        if !self.prepared.is_valid()
            || !self
                .config_snapshot
                .signer_guids
                .contains(&felt::canonical(&guid))
            || !stark_signature_is_valid(&key, &hash, &r, &s)
        {
            return Err(SignatureError::Invalid);
        }
        if self
            .signatures
            .iter()
            .any(|held| guid_of(&held.public_key) == Some(guid))
        {
            return Err(SignatureError::AlreadySigned);
        }
        let threshold = self.config_snapshot.threshold as usize;
        if self.signatures.len() >= threshold {
            return Err(SignatureError::ThresholdReached);
        }
        let mut signatures = self.signatures.clone();
        signatures.push(signature);
        signatures.sort_by_cached_key(|held| guid_of(&held.public_key));
        let state = if signatures.len() == threshold {
            IntentState::Ready
        } else {
            IntentState::Collecting
        };
        Ok(SigningIntent {
            signatures,
            state,
            ..self.clone()
        })
    }
}
