//! `DappSession`'s `wallet_signTypedData` (`src/dapp_sign.rs`) over a real vault: a granted site's
//! typed data queues as its own card, signs only on approval, and every gate refuses as the
//! extension's does. Names after `tests/background/approvals/background-sign-approval.test.ts`
//! and `tests/background/transactions/chain-ops-invalid-inputs.test.ts` where they fit.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

use crate::common;

use bramble_core::ledger::LedgerScheme;
use bramble_core::{AccountFamily, DappCard, LedgerAccountRequest, NetworkId};
use common::dapp::*;
use common::{account_id, imported, pending, selection, PASSWORD};

// "valid request never auto-signs — it queues and holds until approval"
#[test]
fn valid_request_never_auto_signs_it_queues_and_holds_until_approval() {
    let (_dir, _clock, core) = imported();
    let dapp = connected(&core);
    let account = core.active_account().unwrap().unwrap();
    let card = pending_id(sign_request(&dapp, SITE, &sepolia_mail(), 7));

    let listed = dapp.pending_approvals(NOW);
    assert_eq!(listed.len(), 1);
    let DappCard::SignTypedData {
        account_id,
        address,
        network,
        summary,
        typed_data_json,
        call_summaries,
        flags,
    } = listed[0].card.clone()
    else {
        panic!("not a sign card: {:?}", listed[0].card);
    };
    assert_eq!(
        (account_id.as_str(), address.as_str(), network),
        (
            account.id.as_str(),
            account.address.as_str(),
            NetworkId::Sepolia
        )
    );
    assert_eq!(summary, "Mail · Bramble Test");
    assert_eq!(typed_data_json, sepolia_mail());
    // A plain SNIP-12 document carries no calls to review.
    assert!(call_summaries.is_empty() && flags.is_empty());
    assert!(!listed[0].blocked);

    // A connect decision never lands on a sign card, nor the reverse.
    assert!(dapp
        .approve_connect(card.clone(), account.id.clone(), NOW)
        .is_empty());
    assert_eq!(dapp.pending_approvals(NOW).len(), 1);

    let settled = dapp.approve_sign_typed_data(card.clone(), account.id.clone(), NOW);
    assert_eq!(settled.len(), 1);
    assert_eq!(settled[0].request_id, 7);
    let signature = signature(&settled[0].reply);
    assert_eq!(signature.len(), 2, "OpenZeppelin signs `[r, s]`");
    assert!(verifies(
        &core,
        &sepolia_mail(),
        &signature[0],
        &signature[1]
    ));
    // Canonical spellings, as the page's `signaturePayload` requires.
    for part in &signature {
        assert_eq!(part, &format!("{:#x}", felt(part)));
    }
    // A card is decided once.
    assert!(dapp
        .approve_sign_typed_data(card, account.id, NOW)
        .is_empty());
    assert!(dapp.pending_approvals(NOW).is_empty());
}

#[test]
fn every_request_is_its_own_card() {
    let (_dir, _clock, core) = imported();
    let dapp = connected(&core);
    let first = pending_id(sign_request(&dapp, SITE, &sepolia_mail(), 1));
    let second = pending_id(sign_request(&dapp, SITE, &sepolia_mail(), 2));
    assert_ne!(first, second);
    assert_eq!(dapp.pending_approvals(NOW).len(), 2);
}

// "returns the standard Argent account signature for approved typed data"
#[test]
fn returns_the_standard_argent_account_signature_for_approved_typed_data() {
    let (_dir, _clock, core) = pending();
    core.restore_recovered_accounts(
        NetworkId::Sepolia,
        vec![selection(
            AccountFamily::Argent,
            0,
            Some(&class("argent 0.4.0")),
        )],
    )
    .unwrap();
    let dapp = connected(&core);
    let account = core.active_account().unwrap().unwrap();
    let card = pending_id(sign_request(&dapp, SITE, &sepolia_mail(), 1));
    let signature = signature(&approved(&dapp, card, account.id));
    assert_eq!(signature.len(), 5);
    assert_eq!(&signature[..2], ["1", "0"]);
    assert_eq!(felt(&signature[2]), felt(&account.public_key));
    assert_eq!(signature[2], format!("{:#x}", felt(&signature[2])));
    assert!(verifies(
        &core,
        &sepolia_mail(),
        &signature[3],
        &signature[4]
    ));
}

fn class(label: &str) -> String {
    bramble_core::account_classes()
        .into_iter()
        .find(|class| class.label == label)
        .unwrap()
        .class_hash
}

/// A connected site's request from `family` at `class_label` is refused before any card.
fn refused_for(family: AccountFamily, class_label: &str) {
    let (_dir, _clock, core) = pending();
    core.restore_recovered_accounts(
        NetworkId::Sepolia,
        vec![selection(family, 0, Some(&class(class_label)))],
    )
    .unwrap();
    let dapp = connected(&core);
    assert_eq!(code(&sign_request(&dapp, SITE, &sepolia_mail(), 1)), 163);
    assert!(dapp.pending_approvals(NOW).is_empty());
}

// "refuses typed data for an Argent account that still runs 0.3.x": here by the stored class,
// since the live class read is not wired.
#[test]
fn refuses_typed_data_for_an_argent_account_that_still_runs_0_3_x() {
    refused_for(AccountFamily::Argent, "argent 0.3.1");
    refused_for(AccountFamily::Argent, "argent 0.3.0");
}

// "refuses typed data for a 2FA Braavos account before using the key": every Braavos account,
// until the live class and signer read is wired.
#[test]
fn refuses_typed_data_for_any_braavos_account_until_the_live_check_lands() {
    refused_for(AccountFamily::Braavos, "braavos 1.0.0");
}

// "a failed Ledger device sign settles the held dapp request with { error }": here a Ledger
// account is refused before any card, since the device path is not wired for typed data.
#[test]
fn a_ledger_account_cannot_queue_a_signature() {
    let (_dir, _clock, core) = imported();
    core.add_ledger_account(LedgerAccountRequest {
        network: NetworkId::Sepolia,
        family: AccountFamily::Argent,
        scheme: LedgerScheme::Ready,
        account_index: 3,
        public_key_x: "0x426212993d56613e1886a4cbc5b58810570023581c2aab0b423277776b79d2e".into(),
    })
    .unwrap();
    let dapp = connected(&core);
    assert_eq!(code(&sign_request(&dapp, SITE, &sepolia_mail(), 1)), 163);
    assert!(dapp.pending_approvals(NOW).is_empty());
}

// "deny rejects the held request with an error"
#[test]
fn deny_rejects_the_held_request_with_an_error() {
    let (_dir, _clock, core) = imported();
    let dapp = connected(&core);
    let card = pending_id(sign_request(&dapp, SITE, &sepolia_mail(), 4));
    let settled = dapp.deny(card.clone(), NOW);
    assert_eq!(settled.len(), 1);
    assert_eq!(settled[0].request_id, 4);
    assert_eq!(code(&settled[0].reply), 113);
    assert!(dapp
        .approve_sign_typed_data(card, account_id(&core), NOW)
        .is_empty());
}

#[test]
fn an_expired_card_refuses_its_request() {
    let (_dir, clock, core) = imported();
    let dapp = connected(&core);
    let card = pending_id(sign_request(&dapp, SITE, &sepolia_mail(), 9));
    clock.advance(5 * 60 * 1_000);
    let settled = dapp.settle_expired();
    assert_eq!(settled.len(), 1);
    assert_eq!(settled[0].request_id, 9);
    assert_eq!(code(&settled[0].reply), 113);
    assert!(dapp
        .approve_sign_typed_data(card, account_id(&core), NOW)
        .is_empty());
}

// "an unconnected origin cannot queue a signature"
#[test]
fn an_unconnected_origin_cannot_queue_a_signature() {
    let (_dir, _clock, core) = imported();
    let dapp = connected(&core);
    let reply = sign_request(&dapp, "https://other.example", &sepolia_mail(), 1);
    assert_eq!(code(&reply), 163);
    assert!(dapp.pending_approvals(NOW).is_empty());
}

// "rejects a signTypedData while the wallet is locked"
#[test]
fn rejects_a_sign_typed_data_while_the_wallet_is_locked() {
    let (_dir, _clock, core) = imported();
    let dapp = connected(&core);
    core.lock();
    assert_eq!(code(&sign_request(&dapp, SITE, &sepolia_mail(), 1)), 163);
    assert!(dapp.pending_approvals(NOW).is_empty());
}

// "fails when the wallet is locked" (`executeSignApproval`): the card survives the lock, as the
// extension's does, and approving it while locked refuses and spends it.
#[test]
fn approving_while_locked_refuses_and_spends_the_card() {
    let (_dir, _clock, core) = imported();
    let dapp = connected(&core);
    let id = account_id(&core);
    let card = pending_id(sign_request(&dapp, SITE, &sepolia_mail(), 1));
    core.lock();
    assert_eq!(dapp.pending_approvals(NOW).len(), 1);
    assert_eq!(code(&approved(&dapp, card.clone(), id.clone())), 163);
    assert!(dapp.pending_approvals(NOW).is_empty());
    core.unlock(PASSWORD.into()).unwrap();
    assert!(dapp.approve_sign_typed_data(card, id, NOW).is_empty());
}

// A card that survives a lock is still approvable once unlocked: it binds an account, not a
// session.
#[test]
fn a_card_queued_before_a_lock_signs_after_unlock() {
    let (_dir, _clock, core) = imported();
    let dapp = connected(&core);
    let card = pending_id(sign_request(&dapp, SITE, &sepolia_mail(), 1));
    core.lock();
    core.unlock(PASSWORD.into()).unwrap();
    let signature = signature(&approved(&dapp, card, account_id(&core)));
    assert!(verifies(
        &core,
        &sepolia_mail(),
        &signature[0],
        &signature[1]
    ));
}

// "fails when the origin holds no grant on the active network": the account switched before
// approval; and a reviewed account that is not the card's.
#[test]
fn an_account_switched_before_approval_signs_nothing() {
    let (_dir, _clock, core) = imported();
    let dapp = connected(&core);
    let first = account_id(&core);
    let card = pending_id(sign_request(&dapp, SITE, &sepolia_mail(), 1));
    let second = core.add_regular_account(NetworkId::Sepolia).unwrap().id;
    assert_ne!(first, second);
    assert_eq!(code(&approved(&dapp, card, first.clone())), 163);

    // Switched back, the echo must still name the card's account.
    core.select_account(first.clone()).unwrap();
    let card = pending_id(sign_request(&dapp, SITE, &sepolia_mail(), 2));
    assert_eq!(code(&approved(&dapp, card, second)), 163);
    assert!(dapp.pending_approvals(NOW).is_empty());
}

// "rejects typed data for a different network"
#[test]
fn rejects_typed_data_for_a_different_network() {
    let (_dir, _clock, core) = imported();
    let dapp = connected(&core);
    let mainnet = typed_data("SN_MAIN", "Mail");
    assert_eq!(code(&sign_request(&dapp, SITE, &mainnet, 1)), 114);
    assert!(dapp.pending_approvals(NOW).is_empty());
}

// "rejects a malformed signTypedData message", and typed data no SNIP-12 gate accepts.
#[test]
fn rejects_malformed_typed_data() {
    let (_dir, _clock, core) = imported();
    let dapp = connected(&core);
    for document in [
        r#"{"foo":1}"#,
        r#""text""#,
        &sepolia_mail().replace(r#""type":"felt""#, r#""type":"Mail""#),
        &sepolia_mail().replace("Mail", "Ma(il"),
    ] {
        assert_eq!(
            code(&sign_request(&dapp, SITE, document, 1)),
            114,
            "{document}"
        );
    }
    let reply = dapp
        .request(
            SITE.into(),
            "wallet_signTypedData".into(),
            Some("null".into()),
            NetworkId::Sepolia,
            2,
        )
        .unwrap();
    assert_eq!(code(&reply), 114);
    assert!(dapp.pending_approvals(NOW).is_empty());
}

#[test]
fn a_blocked_origin_cannot_queue_a_signature() {
    let (_dir, _clock, core) = imported();
    let dapp = connected(&core);
    let reply = sign_request(&dapp, "https://starknet-airdrop.com", &sepolia_mail(), 1);
    assert_eq!(code(&reply), 163);
    assert!(dapp.pending_approvals(NOW).is_empty());
}

// The queue's per-site cap applies before the vault is read or anything is hashed.
#[test]
fn a_site_holds_at_most_five_sign_cards() {
    let (_dir, _clock, core) = imported();
    let dapp = connected(&core);
    for request_id in 1..=5 {
        pending_id(sign_request(&dapp, SITE, &sepolia_mail(), request_id));
    }
    assert_eq!(code(&sign_request(&dapp, SITE, &sepolia_mail(), 6)), 163);
}
