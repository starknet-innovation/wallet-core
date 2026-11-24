//! Every vault record and chain read P1's export takes is bound to the multisig it acts on, by
//! address and by network: a member's key on another network, a read of the same address on
//! another network, or a read of another account is refused. Also what the export keeps out of
//! the store: a stale intent short of its threshold is never submitted, a removed multisig's
//! intents go with it, and an import needs a parseable package and a finished read.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

mod common;
mod multisig_support;
mod multisig_tx_support;

use bramble_core::{CoreError, MultisigStateRead, NetworkId};
use multisig_support::fixture;
use multisig_tx_support::*;

fn sepolia() -> Wallet {
    wallet(&fixture()["networks"][0])
}

fn reason(error: CoreError) -> String {
    match error {
        CoreError::InvalidInput { reason } => reason,
        other => panic!("{other:?}"),
    }
}

/// The Sepolia record `sepolia_id` projected onto Mainnet: the same key, another network.
fn on_mainnet(wallet: &Wallet, sepolia_id: &str) -> String {
    wallet.core.select_account(sepolia_id.into()).unwrap();
    let projected = wallet
        .core
        .ensure_network_account(NetworkId::Mainnet)
        .unwrap()
        .unwrap();
    assert_eq!(projected.network, NetworkId::Mainnet);
    projected.id
}

/// A state read of `address` on `network`, answered as the fixture multisig's chain.
fn state_of(
    wallet: &Wallet,
    network: NetworkId,
    address: &str,
    nonce: &str,
) -> std::sync::Arc<MultisigStateRead> {
    let read = MultisigStateRead::new(network, address.into()).unwrap();
    let mut replies = wallet.authority_replies(wallet.threshold(), &wallet.guids());
    replies[0] = serde_json::json!(match network {
        NetworkId::Mainnet => "0x534e5f4d41494e",
        NetworkId::Sepolia => "0x534e5f5345504f4c4941",
    });
    replies.push(serde_json::json!(nonce));
    for result in replies {
        let request = read.next_request().unwrap();
        read.accept_response(request.id, body(request.id, result))
            .unwrap();
    }
    read
}

#[test]
fn a_member_key_on_another_network_neither_deploys_nor_signs() {
    let wallet = sepolia();
    let deployer = wallet.signer_id(
        wallet.net["deploy"]["deployerAccountIndex"]
            .as_u64()
            .unwrap(),
    );
    let elsewhere = on_mainnet(&wallet, &deployer);
    let refused = wallet
        .core
        .clone()
        .prepare_multisig_deployment(wallet.multisig_id(), elsewhere)
        .err()
        .unwrap();
    assert_eq!(reason(refused), "unknown account");

    let intent = wallet.propose();
    let cosigner = wallet.signer_id(
        wallet.net["proposal"]["cosignerAccountIndexes"][0]
            .as_u64()
            .unwrap(),
    );
    let elsewhere = on_mainnet(&wallet, &cosigner);
    let refused = wallet
        .core
        .sign_multisig_intent(intent.id.clone(), elsewhere, wallet.current_state(&intent))
        .err()
        .unwrap();
    assert_eq!(
        reason(refused),
        "selected signer account is unavailable on this network"
    );
}

#[test]
fn a_read_of_another_network_or_account_is_refused() {
    let wallet = sepolia();
    let intent = wallet.propose();
    let signer = wallet.signer_id(0);
    let another = "0x1234";
    for (network, address) in [
        (NetworkId::Mainnet, wallet.address()),
        (NetworkId::Sepolia, another.to_string()),
    ] {
        let state = state_of(&wallet, network, &address, &intent.nonce);
        let refused = wallet
            .core
            .sign_multisig_intent(intent.id.clone(), signer.clone(), state)
            .err()
            .unwrap();
        assert_eq!(
            reason(refused),
            "Argent multisig authority read is for another account",
            "{network:?} {address}"
        );
    }
    // A proposal's read names its multisig: the same address on Mainnet is no saved multisig.
    let read =
        bramble_core::MultisigAuthorityRead::new(NetworkId::Mainnet, wallet.address()).unwrap();
    let refused = wallet
        .core
        .clone()
        .propose_multisig_transaction(read, wallet.transfer(), wallet.tokens(), false)
        .err()
        .unwrap();
    assert_eq!(reason(refused), "Argent multisig not found");
}

/// An intent marked stale before it collected its threshold is submittable only once it has it.
#[test]
fn a_stale_intent_short_of_its_threshold_is_not_submitted() {
    let wallet = sepolia();
    let intent = wallet.propose();
    let moved = wallet.intent_state(&intent.id, wallet.threshold(), &wallet.guids(), "0x99");
    let cosigner = wallet.signer_id(
        wallet.net["proposal"]["cosignerAccountIndexes"][0]
            .as_u64()
            .unwrap(),
    );
    assert!(wallet
        .core
        .sign_multisig_intent(intent.id.clone(), cosigner, moved)
        .is_err());
    let refused = wallet
        .core
        .clone()
        .submit_multisig_intent(intent.id.clone(), wallet.current_state(&intent))
        .err()
        .unwrap();
    assert_eq!(
        reason(refused),
        "Argent multisig threshold is not satisfied"
    );
}

#[test]
fn removing_a_multisig_drops_its_intents() {
    let wallet = sepolia();
    wallet.propose();
    assert_eq!(
        wallet
            .core
            .multisig_intents(wallet.multisig_id())
            .unwrap()
            .len(),
        1
    );
    wallet.core.remove_multisig(wallet.multisig_id()).unwrap();
    assert!(wallet
        .core
        .multisig_intents(wallet.multisig_id())
        .unwrap()
        .is_empty());
}

#[test]
fn an_import_needs_a_package_and_a_finished_read() {
    let wallet = sepolia();
    let package = wallet.net["proposal"]["proposalPackage"]
        .as_str()
        .unwrap()
        .to_string();
    let unfinished = MultisigStateRead::new(wallet.network, wallet.address()).unwrap();
    let refused = wallet
        .core
        .import_multisig_intent(package, unfinished, wallet.tokens())
        .err()
        .unwrap();
    assert_eq!(reason(refused), "Argent multisig config unavailable");
    let state = wallet.state(wallet.threshold(), &wallet.guids(), "0x0");
    let refused = wallet
        .core
        .import_multisig_intent("{\"intent\":{}}".into(), state, wallet.tokens())
        .err()
        .unwrap();
    assert_eq!(reason(refused), "invalid Argent multisig intent package");
}
