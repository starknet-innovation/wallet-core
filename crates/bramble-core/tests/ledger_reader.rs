//! P8: the Ledger account picker. A scripted device holding the published phrase's account 0 key
//! at Bramble's index 0 answers a `LedgerKeyReader`; `ledger_account_candidates` places the
//! address, and its request makes the wallet.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

use crate::send_support;

use bramble_core::ledger::{ledger_account_path, LedgerScheme};
use bramble_core::{
    ledger_account_candidates, AccountFamily, CoreError, LedgerKey, LedgerKeyReader,
    LedgerReadStep, NetworkId,
};
use send_support::{empty_core, ACCOUNT_0, PASSWORD};
use starknet_wallet_core::ledger::{app_version_request, public_key_request, TransportFailure};
use starknet_wallet_core::Felt;

/// The published phrase's OpenZeppelin account 0 public key.
const KEY_0: &str = "0x426212993d56613e1886a4cbc5b58810570023581c2aab0b423277776b79d2e";

/// `getPubKey`'s reply for `key` (a format byte, X, an arbitrary Y, 0x9000).
fn key_reply(key: &str) -> Vec<u8> {
    let x = Felt::from_hex(key).unwrap().to_bytes_be();
    [&[0x04][..], &x, &[0x44; 32], &[0x90, 0x00]].concat()
}

fn apdu(step: LedgerReadStep) -> Vec<u8> {
    match step {
        LedgerReadStep::Send { apdu } => apdu,
        other => panic!("expected an APDU, got {other:?}"),
    }
}

#[test]
fn a_page_of_keys_becomes_candidates_and_a_wallet() {
    let reader = LedgerKeyReader::new(LedgerScheme::Bramble, 0, 2).unwrap();
    assert_eq!(apdu(reader.start()), app_version_request());
    let path = |index| ledger_account_path("LedgerW", index).unwrap();
    assert_eq!(
        apdu(reader.reply(vec![2, 4, 0, 0x90, 0x00])),
        public_key_request(&path(0))
    );
    assert_eq!(
        apdu(reader.reply(key_reply(KEY_0))),
        public_key_request(&path(1))
    );
    let LedgerReadStep::Keys { keys } = reader.reply(key_reply("0x1234")) else {
        panic!("expected the page's keys");
    };
    assert_eq!(
        keys,
        vec![
            LedgerKey {
                scheme: LedgerScheme::Bramble,
                account_index: 0,
                public_key_x: KEY_0.into(),
            },
            LedgerKey {
                scheme: LedgerScheme::Bramble,
                account_index: 1,
                public_key_x: "0x1234".into(),
            },
        ]
    );
    let candidates =
        ledger_account_candidates(NetworkId::Sepolia, AccountFamily::OpenZeppelin, keys).unwrap();
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].address, ACCOUNT_0);
    // The picked candidate's request makes the wallet at the address the picker showed.
    let (_dir, _clock, core) = empty_core();
    let record = core
        .create_ledger_wallet(PASSWORD.into(), candidates[0].request.clone())
        .unwrap();
    assert_eq!(record.address, candidates[0].address);
    assert!(record.ledger);
}

#[test]
fn a_refusal_returns_no_key_and_the_reader_ends() {
    let reader = LedgerKeyReader::new(LedgerScheme::Ready, 4, 1).unwrap();
    reader.start();
    assert_eq!(
        reader.transport_failure(TransportFailure::Locked),
        LedgerReadStep::Refused {
            reason: "device-locked".into()
        }
    );
    assert!(matches!(
        reader.reply(vec![2, 4, 0, 0x90, 0x00]),
        LedgerReadStep::Refused { .. }
    ));
}

/// Asked for its first step twice, a reader ends: a later reply moves nothing on.
#[test]
fn a_second_start_ends_the_reader() {
    let reader = LedgerKeyReader::new(LedgerScheme::Bramble, 0, 1).unwrap();
    assert_eq!(apdu(reader.start()), app_version_request());
    assert!(matches!(reader.start(), LedgerReadStep::Refused { .. }));
    assert!(matches!(
        reader.reply(vec![2, 4, 0, 0x90, 0x00]),
        LedgerReadStep::Refused { .. }
    ));
}

/// A refusing status or a short key mid-page returns no key; a device reporting a zero key
/// reaches no candidate.
#[test]
fn a_bad_reply_mid_page_returns_no_key() {
    for bad in [
        vec![0x69, 0x85],
        [&[0x04][..], &[0x11; 63], &[0x90, 0x00]].concat(),
    ] {
        let reader = LedgerKeyReader::new(LedgerScheme::Bramble, 0, 2).unwrap();
        reader.start();
        reader.reply(vec![2, 4, 0, 0x90, 0x00]);
        reader.reply(key_reply(KEY_0));
        assert!(
            matches!(reader.reply(bad.clone()), LedgerReadStep::Refused { .. }),
            "{bad:?}"
        );
    }
    let reader = LedgerKeyReader::new(LedgerScheme::Bramble, 0, 1).unwrap();
    reader.start();
    reader.reply(vec![2, 4, 0, 0x90, 0x00]);
    let LedgerReadStep::Keys { keys } = reader.reply(key_reply("0x0")) else {
        panic!("expected the page's keys");
    };
    assert_eq!(keys[0].public_key_x, "0x0");
    assert!(
        ledger_account_candidates(NetworkId::Sepolia, AccountFamily::OpenZeppelin, keys).is_err()
    );
}

#[test]
fn a_page_is_one_to_five_indexes_below_2_31() {
    let refused = |start, count| {
        matches!(
            LedgerKeyReader::new(LedgerScheme::Bramble, start, count),
            Err(CoreError::InvalidInput { .. })
        )
    };
    assert!(refused(0, 0));
    assert!(refused(0, 6));
    assert!(refused(0x7fff_ffff, 2));
    assert!(refused(u32::MAX, 1));
    assert!(!refused(0x7fff_fffb, 5));
}

#[test]
fn candidates_refuse_what_add_ledger_account_would() {
    let key = |index| LedgerKey {
        scheme: LedgerScheme::Ready,
        account_index: index,
        public_key_x: KEY_0.into(),
    };
    let candidates = |family, keys| ledger_account_candidates(NetworkId::Sepolia, family, keys);
    assert!(candidates(AccountFamily::Braavos, vec![key(0)]).is_err());
    assert!(candidates(AccountFamily::OpenZeppelin, (0..6).map(key).collect()).is_err());
    let mut zero = key(0);
    zero.public_key_x = "0x0".into();
    assert!(candidates(AccountFamily::OpenZeppelin, vec![zero]).is_err());
    assert!(candidates(AccountFamily::OpenZeppelin, vec![key(1 << 31)]).is_err());
    // An Argent account at Ready's scheme places at Bramble's Argent class.
    let argent = candidates(AccountFamily::Argent, vec![key(3)]).unwrap();
    assert_ne!(argent[0].address, ACCOUNT_0);
}

#[test]
fn a_candidate_takes_the_scheme_its_key_was_read_at() {
    let key = |scheme| LedgerKey {
        scheme,
        account_index: 0,
        public_key_x: KEY_0.into(),
    };
    let place =
        |keys| ledger_account_candidates(NetworkId::Sepolia, AccountFamily::OpenZeppelin, keys);
    let ready = place(vec![key(LedgerScheme::Ready)]).unwrap();
    assert_eq!(ready[0].request.scheme, LedgerScheme::Ready);
    let bramble = place(vec![key(LedgerScheme::Bramble)]).unwrap();
    assert_eq!(bramble[0].request.scheme, LedgerScheme::Bramble);
    assert!(matches!(
        place(vec![key(LedgerScheme::Bramble), key(LedgerScheme::Ready)]),
        Err(CoreError::InvalidInput { .. })
    ));
}
