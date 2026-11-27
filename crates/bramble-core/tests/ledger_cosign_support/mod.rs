//! Shared by the Ledger co-signer suites: a scripted device holding the fixture's last co-signer
//! key, and a wallet whose intent waits for that signer's signature, the key added as a Ledger
//! Argent account (same key, another address).
#![allow(dead_code)] // Each suite reads a different subset.

use std::sync::Arc;

use bramble_core::ledger::LedgerScheme;
use bramble_core::{
    AccountFamily, CoreError, IntentState, LedgerAccountRequest, MultisigDeviceSigning,
    MultisigDeviceStep, MultisigIntent,
};
use serde_json::Value;
use starknet_wallet_core::accounts::Ladder;
use starknet_wallet_core::kms::account_keypair;
use starknet_wallet_core::{felt, SecretFelt};

use super::multisig_support::fixture;
use super::multisig_tx_support::*;

pub use super::common::ledger_device::Device;

/// Answers APDUs until the signing leaves the device.
pub fn drive_device(
    signing: &MultisigDeviceSigning,
    device: &mut Device,
    mut step: MultisigDeviceStep,
) -> Result<MultisigDeviceStep, CoreError> {
    while let MultisigDeviceStep::Device { apdu } = &step {
        step = signing.reply(device.answer(apdu))?;
    }
    Ok(step)
}

pub struct Setup {
    pub wallet: Wallet,
    /// The intent, signed by every co-signer but the last.
    pub intent: MultisigIntent,
    /// The last co-signer's key as a Ledger account.
    pub ledger_id: String,
    /// The last co-signer's software record.
    pub software_id: String,
    pub key: SecretFelt,
}

pub fn setup(net: &Value) -> Setup {
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
        .add_ledger_account(
            LedgerAccountRequest {
                network: wallet.network,
                family: AccountFamily::Argent,
                scheme: LedgerScheme::Bramble,
                account_index: 0,
                public_key_x: public_key,
            },
            Vec::new(),
        )
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
    pub fn on_device(&self) -> Arc<MultisigDeviceSigning> {
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

    pub fn device(&self) -> Device {
        Device::holding(self.key.clone())
    }

    pub fn stored(&self) -> MultisigIntent {
        let stored = self
            .wallet
            .core
            .multisig_intents(self.wallet.multisig_id())
            .unwrap();
        stored.into_iter().find(|i| i.id == self.intent.id).unwrap()
    }
}

pub fn networks() -> Vec<Value> {
    fixture()["networks"].as_array().unwrap().clone()
}
