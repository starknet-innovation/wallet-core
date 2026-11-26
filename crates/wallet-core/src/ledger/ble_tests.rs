//! BLE framing against Ledger's own vectors: `device-sdk-ts` `9255a42e`
//! `DefaultApduSenderService.test.ts` and `DefaultApduReceiverService.test.ts`, the "[BLE]
//! Without padding nor channel" cases (the receiver's frames are its USB captures less their
//! two channel bytes, as that test slices them), plus ledgerjs `receiveAPDU.ts`'s three refusals.

use super::ble::*;

const GET_VERSION: [u8; 5] = [0xe0, 0x01, 0x00, 0x00, 0x00];

#[test]
fn one_frame_at_123() {
    let frames = frame_apdu(&GET_VERSION, 123).unwrap();
    assert_eq!(
        frames,
        vec![vec![
            0x05, 0x00, 0x00, 0x00, 0x05, 0xe0, 0x01, 0x00, 0x00, 0x00
        ]]
    );
}

#[test]
fn three_frames_at_10() {
    let apdu = [
        0x01, 0x05, 0x4f, 0x4c, 0x4f, 0x53, 0x00, 0x07, 0x2e, 0x32, 0x2e, 0x34, 0x2d, 0x32, 0x00,
        0x90, 0x00,
    ];
    let frames = frame_apdu(&apdu, 10).unwrap();
    assert_eq!(
        frames,
        vec![
            vec![0x05, 0x00, 0x00, 0x00, 0x11, 0x01, 0x05, 0x4f, 0x4c, 0x4f],
            vec![0x05, 0x00, 0x01, 0x53, 0x00, 0x07, 0x2e, 0x32, 0x2e, 0x34],
            vec![0x05, 0x00, 0x02, 0x2d, 0x32, 0x00, 0x90, 0x00],
        ]
    );
}

#[test]
fn framing_refuses_bad_sizes() {
    assert_eq!(frame_apdu(&[], 20), Err(BleFrameError::ApduLength));
    assert_eq!(
        frame_apdu(&vec![0; 65_536], 20),
        Err(BleFrameError::ApduLength)
    );
    assert_eq!(frame_apdu(&GET_VERSION, 5), Err(BleFrameError::FrameSize));
    assert_eq!(frame_apdu(&GET_VERSION, 6).unwrap().len(), 3);
}

#[test]
fn frames_round_trip_through_the_reassembler() {
    let apdu: Vec<u8> = (0..=255u8).cycle().take(1_000).collect();
    for size in [6, 20, 23, 153, 156, 255] {
        let mut r = Reassembler::new();
        let frames = frame_apdu(&apdu, size).unwrap();
        let (last, init) = frames.split_last().unwrap();
        for f in init {
            assert!(f.len() <= size);
            assert_eq!(r.push(f), Ok(None));
        }
        assert_eq!(r.push(last), Ok(Some(apdu.clone())));
    }
}

/// `RESPONSE_GET_VERSION` sliced `[2, 40)`.
const GET_VERSION_REPLY: [u8; 38] = [
    0x05, 0x00, 0x00, 0x00, 0x21, 0x33, 0x00, 0x00, 0x04, 0x05, 0x32, 0x2e, 0x32, 0x2e, 0x33, 0x04,
    0xe6, 0x00, 0x00, 0x00, 0x04, 0x32, 0x2e, 0x33, 0x30, 0x04, 0x31, 0x2e, 0x31, 0x36, 0x01, 0x01,
    0x01, 0x00, 0x01, 0x00, 0x90, 0x00,
];

#[test]
fn one_frame_reply() {
    let mut r = Reassembler::new();
    assert_eq!(
        r.push(&GET_VERSION_REPLY),
        Ok(Some(GET_VERSION_REPLY[5..].to_vec()))
    );
    assert_eq!(r.push(&GET_VERSION_REPLY), Err(BleFrameError::Closed));
}

#[test]
fn locked_device_reply() {
    let mut r = Reassembler::new();
    let frame = [0x05, 0x00, 0x00, 0x00, 0x02, 0x55, 0x15];
    assert_eq!(r.push(&frame), Ok(Some(vec![0x55, 0x15])));
}

#[test]
fn three_frame_list_apps_reply() {
    let f0: [u8; 62] = [
        0x05, 0x00, 0x00, 0x00, 0x9e, 0x01, 0x4d, 0x00, 0x13, 0xca, 0x50, 0xfa, 0xa1, 0x91, 0x40,
        0x9a, 0x6b, 0xfa, 0x6c, 0x0f, 0xbb, 0xb2, 0xe7, 0xc4, 0xa9, 0xcf, 0xe5, 0x57, 0x41, 0x00,
        0x5d, 0xbd, 0x84, 0xab, 0x9a, 0xbd, 0x66, 0xc7, 0x6c, 0x90, 0xdd, 0x08, 0x79, 0x0d, 0x08,
        0x47, 0xb9, 0x3a, 0x8f, 0xa7, 0x6f, 0x60, 0x33, 0xae, 0xd3, 0x25, 0xd7, 0xb1, 0xe5, 0x7c,
        0xeb, 0xd7,
    ];
    let f1: [u8; 62] = [
        0x05, 0x00, 0x01, 0x4b, 0x2e, 0x2c, 0x9f, 0xb4, 0x46, 0x78, 0xde, 0x05, 0x5f, 0x9e, 0x80,
        0x0a, 0x07, 0x42, 0x69, 0x74, 0x63, 0x6f, 0x69, 0x6e, 0x4e, 0x00, 0x15, 0xca, 0x40, 0x06,
        0x03, 0x28, 0xf8, 0x8f, 0xc6, 0xd6, 0x42, 0x98, 0xd0, 0x49, 0x00, 0xc7, 0x04, 0x98, 0x19,
        0x1b, 0x6c, 0xeb, 0xed, 0xd8, 0xcb, 0x84, 0x5d, 0xf5, 0x4b, 0xe3, 0xbd, 0xbb, 0x25, 0x7a,
        0x3f, 0x6f,
    ];
    let f2: [u8; 45] = [
        0x05, 0x00, 0x02, 0x68, 0x8f, 0x54, 0xef, 0x7f, 0xaa, 0xc4, 0x22, 0xaa, 0x54, 0xe7, 0xb8,
        0x0a, 0xc8, 0xa3, 0x2f, 0x96, 0xe5, 0x5e, 0x43, 0x2d, 0xf3, 0xa3, 0x45, 0x8d, 0x8e, 0xaa,
        0xf1, 0x4e, 0xd1, 0x1e, 0x08, 0x45, 0x74, 0x68, 0x65, 0x72, 0x65, 0x75, 0x6d, 0x90, 0x00,
    ];
    let mut r = Reassembler::new();
    assert_eq!(r.push(&f0), Ok(None));
    assert_eq!(r.push(&f1), Ok(None));
    let reply = r.push(&f2).unwrap().unwrap();
    let expected: Vec<u8> = [&f0[5..], &f1[3..], &f2[3..]].concat();
    assert_eq!(reply.len(), 0x9e);
    assert_eq!(reply, expected);
    assert_eq!(reply[reply.len() - 2..], [0x90, 0x00]);
}

#[test]
fn refuses_wrong_tag_gap_overrun_and_short() {
    let refuse = |frames: &[&[u8]]| {
        let mut r = Reassembler::new();
        let (last, init) = frames.split_last().unwrap();
        for f in init {
            assert_eq!(r.push(f), Ok(None));
        }
        let err = r.push(last).unwrap_err();
        assert_eq!(r.push(&GET_VERSION_REPLY), Err(BleFrameError::Closed));
        err
    };
    assert_eq!(
        refuse(&[&[0x08, 0, 0, 0, 2, 0x90, 0]]),
        BleFrameError::WrongTag(0x08)
    );
    assert_eq!(
        refuse(&[&[0x05, 0, 1, 0x90, 0]]),
        BleFrameError::OutOfOrder {
            expected: 0,
            received: 1
        }
    );
    assert_eq!(
        refuse(&[&[0x05, 0, 0, 0, 4, 1, 2], &[0x05, 0, 2, 0x90, 0]]),
        BleFrameError::OutOfOrder {
            expected: 1,
            received: 2
        }
    );
    assert_eq!(
        refuse(&[&[0x05, 0, 0, 0, 4, 1, 2], &[0x05, 0, 0, 0, 4, 1, 2]]),
        BleFrameError::OutOfOrder {
            expected: 1,
            received: 0
        }
    );
    assert_eq!(
        refuse(&[&[0x05, 0, 0, 0, 2, 0x90, 0, 0]]),
        BleFrameError::TooMuchData
    );
    assert_eq!(
        refuse(&[&[0x05, 0, 0, 0, 2, 1], &[0x05, 0, 1, 0x90, 0]]),
        BleFrameError::TooMuchData
    );
    assert_eq!(refuse(&[&[0x05, 0]]), BleFrameError::ShortFrame);
    assert_eq!(refuse(&[&[0x05, 0, 0, 0]]), BleFrameError::ShortFrame);
    assert_eq!(
        refuse(&[&[0x05, 0, 0, 0, 1, 0x90]]),
        BleFrameError::ShortReply
    );
}

#[test]
fn mtu_reply() {
    // `WebBleApduSender.ts`: frame size is byte 5 of a notification starting 0x08.
    assert_eq!(parse_mtu_reply(&[0x08, 0, 0, 0, 1, 0x99]), Ok(0x99));
    assert_eq!(parse_mtu_reply(&[0x08, 0, 0, 0, 1, 0x99, 7]), Ok(0x99));
    assert_eq!(
        parse_mtu_reply(&[0x08, 0, 0, 0, 1]),
        Err(BleFrameError::MtuReply)
    );
    assert_eq!(
        parse_mtu_reply(&[0x05, 0, 0, 0, 1, 0x99]),
        Err(BleFrameError::MtuReply)
    );
    assert_eq!(
        parse_mtu_reply(&[0x08, 0, 0, 0, 1, 5]),
        Err(BleFrameError::MtuReply)
    );
    assert_eq!(MTU_REQUEST, [0x08, 0, 0, 0, 0]);
}

#[test]
fn specs_are_distinct_and_share_ledger_base() {
    for (i, s) in BLE_SPECS.iter().enumerate() {
        for id in [s.service, s.notify, s.write, s.write_cmd] {
            assert!(id.starts_with("13d63400-2c97-") && id.ends_with("-4c6564676572"));
        }
        assert!(BLE_SPECS[i + 1..].iter().all(|o| o.service != s.service));
    }
}
