//! P8: Ledger's Bluetooth framing as the apps see it. A GetVersion APDU goes out as frames and a
//! reply comes back through `LedgerBleExchange` into a `LedgerKeyReader`, as each app's shim
//! drives it; malformed frames end the exchange as a transport failure.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

use bramble_core::ledger::LedgerScheme;
use bramble_core::{
    ledger_ble_frame_size, ledger_ble_link, LedgerBleExchange, LedgerBleStep, LedgerKeyReader,
    LedgerReadStep,
};
use starknet_wallet_core::ledger::TransportFailure;

/// `apdu`'s reply as one frame: tag, sequence 0, length, then the bytes.
fn one_frame(reply: &[u8]) -> Vec<u8> {
    let len = u16::try_from(reply.len()).unwrap().to_be_bytes();
    [&[0x05, 0, 0, len[0], len[1]][..], reply].concat()
}

#[test]
fn every_model_is_listed_with_four_lowercase_identifiers() {
    let specs = ledger_ble_link().specs;
    assert_eq!(specs.len(), 5);
    let nano_x = specs
        .iter()
        .find(|spec| spec.model == "Ledger Nano X")
        .unwrap();
    assert_eq!(nano_x.service, "13d63400-2c97-0004-0000-4c6564676572");
    assert_eq!(nano_x.notify, "13d63400-2c97-0004-0001-4c6564676572");
    assert_eq!(nano_x.write, "13d63400-2c97-0004-0002-4c6564676572");
    assert_eq!(nano_x.write_cmd, "13d63400-2c97-0004-0003-4c6564676572");
    for spec in &specs {
        for id in [&spec.service, &spec.notify, &spec.write, &spec.write_cmd] {
            assert_eq!(id, &id.to_lowercase());
        }
    }
}

#[test]
fn the_mtu_handshake_sets_the_frame_size() {
    let link = ledger_ble_link();
    assert_eq!(link.mtu_request, vec![0x08, 0, 0, 0, 0]);
    assert_eq!(link.default_frame_size, 20);
    assert_eq!(
        ledger_ble_frame_size(vec![0x08, 0, 0, 0, 1, 153]).unwrap(),
        153
    );
    assert!(ledger_ble_frame_size(vec![0x05, 0, 0, 0, 1, 153]).is_err());
    assert!(ledger_ble_frame_size(vec![0x08, 0, 0, 0, 1, 5]).is_err());
}

#[test]
fn a_reader_s_apdu_goes_out_framed_and_its_reply_comes_back_whole() {
    let reader = LedgerKeyReader::new(LedgerScheme::Bramble, 0, 1).unwrap();
    let LedgerReadStep::Send { apdu } = reader.start() else {
        panic!("expected an APDU");
    };
    let exchange = LedgerBleExchange::new(apdu.clone(), 20).unwrap();
    let frames = exchange.frames();
    let sent: Vec<u8> = frames
        .iter()
        .map(|frame| &frame.bytes)
        .enumerate()
        .flat_map(|(seq, frame)| {
            assert_eq!(frame[0], 0x05);
            assert_eq!(u16::from_be_bytes([frame[1], frame[2]]), seq as u16);
            assert!(frame.len() <= 20);
            frame[if seq == 0 { 5 } else { 3 }..].to_vec()
        })
        .collect();
    assert_eq!(sent, apdu);

    // The app version 2.4.0 then 9000, split over two notifications.
    let reply = [2, 4, 0, 0x90, 0x00];
    let whole = one_frame(&reply);
    assert_eq!(exchange.push(whole[..6].to_vec()), LedgerBleStep::More);
    let mut rest = vec![0x05, 0, 1];
    rest.extend_from_slice(&whole[6..]);
    let LedgerBleStep::Reply { reply: got } = exchange.push(rest) else {
        panic!("expected the whole reply");
    };
    assert_eq!(got, reply);
    // The reader takes it and asks for the key next.
    assert!(matches!(reader.reply(got), LedgerReadStep::Send { .. }));
}

#[test]
fn malformed_frames_end_the_exchange_as_a_transport_failure() {
    let failed = LedgerBleStep::Failed {
        failure: TransportFailure::Other,
    };
    let exchange = LedgerBleExchange::new(vec![0xe0, 0x01, 0, 0, 0], 20).unwrap();
    assert_eq!(exchange.push(vec![0x08, 0, 0, 0, 2, 0x90, 0]), failed);
    // Closed: even a good frame is refused now.
    assert_eq!(exchange.push(one_frame(&[0x90, 0])), failed);

    let exchange = LedgerBleExchange::new(vec![0xe0, 0x01, 0, 0, 0], 20).unwrap();
    assert_eq!(exchange.push(vec![0x05, 0, 1, 0, 2, 0x90, 0]), failed);

    let exchange = LedgerBleExchange::new(vec![0xe0, 0x01, 0, 0, 0], 20).unwrap();
    assert_eq!(exchange.push(vec![0x05, 0, 0, 0, 2, 0x90, 0, 0xff]), failed);
}

#[test]
fn an_empty_apdu_or_a_frame_with_no_room_is_refused() {
    assert!(LedgerBleExchange::new(vec![], 20).is_err());
    assert!(LedgerBleExchange::new(vec![0xe0], 5).is_err());
    assert!(LedgerBleExchange::new(vec![0; 65_536], 20).is_err());
    assert!(LedgerBleExchange::new(vec![0xe0], 6).is_ok());
}
