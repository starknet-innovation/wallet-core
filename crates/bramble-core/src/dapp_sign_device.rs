//! A Ledger account signs a site's typed data on its device, as the extension's
//! `executeSignApproval` does through `accountHashSigner` (`ledgerStarkHashSigner` and the
//! panel's sign queue): the device blind-signs the SNIP-12 message hash, and the signature is
//! verified against the account's stored key before any page sees it.
//!
//! The hash is the one `finish_sign` computed under the session hold that ran every check
//! (`reviewed_hash` at `Stage::Sign`, an outside execution's review included); the host names an
//! approval, never a hash. The exchange is `ledger::HashSigning`: the app version, the key at the
//! record's path against its stored key, `signHash`, the signature verified through Krusty. A
//! refusal parks at `DeviceRetry`; a lock ends it before another APDU. Before the signature
//! answers the pages, the cheap checks run again (the session the hash was computed in, the site
//! not blocked, the card's account still the granted active one), as the extension's queue
//! rejects at settle time.

use std::sync::atomic::Ordering;

use starknet_wallet_core::ledger::{
    DeviceKey, HashSigning, HashStep, Reply, TransportFailure, VerifiedSignature,
};
use starknet_wallet_core::Felt;

use crate::dapp_session::DappSession;
use crate::dapp_sign::{refused as refused_rpc, signature_json, still_granted, Approved};
use crate::dapp_sign_read::{DappSignStep, PendingSign, SignStage};
use crate::felt;
use crate::wallet::guard;

/// The device a Ledger account signs on, the hash it signs and the key it must verify under.
pub(crate) struct DeviceStart {
    pub(crate) device: DeviceKey,
    pub(crate) hash: Felt,
    pub(crate) public_key: Felt,
}

impl DappSession {
    fn current(&self, generation: u64) -> bool {
        self.wallet.session_generation() == Some(generation)
    }

    /// Hands `start`'s hash to the device, under the session `generation` that computed it.
    pub(crate) fn start_device(
        &self,
        approval_id: String,
        approved: Approved,
        start: DeviceStart,
        generation: u64,
    ) -> DappSignStep {
        let (signing, step) =
            HashSigning::start(start.device, start.hash, start.public_key, &|| {
                self.current(generation)
            });
        let stage = SignStage::Device {
            signing: Box::new(signing),
            generation,
            public_key: start.public_key,
        };
        self.device_step(approval_id, PendingSign { approved, stage }, step)
    }

    fn device_step(&self, approval_id: String, sign: PendingSign, step: HashStep) -> DappSignStep {
        let reply = match step {
            HashStep::Send(apdu) => DappSignStep::Device { apdu },
            HashStep::Retry(code) => DappSignStep::DeviceRetry {
                reason: code.as_str().into(),
            },
            HashStep::Failed(_) => return sign.approved.answer(Err(refused_rpc())),
            HashStep::Signed(signature) => return self.settle_device(sign, signature),
        };
        guard(&self.state).signing.insert(approval_id, sign);
        reply
    }

    /// Answers the pages with the device's verified signature, if the session that computed the
    /// hash is still the live one and the card's account is still the granted active one.
    fn settle_device(&self, sign: PendingSign, signature: VerifiedSignature) -> DappSignStep {
        let SignStage::Device {
            generation,
            public_key,
            ..
        } = sign.stage
        else {
            return sign.approved.answer(Err(refused_rpc()));
        };
        let approved = &sign.approved;
        let json = self.with_site(
            &approved.origin,
            approved.card.network,
            false,
            |site, session| {
                let payload = session.map(|session| &session.payload);
                if payload.is_none() || self.wallet.lock_epoch.load(Ordering::SeqCst) != generation
                {
                    return Ok(Err(refused_rpc()));
                }
                Ok(
                    still_granted(site, payload, &approved.card, &approved.reviewed_account_id)
                        .and_then(|record| {
                            if felt::parse(&record.public_key) != Some(public_key) {
                                return Err(refused_rpc());
                            }
                            let (r, s) = (signature.r(), signature.s());
                            signature_json(record, &felt::canonical(&r), &felt::canonical(&s))
                        }),
                )
            },
        );
        sign.approved
            .answer(json.unwrap_or_else(|_| Err(refused_rpc())))
    }

    /// One step of `approval_id`'s device signing, if one waits on the device.
    fn advance_device(
        &self,
        approval_id: String,
        step: impl FnOnce(&mut HashSigning, &dyn Fn() -> bool) -> HashStep,
    ) -> DappSignStep {
        let Some(mut sign) = self.take_sign(&approval_id, true) else {
            return DappSignStep::Settled {
                settled: Vec::new(),
            };
        };
        let SignStage::Device {
            signing,
            generation,
            ..
        } = &mut sign.stage
        else {
            return sign.approved.answer(Err(refused_rpc()));
        };
        let generation = *generation;
        let next = step(signing, &|| self.current(generation));
        self.device_step(approval_id, sign, next)
    }
}

#[uniffi::export]
impl DappSession {
    /// The device's answer to `approval_id`'s last APDU: its data, then the status word. The next
    /// APDU, a parked refusal, or the pages' answers. An approval not waiting on the device
    /// answers nothing.
    pub fn answer_sign_device(&self, approval_id: String, reply: Vec<u8>) -> DappSignStep {
        self.advance_device(approval_id, |signing, current| {
            signing.on_reply(Ok(&reply), current)
        })
    }

    /// What the transport reported in place of the answer to `approval_id`'s last APDU.
    pub fn sign_device_transport_failure(
        &self,
        approval_id: String,
        failure: TransportFailure,
    ) -> DappSignStep {
        self.advance_device(approval_id, |signing, current| {
            signing.on_reply(Reply::Err(failure), current)
        })
    }

    /// After `DeviceRetry`: sign the same hash on the device again, from the start.
    pub fn retry_sign_device(&self, approval_id: String) -> DappSignStep {
        self.advance_device(approval_id, |signing, current| signing.retry(current))
    }

    /// The person gave up on the device (or the host's sign timeout passed): nothing is signed
    /// and the pages are refused (163), as the side panel's reject settles the held request.
    pub fn cancel_sign_device(&self, approval_id: String) -> DappSignStep {
        match self.take_sign(&approval_id, true) {
            Some(sign) => sign.approved.answer(Err(refused_rpc())),
            None => DappSignStep::Settled {
                settled: Vec::new(),
            },
        }
    }
}
