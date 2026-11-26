//! Ledger's Bluetooth LE APDU framing, sans-IO: the GATT identifiers per device model, the MTU
//! handshake, an APDU cut into write frames, and notification frames put back into one reply
//! (data then status word) for the readers in this module's parent.
//!
//! Sources (Apache-2.0): Ledger's `device-sdk-ts` at `9255a42e`
//! (`packages/device-management-kit/src/api/device-model/data/StaticDeviceModelDataSource.ts`
//! for the UUIDs; `.../device-session/data/FramerConst.ts`, `.../internal/device-session/service/
//! DefaultApdu{Sender,Receiver}Service.ts` for the framing; `packages/transport/web-ble/src/api/
//! transport/WebBleApduSender.ts` for the MTU handshake), and the archived `ledgerjs` at
//! `5df71e84` (`packages/devices/src/ble/{sendAPDU,receiveAPDU}.ts`), whose receiver rejects a
//! wrong tag, a sequence gap and an overrun as this one does. Ledger's DMK receiver checks none
//! of the three; this one follows ledgerjs.
//!
//! Frame layout (no channel, no padding on BLE): `0x05`, a big-endian u16 sequence number from
//! 0, then on frame 0 only a big-endian u16 total length, then data, all within the frame size.
//!
//! The framing carries bytes only: it never reads, alters or authorises an APDU's content. A
//! frame error is terminal for the exchange; the host reports it as a transport failure.

/// The tag every APDU frame starts with (`HEAD_TAG`, `TagId`).
pub const BLE_TAG: u8 = 0x05;

/// The MTU request the host writes once after subscribing (`[MTU_OP, 0, 0, 0, 0]`).
pub const MTU_REQUEST: [u8; 5] = [0x08, 0, 0, 0, 0];

const MTU_OP: u8 = 0x08;
/// Frame 0's header: tag, sequence, total length.
const FIRST_HEADER: usize = 5;
/// Every later frame's header: tag, sequence.
const NEXT_HEADER: usize = 3;
/// The frame size before a handshake answers, as ledgerjs assumes (ATT MTU 23 less 3).
pub const DEFAULT_FRAME_SIZE: usize = 20;

/// A device model's GATT service and characteristics, lowercase as Ledger writes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BleSpec {
    /// Ledger's product name.
    pub model: &'static str,
    /// The primary service a scan filters on.
    pub service: &'static str,
    /// Notifications: the device's reply frames and the MTU answer.
    pub notify: &'static str,
    /// Write with response.
    pub write: &'static str,
    /// Write without response (`writeCmdUuid`); the host prefers it when the characteristic
    /// allows it, as Ledger's transports do.
    pub write_cmd: &'static str,
}

const fn spec(model: &'static str, ids: [&'static str; 4]) -> BleSpec {
    BleSpec {
        model,
        service: ids[0],
        notify: ids[1],
        write: ids[2],
        write_cmd: ids[3],
    }
}

/// Every BLE device spec Ledger lists. Nano Gen5 (`APEX`) has two services.
pub const BLE_SPECS: [BleSpec; 5] = [
    spec(
        "Ledger Nano X",
        [
            "13d63400-2c97-0004-0000-4c6564676572",
            "13d63400-2c97-0004-0001-4c6564676572",
            "13d63400-2c97-0004-0002-4c6564676572",
            "13d63400-2c97-0004-0003-4c6564676572",
        ],
    ),
    spec(
        "Ledger Stax",
        [
            "13d63400-2c97-6004-0000-4c6564676572",
            "13d63400-2c97-6004-0001-4c6564676572",
            "13d63400-2c97-6004-0002-4c6564676572",
            "13d63400-2c97-6004-0003-4c6564676572",
        ],
    ),
    spec(
        "Ledger Flex",
        [
            "13d63400-2c97-3004-0000-4c6564676572",
            "13d63400-2c97-3004-0001-4c6564676572",
            "13d63400-2c97-3004-0002-4c6564676572",
            "13d63400-2c97-3004-0003-4c6564676572",
        ],
    ),
    spec(
        "Ledger Nano Gen5",
        [
            "13d63400-2c97-8004-0000-4c6564676572",
            "13d63400-2c97-8004-0001-4c6564676572",
            "13d63400-2c97-8004-0002-4c6564676572",
            "13d63400-2c97-8004-0003-4c6564676572",
        ],
    ),
    spec(
        "Ledger Nano Gen5",
        [
            "13d63400-2c97-9004-0000-4c6564676572",
            "13d63400-2c97-9004-0001-4c6564676572",
            "13d63400-2c97-9004-0002-4c6564676572",
            "13d63400-2c97-9004-0003-4c6564676572",
        ],
    ),
];

/// Why framing or reassembly stopped. Every variant ends the exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BleFrameError {
    /// An empty APDU, or one longer than the u16 length field.
    ApduLength,
    /// A frame size with no room for frame 0's header and one byte.
    FrameSize,
    /// The MTU answer is not `0x08 .. .. .. .. size`, or its size is too small.
    MtuReply,
    /// A frame too short for its header.
    ShortFrame,
    /// A frame whose first byte is not [`BLE_TAG`].
    WrongTag(u8),
    /// A sequence number other than the next one.
    OutOfOrder {
        /// The sequence number due.
        expected: u16,
        /// The one received.
        received: u16,
    },
    /// More bytes than frame 0 announced.
    TooMuchData,
    /// A reply too short to carry a status word.
    ShortReply,
    /// A frame after the reply completed or after an error.
    Closed,
}

/// The frame size the device answered to [`MTU_REQUEST`] (byte 5 of a `0x08` notification).
/// The host writes `min(this, its own maximum write length)`; a longer frame is not sent whole.
pub fn parse_mtu_reply(frame: &[u8]) -> Result<usize, BleFrameError> {
    match frame {
        [MTU_OP, _, _, _, _, size, ..] if usize::from(*size) > FIRST_HEADER => {
            Ok(usize::from(*size))
        }
        _ => Err(BleFrameError::MtuReply),
    }
}

/// `apdu` cut into frames of at most `frame_size` bytes, in write order.
pub fn frame_apdu(apdu: &[u8], frame_size: usize) -> Result<Vec<Vec<u8>>, BleFrameError> {
    let total = u16::try_from(apdu.len()).map_err(|_| BleFrameError::ApduLength)?;
    if total == 0 {
        return Err(BleFrameError::ApduLength);
    }
    if frame_size <= FIRST_HEADER {
        return Err(BleFrameError::FrameSize);
    }
    let mut frames = Vec::new();
    let mut rest = apdu;
    let mut seq: u16 = 0;
    while !rest.is_empty() {
        let header = if seq == 0 { FIRST_HEADER } else { NEXT_HEADER };
        let (chunk, tail) = rest.split_at(rest.len().min(frame_size - header));
        let mut frame = Vec::with_capacity(header + chunk.len());
        frame.push(BLE_TAG);
        frame.extend_from_slice(&seq.to_be_bytes());
        if seq == 0 {
            frame.extend_from_slice(&total.to_be_bytes());
        }
        frame.extend_from_slice(chunk);
        frames.push(frame);
        rest = tail;
        // Cannot overflow: at most 65 535 bytes, at least one per frame.
        seq = seq.wrapping_add(1);
    }
    Ok(frames)
}

/// Notification frames put back into one reply. One per exchange; after it returns the reply or
/// an error, every further frame is [`BleFrameError::Closed`].
#[derive(Debug, Default)]
pub struct Reassembler {
    total: usize,
    next: u16,
    data: Vec<u8>,
    closed: bool,
}

impl Reassembler {
    /// An empty reassembler, waiting for frame 0.
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes one notification. `Ok(None)`: more frames are due. `Ok(Some(reply))`: the reply,
    /// data then status word, for the parent module's readers.
    pub fn push(&mut self, frame: &[u8]) -> Result<Option<Vec<u8>>, BleFrameError> {
        if self.closed {
            return Err(BleFrameError::Closed);
        }
        let result = self.take(frame);
        if !matches!(result, Ok(None)) {
            self.closed = true;
        }
        result
    }

    fn take(&mut self, frame: &[u8]) -> Result<Option<Vec<u8>>, BleFrameError> {
        let [tag, hi, lo, body @ ..] = frame else {
            return Err(BleFrameError::ShortFrame);
        };
        if *tag != BLE_TAG {
            return Err(BleFrameError::WrongTag(*tag));
        }
        let received = u16::from_be_bytes([*hi, *lo]);
        if received != self.next {
            return Err(BleFrameError::OutOfOrder {
                expected: self.next,
                received,
            });
        }
        let body = if received == 0 {
            let [len_hi, len_lo, rest @ ..] = body else {
                return Err(BleFrameError::ShortFrame);
            };
            self.total = usize::from(u16::from_be_bytes([*len_hi, *len_lo]));
            if self.total < 2 {
                return Err(BleFrameError::ShortReply);
            }
            rest
        } else {
            body
        };
        if self.data.len() + body.len() > self.total {
            return Err(BleFrameError::TooMuchData);
        }
        self.data.extend_from_slice(body);
        if self.data.len() == self.total {
            return Ok(Some(std::mem::take(&mut self.data)));
        }
        // Only empty frames can reach u16::MAX; past it the sequence cannot continue.
        self.next = self.next.checked_add(1).ok_or(BleFrameError::TooMuchData)?;
        Ok(None)
    }
}
