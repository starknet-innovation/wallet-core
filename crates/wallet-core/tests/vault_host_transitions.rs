//! Tests for host clock and password-envelope transitions based on the Quint vault spec.
//! Each test exercises a specific path (transition) through a vault operation.

use starknet_wallet_core::envelope::{open, SealingKey};
use starknet_wallet_core::host::ClockReading;
use starknet_wallet_core::Error;

// ================================================================
// Tests for Clock operations
// ================================================================

/// ClockReading::elapsed_since with monotonic increase.
/// Transition: "Advance clock forward" (d8d40f77630a5001)
#[test]
fn clock_reading_elapsed_since_with_advance() {
    let earlier = ClockReading {
        boot_id: "boot1".to_string(),
        monotonic_ms: 100,
    };
    let now = ClockReading {
        boot_id: "boot1".to_string(),
        monotonic_ms: 160,
    };

    let elapsed = now.elapsed_since(&earlier.boot_id, earlier.monotonic_ms);
    assert_eq!(elapsed, Some(60));
}

/// ClockReading::elapsed_since when clock moves backwards.
/// Transition: "Move clock backward" (dcfdf70fd2a3c993)
#[test]
fn clock_reading_elapsed_since_backward() {
    let earlier = ClockReading {
        boot_id: "boot1".to_string(),
        monotonic_ms: 100,
    };
    let now = ClockReading {
        boot_id: "boot1".to_string(),
        monotonic_ms: 50,
    };

    let elapsed = now.elapsed_since(&earlier.boot_id, earlier.monotonic_ms);
    assert_eq!(elapsed, None);
}

/// ClockReading across different boot IDs.
#[test]
fn clock_reading_elapsed_since_different_boot() {
    let earlier = ClockReading {
        boot_id: "boot1".to_string(),
        monotonic_ms: 100,
    };
    let now = ClockReading {
        boot_id: "boot2".to_string(),
        monotonic_ms: 200,
    };

    let elapsed = now.elapsed_since(&earlier.boot_id, earlier.monotonic_ms);
    assert_eq!(elapsed, None);
}

// ================================================================
// Tests for Envelope operations
// ================================================================

/// open returns the plaintext sealed in the envelope with correct password.
#[test]
fn open_returns_sealed_plaintext_with_correct_password() {
    let key = SealingKey::create("correct passphrase").unwrap();
    let plaintext = b"secret data";
    let scope = "test.scope";

    let envelope = key.seal(plaintext, scope).unwrap();
    let (opened, _) = open(&envelope, "correct passphrase", scope).unwrap();

    assert_eq!(opened.as_slice(), plaintext);
}

/// open rejects wrong password.
#[test]
fn open_rejects_wrong_password() {
    let key = SealingKey::create("correct passphrase").unwrap();
    let plaintext = b"secret data";
    let scope = "test.scope";

    let envelope = key.seal(plaintext, scope).unwrap();
    let result = open(&envelope, "wrong passphrase", scope);

    assert!(matches!(result, Err(Error::WrongPassword)));
}

/// open only accepts sealed scope.
#[test]
fn open_only_accepts_sealed_scope() {
    let key = SealingKey::create("passphrase").unwrap();
    let envelope = key.seal(b"data", "scope.a").unwrap();

    let result = open(&envelope, "passphrase", "scope.b");
    assert!(result.is_err());
}
