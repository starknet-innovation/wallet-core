//! Bramble's Ledger choices over `wallet-core`'s device protocol (`wallet_core::ledger`): the
//! minimum Starknet app and device OS it accepts (`client.ts`), and the two path schemes it signs
//! at (`ledger-path.ts`), which the vault also validates, and the device a Ledger record signs
//! on (`LedgerScheme::device`).

#[cfg(feature = "mobile")]
use crate::account_store::{KeySource, StoredAccount};
use starknet_wallet_core::ledger::{DeviceModel, LedgerPath, PathComponent, Version};
#[cfg(feature = "mobile")]
use starknet_wallet_core::send::plan_device::DeviceKey;

const fn version(major: u16, minor: u16, patch: u16) -> Version {
    Version {
        major,
        minor,
        patch,
    }
}

/// `BRAMBLE_LEDGER_STARKNET_APP_MIN`: the oldest Starknet app Bramble accepts. A compatibility
/// floor, not a trust decision: every signature is still verified.
pub const LEDGER_APP_MIN: Version = version(2, 4, 0);

/// `BRAMBLE_LEDGER_OS_MIN`: by model, the oldest device OS with an official Starknet 2.4.0 build.
/// A model missing here is unsupported.
pub const LEDGER_OS_MIN: [(DeviceModel, Version); 5] = [
    (DeviceModel::Flex, version(1, 5, 1)),
    (DeviceModel::NanoGen5, version(1, 0, 5)),
    (DeviceModel::NanoSPlus, version(1, 5, 1)),
    (DeviceModel::NanoX, version(2, 6, 1)),
    (DeviceModel::Stax, version(1, 9, 1)),
];

/// `ledgerAccountPath(index, application)` (`PATH_SCHEMES`): Bramble's own accounts at
/// `LedgerW`, `m/2645'/1195502025'/734980059'/0'/<index>'/0`, and Ready's at `argentx`,
/// `m/2645'/1195502025'/1148870696'/0'/0'/<index>`. `None` for another application or an index
/// of 2^31 or more.
pub fn ledger_account_path(application: &str, account_index: u64) -> Option<LedgerPath> {
    let index = u32::try_from(account_index).ok()?;
    let h = PathComponent::hardened;
    let components = match application {
        "LedgerW" => [
            h(2645),
            h(1_195_502_025),
            h(734_980_059),
            h(0),
            h(index),
            PathComponent::plain(0),
        ],
        "argentx" => [
            h(2645),
            h(1_195_502_025),
            h(1_148_870_696),
            h(0),
            h(0),
            PathComponent::plain(index),
        ],
        _ => return None,
    };
    LedgerPath::new(components)
}

/// A Ledger record's path scheme (`LedgerEip2645Application`): Bramble's own (`LedgerW`) or
/// Ready's (`argentx`). A record's path is always its scheme's path at its index.
#[cfg(feature = "mobile")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum LedgerScheme {
    /// Bramble's own accounts (`LedgerW`).
    Bramble,
    /// Accounts Ready (Argent X) created (`argentx`).
    Ready,
}

#[cfg(feature = "mobile")]
impl LedgerScheme {
    pub(crate) fn from_application(application: &str) -> Option<Self> {
        match application {
            "LedgerW" => Some(Self::Bramble),
            "argentx" => Some(Self::Ready),
            _ => None,
        }
    }

    pub(crate) fn application(self) -> &'static str {
        match self {
            Self::Bramble => "LedgerW",
            Self::Ready => "argentx",
        }
    }

    /// The device a record at `account_index` signs on, with Bramble's app minimum.
    pub(crate) fn device(self, account_index: u32) -> Option<DeviceKey> {
        Some(DeviceKey {
            path: ledger_account_path(self.application(), account_index.into())?,
            app_minimum: LEDGER_APP_MIN,
        })
    }
}

/// The device `record` signs on; `None` unless it is a Ledger account.
#[cfg(feature = "mobile")]
pub(crate) fn record_device(record: &StoredAccount) -> Option<DeviceKey> {
    match record.key {
        KeySource::Ledger(scheme) => scheme.device(record.account_index),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "mobile")]
    #[test]
    fn a_scheme_signs_at_its_own_path() {
        let device = |scheme: LedgerScheme, index| scheme.device(index).unwrap().path.to_string();
        assert_eq!(
            device(LedgerScheme::Ready, 3),
            "m/2645'/1195502025'/1148870696'/0'/0'/3"
        );
        assert_eq!(
            device(LedgerScheme::Bramble, 3),
            "m/2645'/1195502025'/734980059'/0'/3'/0"
        );
        assert_eq!(
            LedgerScheme::Ready.device(3).unwrap().app_minimum,
            LEDGER_APP_MIN
        );
        assert!(LedgerScheme::Bramble.device(1 << 31).is_none());
        for scheme in [LedgerScheme::Bramble, LedgerScheme::Ready] {
            assert_eq!(
                LedgerScheme::from_application(scheme.application()),
                Some(scheme)
            );
        }
        assert_eq!(LedgerScheme::from_application("ledgerw"), None);
    }

    #[test]
    fn paths_are_the_extension_schemes() {
        // `ledger-path.test.ts`.
        let path = |application, index| {
            ledger_account_path(application, index).map(|path| path.to_string())
        };
        let max = 0x7fff_ffff;
        for (index, ledgerw, argentx) in [
            (
                0,
                "m/2645'/1195502025'/734980059'/0'/0'/0",
                "m/2645'/1195502025'/1148870696'/0'/0'/0",
            ),
            (
                12,
                "m/2645'/1195502025'/734980059'/0'/12'/0",
                "m/2645'/1195502025'/1148870696'/0'/0'/12",
            ),
            (
                max,
                "m/2645'/1195502025'/734980059'/0'/2147483647'/0",
                "m/2645'/1195502025'/1148870696'/0'/0'/2147483647",
            ),
        ] {
            assert_eq!(path("LedgerW", index).as_deref(), Some(ledgerw));
            assert_eq!(path("argentx", index).as_deref(), Some(argentx));
        }
        for application in ["LedgerW", "argentx"] {
            assert_eq!(path(application, max + 1), None);
            assert_eq!(path(application, u64::MAX), None);
        }
        for application in ["ledgerw", "ArgentX", "argent", ""] {
            assert_eq!(path(application, 0), None);
        }
    }

    #[test]
    fn minimums_are_the_extension_values() {
        assert_eq!(LEDGER_APP_MIN, version(2, 4, 0));
        let nano_x = LEDGER_OS_MIN
            .iter()
            .find(|(model, _)| *model == DeviceModel::NanoX);
        assert_eq!(nano_x, Some(&(DeviceModel::NanoX, version(2, 6, 1))));
    }
}
