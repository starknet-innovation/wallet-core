//! The vault container, from `wallet_core::container`: a random data-encryption key (DEK) seals
//! the payload, and the DEK is itself sealed in slots: under the passphrase (PBKDF2) and,
//! optionally, under a device-held biometric secret.
//!
//! The shared core takes the vault version as an argument; here it is the vault's schema (13 or
//! 14, `vault_v13`), equal to the payload's `schemaVersion`. Containers of versions 2
//! to 5 (earlier test builds) still parse, so their password can be proven, and are then refused.
//! The payload is opaque bytes there; `vault_v13.rs` parses it. The DEK is generated, wrapped, unwrapped and zeroized in the shared core and never
//! crosses the FFI.

pub(crate) use wallet_core::container::*;
