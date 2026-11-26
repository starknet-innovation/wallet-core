//! A Ledger co-signer signs an Argent multisig intent on its device (P8), as the extension's
//! `signIntent` does: its `signerForContext` is `accountHashSigner`, which routes a keyless Ledger
//! account to the device (`account-hash-signer.ts`, `multisig-tx-ops.ts`). A proposal signs
//! nothing (its intent starts with no signatures), so co-signing is the only multisig signature
//! a Ledger signer makes outside a plan.
//!
//! `sign_multisig_intent_on_device` runs `sign_multisig_intent`'s checks (the intent collecting,
//! the chain holding its snapshot and nonce, the record a member that has not signed) and then
//! hands the host the device's APDUs (`ledger::HashSigning`: the app version, the key at the
//! record's path against its stored key, `signHash`, the signature verified through Krusty). A
//! refusal parks at `DeviceRetry`; a lock ends it before another APDU. The verified signature is
//! stored as `sign_multisig_intent` stores one.
//!
//! The hash is the stored intent's prepared transaction hash, which the store recomputes from
//! the prepared fields on every load and which is checked again here; the host names an intent,
//! never a hash, so this cannot be used to sign anything else.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use wallet_core::ledger::{
    DeviceKey, HashFailure, HashSigning, HashStep, Reply, TransportFailure, VerifiedSignature,
};
use wallet_core::send::multisig_intent::IntentState;
use wallet_core::send::multisig_invoke::MultisigSignature;
use wallet_core::Felt;

use crate::multisig_rpc::MultisigStateRead;
use crate::multisig_tx::MultisigIntent;
use crate::multisig_tx_ops::cosigner;
use crate::wallet::WalletCore;
use crate::{felt, CoreError};

fn refused(reason: &str) -> CoreError {
    CoreError::InvalidInput {
        reason: reason.into(),
    }
}

/// What the host does next with a `MultisigDeviceSigning`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum MultisigDeviceStep {
    /// Send `apdu` to the signer's device; pass its answer to `reply`, or what the transport
    /// reported to `transport_failure`.
    Device { apdu: Vec<u8> },
    /// The device refused (`reason` is the extension's rejection code: `device-locked`,
    /// `wrong-device`, ...). Nothing was signed. Call `retry` once the person has fixed it.
    DeviceRetry { reason: String },
    /// Done: the intent is stored with this signer's signature (`multisig_intents` reads it,
    /// ready once the threshold is reached).
    Signed { intent_id: String },
}

enum Stage {
    Ready(DeviceKey),
    Signing(HashSigning),
    Ended,
}

/// One co-signature of one intent on one Ledger signer's device. Every call after an end fails.
#[derive(uniffi::Object)]
pub struct MultisigDeviceSigning {
    core: Weak<WalletCore>,
    generation: u64,
    intent_id: String,
    hash: Felt,
    public_key: Felt,
    stage: Mutex<Stage>,
}

#[uniffi::export]
impl WalletCore {
    /// Co-signs a collecting intent on the device of the Ledger record `signer_account_id`
    /// (`signIntent` for a Ledger signer), under `sign_multisig_intent`'s checks; `state` is
    /// `multisig_intent_read` of this intent, driven to its end. Call `start` on the result.
    pub fn sign_multisig_intent_on_device(
        self: Arc<Self>,
        intent_id: String,
        signer_account_id: String,
        state: Arc<MultisigStateRead>,
    ) -> Result<Arc<MultisigDeviceSigning>, CoreError> {
        let (intent, record) = self.collecting_intent(&intent_id, &state)?;
        // The stored intent was validated on load; the hash is checked against its prepared
        // fields once more before a device is asked to sign it.
        let hash = Some(&intent.prepared)
            .filter(|prepared| prepared.is_valid())
            .and_then(|prepared| felt::parse(&prepared.transaction_hash))
            .ok_or_else(|| refused("invalid Argent multisig intent"))?;
        let (device, public_key, generation) = self.with_session(true, |session| {
            let signer = cosigner(&session.payload, &intent, &record, &signer_account_id)?;
            let device = crate::ledger::record_device(signer)
                .ok_or_else(|| refused("selected signer is not a Ledger account"))?;
            let public_key = felt::parse(&signer.public_key)
                .ok_or_else(|| refused("selected signer account is unavailable"))?;
            Ok((device, public_key, self.lock_epoch.load(Ordering::SeqCst)))
        })?;
        Ok(Arc::new(MultisigDeviceSigning {
            core: Arc::downgrade(&self),
            generation,
            intent_id: intent.id,
            hash,
            public_key,
            stage: Mutex::new(Stage::Ready(device)),
        }))
    }
}

#[uniffi::export]
impl MultisigDeviceSigning {
    /// The first step. Asked again, the signature ends.
    pub fn start(&self) -> Result<MultisigDeviceStep, CoreError> {
        let mut stage = self.stage.lock().unwrap_or_else(PoisonError::into_inner);
        let Stage::Ready(device) = *stage else {
            return Self::end(&mut stage, HashFailure::Refused(unexpected()));
        };
        let (signing, step) =
            HashSigning::start(device, self.hash, self.public_key, &|| self.current());
        *stage = Stage::Signing(signing);
        self.settle(&mut stage, step)
    }

    /// The device's answer to the last APDU: its data, then the status word.
    pub fn reply(&self, reply: Vec<u8>) -> Result<MultisigDeviceStep, CoreError> {
        self.advance(Ok(&reply))
    }

    /// What the transport reported in place of an answer.
    pub fn transport_failure(
        &self,
        failure: TransportFailure,
    ) -> Result<MultisigDeviceStep, CoreError> {
        self.advance(Err(failure))
    }

    /// After `DeviceRetry`: sign the same intent on the device again, from the start.
    pub fn retry(&self) -> Result<MultisigDeviceStep, CoreError> {
        let mut stage = self.stage.lock().unwrap_or_else(PoisonError::into_inner);
        let step = match &mut *stage {
            Stage::Signing(signing) => signing.retry(&|| self.current()),
            _ => return Self::end(&mut stage, HashFailure::Refused(unexpected())),
        };
        self.settle(&mut stage, step)
    }
}

fn unexpected() -> wallet_core::ledger::RejectionCode {
    wallet_core::ledger::RejectionCode::UnexpectedDeviceError
}

impl MultisigDeviceSigning {
    /// Whether the session this was made in is still the live one.
    fn current(&self) -> bool {
        self.core
            .upgrade()
            .is_some_and(|core| core.session_generation() == Some(self.generation))
    }

    fn advance(&self, reply: Reply<'_>) -> Result<MultisigDeviceStep, CoreError> {
        let mut stage = self.stage.lock().unwrap_or_else(PoisonError::into_inner);
        let step = match &mut *stage {
            Stage::Signing(signing) => signing.on_reply(reply, &|| self.current()),
            _ => return Self::end(&mut stage, HashFailure::Refused(unexpected())),
        };
        self.settle(&mut stage, step)
    }

    fn settle(&self, stage: &mut Stage, step: HashStep) -> Result<MultisigDeviceStep, CoreError> {
        match step {
            HashStep::Send(apdu) => Ok(MultisigDeviceStep::Device { apdu }),
            HashStep::Retry(code) => Ok(MultisigDeviceStep::DeviceRetry {
                reason: code.as_str().into(),
            }),
            HashStep::Failed(failure) => Self::end(stage, failure),
            HashStep::Signed(signature) => {
                *stage = Stage::Ended;
                self.store(signature)
                    .map(|intent| MultisigDeviceStep::Signed {
                        intent_id: intent.id,
                    })
            }
        }
    }

    fn end(stage: &mut Stage, failure: HashFailure) -> Result<MultisigDeviceStep, CoreError> {
        *stage = Stage::Ended;
        Err(match failure {
            HashFailure::Locked => CoreError::Locked,
            HashFailure::Refused(code) => refused(code.as_str()),
        })
    }

    /// Adds the signature to the intent as stored now (another signer may have signed while the
    /// person confirmed), still the same transaction and still collecting.
    fn store(&self, signature: VerifiedSignature) -> Result<MultisigIntent, CoreError> {
        let core = self
            .core
            .upgrade()
            .filter(|core| core.session_generation() == Some(self.generation))
            .ok_or(CoreError::Locked)?;
        let intent = core.stored_intent(&self.intent_id)?;
        if intent.state != IntentState::Collecting {
            return Err(refused(
                "Argent multisig intent is not collecting signatures",
            ));
        }
        if felt::parse(&intent.prepared.transaction_hash) != Some(self.hash) {
            return Err(refused("invalid Argent multisig intent"));
        }
        if intent
            .signatures
            .iter()
            .any(|signed| felt::parse(&signed.public_key) == Some(self.public_key))
        {
            return Err(refused(
                "this Argent multisig signer already signed the intent",
            ));
        }
        core.store_signature(
            intent,
            MultisigSignature {
                public_key: felt::canonical(&self.public_key),
                r: felt::canonical(&signature.r()),
                s: felt::canonical(&signature.s()),
            },
        )
    }
}
