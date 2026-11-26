//! P8: a Ledger co-signer signs an Argent multisig intent on its device, as the extension's
//! `signIntent` does through `accountHashSigner`. The fixture's last co-signer key is added as a
//! Ledger Argent account (same key, another address), and a scripted device holding that key
//! answers the APDUs. Krusty's signatures are deterministic, so the device's signature is the one
//! the extension's co-signer made and every package is the recorded one.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

mod common;
mod multisig_support;
mod multisig_tx_support;

use std::sync::Arc;

use bramble_core::ledger::LedgerScheme;
use bramble_core::{
    AccountFamily, CoreError, IntentState, LedgerAccountRequest, MultisigDeviceSigning,
    MultisigDeviceStep, MultisigIntent, MultisigTxStep, StepReply,
};
use multisig_support::fixture;
use multisig_tx_support::*;
use serde_json::Value;
use starknet_wallet_core::accounts::Ladder;
use starknet_wallet_core::kms::account_keypair;
use starknet_wallet_core::{felt, sign_stark_hash, Felt, SecretFelt};

const OK: [u8; 2] = [0x90, 0x00];
const USER_REJECTED: [u8; 2] = [0x6e, 0x04];

/// A scripted Starknet app holding one key.
struct Device {
    key: SecretFelt,
    /// Answer `signHash` with the person's rejection, this many times.
    reject: usize,
    /// Sign another hash than the one sent.
    tamper: bool,
    apdus: usize,
}

impl Device {
    fn holding(key: SecretFelt) -> Self {
        Self {
            key,
            reject: 0,
            tamper: false,
            apdus: 0,
        }
    }

    fn answer(&mut self, apdu: &[u8]) -> Vec<u8> {
        self.apdus += 1;
        let f = |hex: &str| Felt::from_hex(hex).unwrap();
        match (apdu[1], apdu[2]) {
            (0x00, _) => [&[2, 4, 0][..], &OK].concat(),
            (0x01, _) => {
                let x = f(&sign_stark_hash(&self.key, "0x1").unwrap().public_key);
                [&[0x04][..], &x.to_bytes_be(), &[0x22; 32], &OK].concat()
            }
            (0x02, 0) => OK.to_vec(),
            (0x02, 1) if self.reject > 0 => {
                self.reject -= 1;
                USER_REJECTED.to_vec()
            }
            (0x02, 1) => {
                let mut hash = Felt::from_bytes_be_slice(&apdu[5..37]);
                if self.tamper {
                    hash += Felt::ONE;
                }
                let signature = sign_stark_hash(&self.key, &felt::canonical(&hash)).unwrap();
                let (r, s) = (f(&signature.r), f(&signature.s));
                [
                    &[0x41][..],
                    &r.to_bytes_be(),
                    &s.to_bytes_be(),
                    &[0x00],
                    &OK,
                ]
                .concat()
            }
            other => panic!("unexpected APDU {other:?}"),
        }
    }
}

/// Answers APDUs until the signing leaves the device.
fn drive(
    signing: &MultisigDeviceSigning,
    device: &mut Device,
    mut step: MultisigDeviceStep,
) -> Result<MultisigDeviceStep, CoreError> {
    while let MultisigDeviceStep::Device { apdu } = &step {
        step = signing.reply(device.answer(apdu))?;
    }
    Ok(step)
}

struct Setup {
    wallet: Wallet,
    /// The intent, signed by every co-signer but the last.
    intent: MultisigIntent,
    /// The last co-signer's key as a Ledger account.
    ledger_id: String,
    /// The last co-signer's software record.
    software_id: String,
    key: SecretFelt,
}

fn setup(net: &Value) -> Setup {
    let wallet = wallet(net);
    let cosigners: Vec<u64> = net["proposal"]["cosignerAccountIndexes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|index| index.as_u64().unwrap())
        .collect();
    let (last, others) = cosigners.split_last().unwrap();
    let mut intent = wallet.propose();
    for index in others {
        intent = wallet.sign(&intent, *index);
    }
    assert_eq!(intent.state, IntentState::Collecting);
    let public_key = net["signers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|signer| signer["accountIndex"] == *last)
        .unwrap()["publicKey"]
        .as_str()
        .unwrap()
        .to_string();
    let phrase = fixture()["mnemonic"].as_str().unwrap().to_string();
    // The key behind the co-signer's record, whichever ladder its family derives on.
    let key = [Ladder::Account, Ladder::Address, Ladder::ArgentX]
        .into_iter()
        .flat_map(|ladder| (0..8).map(move |index| (ladder, index)))
        .map(|(ladder, index)| account_keypair(&phrase, None, ladder, index).unwrap().0)
        .find(|pair| felt::parse(&pair.public_key_x) == felt::parse(&public_key))
        .expect("the co-signer's key")
        .private_key;
    let ledger = wallet
        .core
        .add_ledger_account(LedgerAccountRequest {
            network: wallet.network,
            family: AccountFamily::Argent,
            scheme: LedgerScheme::Bramble,
            account_index: 0,
            public_key_x: public_key,
        })
        .unwrap();
    assert!(ledger.ledger);
    Setup {
        software_id: wallet.signer_id(*last),
        wallet,
        intent,
        ledger_id: ledger.id,
        key,
    }
}

impl Setup {
    fn on_device(&self) -> Arc<MultisigDeviceSigning> {
        self.wallet
            .core
            .clone()
            .sign_multisig_intent_on_device(
                self.intent.id.clone(),
                self.ledger_id.clone(),
                self.wallet.current_state(&self.intent),
            )
            .unwrap()
    }

    fn device(&self) -> Device {
        Device::holding(self.key.clone())
    }

    fn stored(&self) -> MultisigIntent {
        let stored = self
            .wallet
            .core
            .multisig_intents(self.wallet.multisig_id())
            .unwrap();
        stored.into_iter().find(|i| i.id == self.intent.id).unwrap()
    }
}

fn networks() -> Vec<Value> {
    fixture()["networks"].as_array().unwrap().clone()
}

#[test]
fn a_ledger_cosigner_reaches_the_threshold_and_the_intent_submits() {
    for net in networks() {
        let setup = setup(&net);
        let signing = setup.on_device();
        let mut device = setup.device();
        let step = drive(&signing, &mut device, signing.start().unwrap()).unwrap();
        assert_eq!(
            step,
            MultisigDeviceStep::Signed {
                intent_id: setup.intent.id.clone()
            }
        );
        // version, key, sign path, sign hash
        assert_eq!(device.apdus, 4);
        let intent = setup.stored();
        assert_eq!(intent.state, IntentState::Ready);
        assert_eq!(
            intent.package,
            net["proposal"]["readyPackage"].as_str().unwrap()
        );
        // Spent: nothing more reaches the device.
        assert!(signing.start().is_err());
        assert!(signing.reply(OK.to_vec()).is_err());

        let wallet = &setup.wallet;
        let submission = wallet
            .core
            .clone()
            .submit_multisig_intent(intent.id.clone(), wallet.current_state(&intent))
            .unwrap();
        let step = submission.advance(StepReply::Start).unwrap();
        let step = multisig_tx_support::drive(step, &phase(&net["proposal"]["submit"]), |reply| {
            submission.advance(reply).unwrap()
        });
        let hash = net["proposal"]["transactionHash"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(step, MultisigTxStep::Submitted { hash });
    }
}

#[test]
fn each_record_signs_only_its_own_way() {
    let net = &networks()[0];
    let setup = setup(net);
    let state = || setup.wallet.current_state(&setup.intent);
    // A Ledger record has no key to sign with.
    assert!(setup
        .wallet
        .core
        .sign_multisig_intent(setup.intent.id.clone(), setup.ledger_id.clone(), state())
        .is_err());
    // A software record keeps its key path.
    let refused = setup
        .wallet
        .core
        .clone()
        .sign_multisig_intent_on_device(setup.intent.id.clone(), setup.software_id.clone(), state())
        .err()
        .unwrap();
    assert!(
        matches!(refused, CoreError::InvalidInput { reason } if reason == "selected signer is not a Ledger account")
    );
    assert_eq!(setup.stored(), setup.intent);
}

#[test]
fn a_wrong_device_parks_and_the_right_one_signs_on_retry() {
    let net = &networks()[0];
    let setup = setup(net);
    let signing = setup.on_device();
    let mut wrong = Device::holding(SecretFelt::new(Felt::from(0xabcu64)));
    let step = drive(&signing, &mut wrong, signing.start().unwrap()).unwrap();
    assert_eq!(
        step,
        MultisigDeviceStep::DeviceRetry {
            reason: "wrong-device".into()
        }
    );
    // Refused before the person is asked: no signHash APDU went out.
    assert_eq!(wrong.apdus, 2);
    assert_eq!(setup.stored(), setup.intent);
    let mut right = setup.device();
    let step = drive(&signing, &mut right, signing.retry().unwrap()).unwrap();
    assert!(
        matches!(step, MultisigDeviceStep::Signed { .. }),
        "{step:?}"
    );
    assert_eq!(setup.stored().state, IntentState::Ready);
}

#[test]
fn a_rejection_on_the_device_parks_until_retried() {
    let net = &networks()[0];
    let setup = setup(net);
    let signing = setup.on_device();
    let mut device = setup.device();
    device.reject = 1;
    let step = drive(&signing, &mut device, signing.start().unwrap()).unwrap();
    assert_eq!(
        step,
        MultisigDeviceStep::DeviceRetry {
            reason: "rejected-on-device".into()
        }
    );
    assert_eq!(setup.stored(), setup.intent);
    // Parked: a stray answer ends it rather than being read.
    let parked = setup.on_device();
    let mut again = setup.device();
    again.reject = 1;
    drive(&parked, &mut again, parked.start().unwrap()).unwrap();
    assert!(parked.reply(OK.to_vec()).is_err());
    assert!(parked.retry().is_err());

    let step = drive(&signing, &mut device, signing.retry().unwrap()).unwrap();
    assert!(
        matches!(step, MultisigDeviceStep::Signed { .. }),
        "{step:?}"
    );
}

#[test]
fn a_lock_mid_signing_stops_the_device() {
    let net = &networks()[0];
    let setup = setup(net);
    let signing = setup.on_device();
    let mut device = setup.device();
    let MultisigDeviceStep::Device { apdu } = signing.start().unwrap() else {
        panic!("expected an APDU");
    };
    let answer = device.answer(&apdu);
    setup.wallet.relock();
    assert!(matches!(signing.reply(answer), Err(CoreError::Locked)));
    // Ended for good, a new session included.
    assert!(signing.reply(OK.to_vec()).is_err());
    assert!(signing.retry().is_err());
    assert_eq!(device.apdus, 1);
    assert_eq!(setup.stored(), setup.intent);
}

#[test]
fn a_signature_that_does_not_verify_is_refused() {
    let net = &networks()[0];
    let setup = setup(net);
    let signing = setup.on_device();
    let mut device = setup.device();
    device.tamper = true;
    let refused = drive(&signing, &mut device, signing.start().unwrap())
        .err()
        .unwrap();
    assert!(
        matches!(refused, CoreError::InvalidInput { reason } if reason == "signature-verify-failed")
    );
    assert!(signing.retry().is_err());
    assert_eq!(setup.stored(), setup.intent);
}
