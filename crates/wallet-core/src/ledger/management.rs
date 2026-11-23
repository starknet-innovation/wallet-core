//! The device's own commands around the Starknet app (`device-transition.ts`): which app is
//! open, the OS version (read only from the dashboard), closing the open app and opening
//! Starknet; then the device model and the firmware gate (`client.ts`). The sequence that leaves
//! the app, reads the OS and reopens Starknet, reconnecting after each change, is the host's
//! session and not here.

use super::rejection::{thrown, LedgerRejection, RejectionCode, Thrown};
use super::{apdu, split, Reply, Version, STATUS_OK};

/// The name the Starknet app reports and `OpenApp` takes.
pub const STARKNET_APP_NAME: &str = "Starknet";
/// Names the dashboard reports for itself.
const DASHBOARD_NAMES: [&str; 3] = ["BOLOS", "OLOS", "OLOS\0"];

const MAX_RESPONSE_BYTES: usize = 260;
const MAX_APP_NAME_BYTES: usize = 64;
const MAX_VERSION_BYTES: usize = 32;
const MAX_FLAGS_BYTES: usize = 64;
const MAX_TRAILING_FIELD_BYTES: usize = 64;
/// Fields after the ones read, which newer firmware may append; bounded so garbage is refused.
const MAX_TRAILING_FIELDS: usize = 5;

/// `GetAppAndVersion` (`APP_STATE_APDU`).
pub fn app_state_request() -> Vec<u8> {
    apdu(0xb0, 0x01, 0, 0, [])
}

/// `GetOsVersion` (`OS_VERSION_APDU`), answered only by the dashboard.
pub fn os_version_request() -> Vec<u8> {
    apdu(0xe0, 0x01, 0, 0, [])
}

/// `CloseApp` (`QUIT_APP_APDU`): back to the dashboard. The link may drop as the app closes.
pub fn quit_app_request() -> Vec<u8> {
    apdu(0xb0, 0xa7, 0, 0, [])
}

/// `OpenApp` (`OPEN_APP_APDU`) with the Starknet app's name. The link may drop as it opens.
pub fn open_starknet_request() -> Vec<u8> {
    let name: [u8; 8] = *b"Starknet";
    apdu(0xe0, 0xd8, 0, 0, name)
}

/// The open app, as `GetAppAndVersion` names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppState {
    pub name: String,
    pub version: String,
}

impl AppState {
    /// Whether the device is on its dashboard rather than in an app.
    pub fn is_dashboard(&self) -> bool {
        DASHBOARD_NAMES.contains(&self.name.as_str())
    }
}

/// `GetOsVersion`'s target id and OS version, both untrusted until checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OsVersion {
    pub target_id: u32,
    pub version: String,
}

/// A Ledger model with a Starknet 2.4 build, from its target id's top two bytes (Ledger's masks,
/// `@ledgerhq/devices` 8.18.0 `identifyTargetId`; the older Blue and Nano S have none).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeviceModel {
    NanoX,
    NanoSPlus,
    Stax,
    Flex,
    NanoGen5,
}

impl DeviceModel {
    /// `ledgerModelFromTargetId`. Low bits name a hardware revision and are ignored.
    fn from_target_id(target_id: u32) -> Option<Self> {
        match target_id & 0xffff_0000 {
            0x3300_0000 => Some(Self::NanoX),
            0x3310_0000 => Some(Self::NanoSPlus),
            0x3320_0000 => Some(Self::Stax),
            0x3330_0000 => Some(Self::Flex),
            0x3340_0000 => Some(Self::NanoGen5),
            _ => None,
        }
    }
}

/// A supported model and its parsed OS version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Firmware {
    pub model: DeviceModel,
    pub version: Version,
}

fn unreadable() -> LedgerRejection {
    LedgerRejection::new(RejectionCode::ManagementUnreadable)
}

/// A management reply's data. The status must be success, or the command was refused.
fn data_of(reply: Reply<'_>) -> Result<&[u8], LedgerRejection> {
    let reply = reply.map_err(|failure| thrown(Thrown::Transport(failure)))?;
    if reply.len() > MAX_RESPONSE_BYTES {
        return Err(unreadable());
    }
    match split(reply) {
        None => Err(unreadable()),
        Some((data, STATUS_OK)) => Ok(data),
        Some((_, status)) => Err(LedgerRejection::with_status(
            RejectionCode::ManagementRefused,
            status,
        )),
    }
}

/// `readLv`: one length-prefixed field at the front of `data`, and what follows it.
fn field(data: &[u8], maximum: usize) -> Result<(&[u8], &[u8]), LedgerRejection> {
    let (&length, rest) = data.split_first().ok_or_else(unreadable)?;
    let length = usize::from(length);
    if length > maximum {
        return Err(unreadable());
    }
    rest.split_at_checked(length).ok_or_else(unreadable)
}

/// Skips the fields after the ones read, at most [`MAX_TRAILING_FIELDS`] of them.
fn trailing(mut data: &[u8], maximum: usize) -> Result<(), LedgerRejection> {
    for _ in 0..MAX_TRAILING_FIELDS {
        if data.is_empty() {
            return Ok(());
        }
        data = field(data, maximum)?.1;
    }
    if data.is_empty() {
        Ok(())
    } else {
        Err(unreadable())
    }
}

/// `TextDecoder("utf-8", { fatal: true })`: strict UTF-8, a leading byte-order mark dropped.
fn decode(bytes: &[u8]) -> Result<String, LedgerRejection> {
    let text = std::str::from_utf8(bytes).map_err(|_| unreadable())?;
    Ok(text.strip_prefix('\u{feff}').unwrap_or(text).to_string())
}

/// `parseAppState`: a format byte of 1, the name, the version, then flags and any fields newer
/// firmware appends.
pub fn parse_app_state(reply: Reply<'_>) -> Result<AppState, LedgerRejection> {
    let data = data_of(reply)?;
    let (&format, data) = data.split_first().ok_or_else(unreadable)?;
    if format != 1 {
        return Err(unreadable());
    }
    let (name, data) = field(data, MAX_APP_NAME_BYTES)?;
    let (version, data) = field(data, MAX_VERSION_BYTES)?;
    trailing(data, MAX_FLAGS_BYTES)?;
    Ok(AppState {
        name: decode(name)?,
        version: decode(version)?,
    })
}

/// `parseOsVersion`: the target id (4 bytes), the OS version (non-empty), flags, the MCU version
/// (printable ASCII), then the optional fields Ledger's envelope carries.
pub fn parse_os_version(reply: Reply<'_>) -> Result<OsVersion, LedgerRejection> {
    let data = data_of(reply)?;
    let (target_id, data) = data.split_first_chunk::<4>().ok_or_else(unreadable)?;
    let (version, data) = field(data, MAX_VERSION_BYTES)?;
    if version.is_empty() {
        return Err(unreadable());
    }
    let (_flags, data) = field(data, MAX_FLAGS_BYTES)?;
    let (mcu, data) = field(data, MAX_VERSION_BYTES)?;
    let mcu = decode(mcu)?;
    if mcu.is_empty() || !mcu.bytes().all(|b| (0x20..=0x7e).contains(&b)) {
        return Err(unreadable());
    }
    trailing(data, MAX_TRAILING_FIELD_BYTES)?;
    Ok(OsVersion {
        target_id: u32::from_be_bytes(*target_id),
        version: decode(version)?,
    })
}

/// `versionFromString`: `major.minor.patch`, decimal without leading zeros, each at most
/// `0xffff`.
fn version_from_text(text: &str) -> Option<Version> {
    let part = |digits: &str| {
        let canonical = !digits.is_empty()
            && digits.bytes().all(|b| b.is_ascii_digit())
            && (digits == "0" || !digits.starts_with('0'));
        canonical.then(|| digits.parse::<u16>().ok()).flatten()
    };
    let mut parts = text.split('.');
    let version = Version {
        major: part(parts.next()?)?,
        minor: part(parts.next()?)?,
        patch: part(parts.next()?)?,
    };
    parts.next().is_none().then_some(version)
}

/// `ledgerFirmwareVersion`: the model first (an unknown one is unsupported, never given another
/// model's floor), then the version.
pub fn firmware(os: &OsVersion) -> Result<Firmware, LedgerRejection> {
    let model = DeviceModel::from_target_id(os.target_id)
        .ok_or(LedgerRejection::new(RejectionCode::UnsupportedLedgerDevice))?;
    let version = version_from_text(&os.version)
        .ok_or(LedgerRejection::new(RejectionCode::UnexpectedDeviceError))?;
    Ok(Firmware { model, version })
}

/// `assertLedgerFirmwareReady`: refuses an OS below the wallet's minimum for its model. A model
/// the wallet gives no minimum for is unsupported.
pub fn check_firmware(
    firmware: &Firmware,
    minimums: &[(DeviceModel, Version)],
) -> Result<(), LedgerRejection> {
    let (_, minimum) = minimums
        .iter()
        .find(|(model, _)| *model == firmware.model)
        .ok_or(LedgerRejection::new(RejectionCode::UnsupportedLedgerDevice))?;
    if firmware.version >= *minimum {
        Ok(())
    } else {
        Err(LedgerRejection::new(RejectionCode::FirmwareBelowMinimum))
    }
}
