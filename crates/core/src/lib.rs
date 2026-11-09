//! Bramble wallet core.
//!
//! One Rust library behind the browser extension (as a wasm32 module), iOS and Android. Today it
//! carries the KMS surface the extension's `KmsPort` needs (`kms`) and Nostr coordination
//! identity (`nostr`); the vault, accounts, transactions and the rest follow the phases in
//! `docs/PLAN.md`.
//!
//! Errors carry an operation name or a fixed reason, never input text, so nothing a user typed
//! can reach a log through an error message.

#![forbid(unsafe_code)]

// `is_private_key` only has callers behind `raw-keys` until the vault lands (Phase 2).
#[cfg_attr(not(feature = "raw-keys"), allow(dead_code))]
mod felt;
mod kms;
mod nostr;

#[cfg(feature = "raw-keys")]
pub use kms::{
    derive_argent_legacy_keypair, derive_starknet_keypair, derive_strk20_viewing_key,
    sign_stark_hash,
};
pub use kms::{
    derive_oz_account_address, generate_mnemonic, oz_account_class_hash, validate_mnemonic,
    NetworkId, StarkKeypair, StarkSignature, StarkXOnlyKeypair,
};
pub use nostr::{
    derive_nostr_public_key, open_nostr_application_data, wrap_nostr_application_data,
    NostrApplicationData,
};

// The wasm2 player needs the allocator and panic hook this crate exports; nothing else
// references it, so the linker would drop it without this line.
#[cfg(target_arch = "wasm32")]
extern crate uniffi_runtime_wasm as _;

uniffi::setup_scaffolding!();

/// The Krusty KMS release compiled into this core. `core_info_matches_the_lockfile` keeps it true.
pub const KRUSTY_VERSION: &str = "2.0.0";

/// Every failure the core reports across the FFI boundary.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum CoreError {
    #[error("invalid input: {reason}")]
    InvalidInput { reason: String },
    #[error("key management failure during {operation}")]
    Kms { operation: String },
}

impl CoreError {
    pub(crate) fn invalid(reason: &str) -> Self {
        Self::InvalidInput {
            reason: reason.into(),
        }
    }

    pub(crate) fn kms(operation: &str) -> Self {
        Self::Kms {
            operation: operation.into(),
        }
    }
}

/// Versions compiled into this core, for About screens, bug reports and the host's pin check.
#[derive(Debug, Clone, uniffi::Record)]
pub struct CoreInfo {
    pub core_version: String,
    pub krusty_version: String,
}

/// Build information.
#[uniffi::export]
pub fn core_info() -> CoreInfo {
    CoreInfo {
        core_version: env!("CARGO_PKG_VERSION").into(),
        krusty_version: KRUSTY_VERSION.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_info_matches_the_lockfile() {
        let lock = include_str!("../../../Cargo.lock");
        let resolved = lock
            .split("[[package]]")
            .find(|entry| entry.contains("name = \"krusty-kms\"\n"))
            .and_then(|entry| {
                entry
                    .lines()
                    .find_map(|line| line.strip_prefix("version = \"")?.strip_suffix('"'))
            })
            .expect("Cargo.lock resolves krusty-kms");
        assert_eq!(core_info().krusty_version, resolved);
        assert_eq!(core_info().core_version, env!("CARGO_PKG_VERSION"));
    }
}
