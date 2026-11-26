//! `DappSession`'s `wallet_signTypedData` for a Ledger account (`src/dapp_sign_device.rs`): the
//! live read first, then the SNIP-12 hash the core computed blind-signed on the device and
//! verified against the stored key, as the extension's `executeSignApproval` does through
//! `accountHashSigner`. A scripted device answers the APDUs.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

use crate::common;

use bramble_core::DappSignStep;
use common::dapp::*;
use common::imported;
use common::ledger_device::Device;
use starknet_wallet_core::{Felt, SecretFelt};

struct Ledger {
    _dir: tempfile::TempDir,
    core: std::sync::Arc<bramble_core::WalletCore>,
    dapp: std::sync::Arc<bramble_core::DappSession>,
    chain: Chain,
    card: String,
    account: String,
}

/// A connected site's Mail card for the active Ledger Ready account.
fn ledger_card(request_id: u64) -> Ledger {
    let (dir, _clock, core) = imported();
    add_ledger(&core, &ledger_key());
    let dapp = connected(&core);
    let account = core.active_account().unwrap().unwrap();
    let card = pending_id(sign_request(&dapp, SITE, &sepolia_mail(), request_id));
    Ledger {
        _dir: dir,
        chain: ready_chain("argent 0.4.0", &account.public_key),
        core,
        dapp,
        card,
        account: account.id,
    }
}

impl Ledger {
    fn approve(&self, device: &mut Device) -> DappSignStep {
        let step = self
            .dapp
            .approve_sign_typed_data(self.card.clone(), self.account.clone(), NOW);
        drive(&self.dapp, &self.card, step, &self.chain, Some(device))
    }

    fn retry(&self, device: &mut Device) -> DappSignStep {
        let step = self.dapp.retry_sign_device(self.card.clone());
        drive(&self.dapp, &self.card, step, &self.chain, Some(device))
    }

    /// The Ready signature array, checked against the account's key.
    fn assert_signed(&self, step: DappSignStep) {
        let signature = signature(&settled_reply(step));
        let account = self.core.active_account().unwrap().unwrap();
        assert_eq!(signature.len(), 5);
        assert_eq!(&signature[..2], ["1", "0"]);
        assert_eq!(felt(&signature[2]), felt(&account.public_key));
        assert!(verifies(
            &self.core,
            &sepolia_mail(),
            &signature[3],
            &signature[4]
        ));
    }
}

// "signs typed data on the device for a keyless Ledger account, verified against publicKeyX"
#[test]
fn a_ledger_account_signs_typed_data_on_its_device() {
    let ledger = ledger_card(1);
    let mut device = Device::holding(ledger_key());
    ledger.assert_signed(ledger.approve(&mut device));
    // version, key, sign path, sign hash
    assert_eq!(device.apdus, 4);
    // Spent: nothing more reaches the device.
    assert!(answers_nothing(
        ledger
            .dapp
            .answer_sign_device(ledger.card.clone(), vec![0x90, 0x00])
    ));
    assert!(answers_nothing(
        ledger.dapp.retry_sign_device(ledger.card.clone())
    ));
}

#[test]
fn a_wrong_device_is_refused_before_it_is_asked_to_sign() {
    let ledger = ledger_card(1);
    let mut other = Device::holding(SecretFelt::new(Felt::from(0x1234_u64)));
    let step = ledger.approve(&mut other);
    assert_eq!(
        format!("{step:?}"),
        r#"DeviceRetry { reason: "wrong-device" }"#
    );
    // version, key: never the hash.
    assert_eq!(other.apdus, 2);
    // The right device, on retry, signs.
    ledger.assert_signed(ledger.retry(&mut Device::holding(ledger_key())));
}

#[test]
fn a_device_refusal_parks_and_a_retry_signs() {
    let ledger = ledger_card(1);
    let mut device = Device::holding(ledger_key());
    device.reject = 1;
    assert!(matches!(
        ledger.approve(&mut device),
        DappSignStep::DeviceRetry { .. }
    ));
    // Parked: no read or reply reaches it, and the card stays claimed.
    assert!(answers_nothing(ledger.dapp.answer_sign_read(
        ledger.card.clone(),
        1,
        "{}".into()
    )));
    assert!(ledger.dapp.pending_approvals(NOW).is_empty());
    ledger.assert_signed(ledger.retry(&mut device));
}

#[test]
fn a_signature_over_another_hash_is_refused() {
    let ledger = ledger_card(1);
    let mut device = Device::holding(ledger_key());
    device.tamper = true;
    assert_eq!(code(&settled_reply(ledger.approve(&mut device))), 163);
}

#[test]
fn a_lock_mid_exchange_ends_it_before_another_apdu() {
    let ledger = ledger_card(1);
    let mut device = Device::holding(ledger_key());
    let step =
        ledger
            .dapp
            .approve_sign_typed_data(ledger.card.clone(), ledger.account.clone(), NOW);
    let DappSignStep::Device { apdu } =
        drive(&ledger.dapp, &ledger.card, step, &ledger.chain, None)
    else {
        panic!("not at the device");
    };
    let answer = device.answer(&apdu);
    ledger.core.lock();
    let step = ledger.dapp.answer_sign_device(ledger.card.clone(), answer);
    assert_eq!(code(&settled_reply(step)), 163);
    assert_eq!(device.apdus, 1);
}

#[test]
fn a_page_that_has_gone_drops_its_device_signing() {
    let ledger = ledger_card(7);
    let mut device = Device::holding(ledger_key());
    device.reject = 1;
    assert!(matches!(
        ledger.approve(&mut device),
        DappSignStep::DeviceRetry { .. }
    ));
    ledger.dapp.withdraw(vec![7]);
    assert!(answers_nothing(
        ledger.dapp.retry_sign_device(ledger.card.clone())
    ));
    // Nor can a deny or another approval reach the claimed card.
    assert!(ledger.dapp.deny(ledger.card.clone(), NOW).is_empty());
    assert!(answers_nothing(ledger.dapp.approve_sign_typed_data(
        ledger.card.clone(),
        ledger.account.clone(),
        NOW
    )));
}

// The extension rejects at settle time: the device's signature answers nothing once the card's
// account is no longer the active one granted to the site.
#[test]
fn an_account_switched_while_the_device_signs_gets_no_signature() {
    let ledger = ledger_card(1);
    let mut device = Device::holding(ledger_key());
    device.reject = 1;
    ledger.approve(&mut device);
    // Another account becomes active before the person confirms on the device.
    ledger
        .core
        .add_regular_account(bramble_core::NetworkId::Sepolia)
        .unwrap();
    assert_eq!(code(&settled_reply(ledger.retry(&mut device))), 163);
    // The device signed (4 + 4 APDUs); its signature was dropped at settle.
    assert_eq!(device.apdus, 8);
}

// The side panel's reject: the person gives up on a parked signing, and the page is refused.
#[test]
fn cancelling_a_parked_device_signing_refuses_the_page() {
    let ledger = ledger_card(1);
    let mut device = Device::holding(ledger_key());
    device.reject = 1;
    ledger.approve(&mut device);
    let step = ledger.dapp.cancel_sign_device(ledger.card.clone());
    assert_eq!(code(&settled_reply(step)), 163);
    assert!(answers_nothing(
        ledger.dapp.retry_sign_device(ledger.card.clone())
    ));
}
