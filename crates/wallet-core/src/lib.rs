//! Starknet wallet core.
//!
//! Key custody through Krusty KMS (`kms`), Nostr coordination identity (`nostr`) and the felt
//! grammar they share (`felt`), as plain Rust. This crate holds mechanism only: product choices
//! (which account version a wallet deploys, which salt, what crosses an FFI boundary) are
//! arguments here and constants in the wallet's own crate (`bramble-core` for Bramble). It has
//! no FFI of its own; a wallet exports what it needs from its crate.
//!
//! Errors carry an operation name or a fixed reason, never input text, so nothing a user typed
//! can reach a log through an error message.

#![forbid(unsafe_code)]

pub mod felt;
pub mod kms;
pub mod nostr;

pub use kms::{
    derive_argent_legacy_keypair, derive_oz_account_address, derive_starknet_keypair,
    derive_strk20_viewing_key, generate_mnemonic, oz_account_class_hash, sign_stark_hash,
    stark_public_key, validate_mnemonic, NetworkId, StarkKeypair, StarkSignature,
    StarkXOnlyKeypair,
};
pub use krusty_kms_common::SecretFelt;
pub use nostr::{
    derive_nostr_public_key, open_nostr_application_data, open_nostr_event,
    wrap_nostr_application_data, NostrApplicationData, NostrEvent,
};
pub use zeroize::Zeroizing;

/// The Krusty KMS release compiled into this crate. `krusty_version_matches_the_lockfile` keeps
/// it true.
pub const KRUSTY_VERSION: &str = "2.0.0";

/// Every failure this crate reports.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("invalid input: {reason}")]
    InvalidInput { reason: String },
    #[error("key management failure during {operation}")]
    Kms { operation: String },
}

impl Error {
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

#[cfg(test)]
mod tests {
    #[test]
    fn krusty_version_matches_the_lockfile() {
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
        assert_eq!(super::KRUSTY_VERSION, resolved);
    }
}
