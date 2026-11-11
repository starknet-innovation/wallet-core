//! What the host provides: key/value storage and a clock. The core reads time and touches
//! storage only through these, so a wallet adapts them to its platform (files on mobile,
//! `chrome.storage` in the extension) and its FFI.

use crate::Error;

/// Durable key/value storage.
pub trait Storage {
    /// The value under `key`, or `None` when there is none. Any other failure is an error.
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, Error>;
    /// Replaces the value under `key` atomically and durably: an interrupted write leaves the old
    /// value, and a completed one has reached the disk.
    fn write(&self, key: &str, value: &[u8]) -> Result<(), Error>;
    /// Moves the value under `from` to `to`, replacing any value there; an error when there is
    /// nothing under `from` or the move fails. The activity log sets a file it cannot read aside
    /// with it rather than overwrite it (`activity::store`).
    fn rename(&self, from: &str, to: &str) -> Result<(), Error>;
}

/// A clock that is monotonic and scoped to one boot.
///
/// There is deliberately no wall clock. Rules that depend on time (a passphrase backoff, an idle
/// lock) are measured on a clock the user cannot set, so changing the device's date cannot
/// shorten a delay or extend a session. A host that lies can only hurt its own user; the core
/// still never trusts a reading across a boot change or a backwards step.
pub trait Clock {
    /// Milliseconds on a clock that keeps counting while the device sleeps and that the user
    /// cannot set (iOS `mach_continuous_time`; Android `SystemClock.elapsedRealtime()`).
    fn monotonic_ms(&self) -> u64;
    /// An identifier that changes on every boot (iOS `kern.bootsessionuuid`; Android
    /// `Settings.Global.BOOT_COUNT`).
    fn boot_id(&self) -> String;
}

/// Longest boot identifier kept; a longer host value is cut, which only makes boots look equal
/// when a host returns absurdly long ids.
const MAX_BOOT_ID_LEN: usize = 128;

/// One reading of the host clock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClockReading {
    pub boot_id: String,
    pub monotonic_ms: u64,
}

impl ClockReading {
    pub fn read(clock: &dyn Clock) -> Self {
        let mut boot_id = clock.boot_id();
        if boot_id.len() > MAX_BOOT_ID_LEN {
            let mut end = MAX_BOOT_ID_LEN;
            while !boot_id.is_char_boundary(end) {
                end -= 1;
            }
            boot_id.truncate(end);
        }
        Self {
            boot_id,
            monotonic_ms: clock.monotonic_ms(),
        }
    }

    /// Milliseconds from `earlier` to `self`, or `None` when they cannot be compared: a different
    /// boot, or a clock that went backwards. Callers treat `None` as the strict outcome.
    pub fn elapsed_since(&self, earlier_boot: &str, earlier_ms: u64) -> Option<u64> {
        if self.boot_id != earlier_boot {
            return None;
        }
        self.monotonic_ms.checked_sub(earlier_ms)
    }
}

/// In-memory `Storage` for this crate's unit tests.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::cell::RefCell;
    use std::collections::BTreeMap;

    #[derive(Default)]
    pub(crate) struct Memory(pub(crate) RefCell<BTreeMap<String, Vec<u8>>>);

    impl Storage for Memory {
        fn read(&self, key: &str) -> Result<Option<Vec<u8>>, Error> {
            Ok(self.0.borrow().get(key).cloned())
        }
        fn write(&self, key: &str, value: &[u8]) -> Result<(), Error> {
            self.0.borrow_mut().insert(key.into(), value.to_vec());
            Ok(())
        }
        fn rename(&self, from: &str, to: &str) -> Result<(), Error> {
            let value = self.0.borrow_mut().remove(from);
            let value = value.ok_or_else(|| Error::storage("rename"))?;
            self.0.borrow_mut().insert(to.into(), value);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readings_only_compare_within_one_boot_and_forwards() {
        let now = ClockReading {
            boot_id: "a".into(),
            monotonic_ms: 100,
        };
        assert_eq!(now.elapsed_since("a", 40), Some(60));
        assert_eq!(now.elapsed_since("a", 100), Some(0));
        assert_eq!(now.elapsed_since("a", 101), None);
        assert_eq!(now.elapsed_since("b", 40), None);
    }

    struct LongBoot;
    impl Clock for LongBoot {
        fn monotonic_ms(&self) -> u64 {
            7
        }
        fn boot_id(&self) -> String {
            "é".repeat(100)
        }
    }

    #[test]
    fn long_boot_ids_are_cut_on_a_char_boundary() {
        let reading = ClockReading::read(&LongBoot);
        assert_eq!(reading.boot_id, "é".repeat(64));
        assert_eq!(reading.monotonic_ms, 7);
    }
}
