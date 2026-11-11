//! Starknet wallet core.
//!
//! Key custody through Krusty KMS (`kms`), Nostr coordination identity (`nostr`), the felt
//! grammar they share (`felt`), where an account lives (`accounts`), the encrypted vault (`envelope`, `container`) and the host
//! storage and clock it runs on (`host`), sans-IO JSON-RPC (`rpc`) and the transfer mechanism
//! (`send`), portfolio reads and display (`portfolio`) and activity history (`history`, over the
//! activity log in `activity`) over caller-given network and token catalogs (`catalog`), and the
//! JavaScript semantics (`js`) display output depends on, as plain Rust. This crate holds mechanism only: product choices
//! (which account version a wallet deploys, which salt, what crosses an FFI boundary) are
//! arguments here and constants in the wallet's own crate (`bramble-core` for Bramble). It has
//! no FFI of its own; a wallet exports what it needs from its crate.
//!
//! Errors carry an operation name or a fixed reason, never input text, so nothing a user typed
//! can reach a log through an error message.

#![forbid(unsafe_code)]

pub mod accounts;
pub mod activity;
pub mod catalog;
pub mod container;
pub mod envelope;
pub mod felt;
pub mod history;
pub mod host;
pub mod js;
pub mod kms;
pub mod nostr;
pub mod portfolio;
pub mod rpc;
pub mod send;

pub use kms::{
    account_key, account_public_key, derive_argent_legacy_keypair, derive_oz_account_address,
    derive_starknet_keypair, derive_strk20_viewing_key, generate_mnemonic, oz_account_class_hash,
    sign_stark_hash, stark_public_key, validate_mnemonic, NetworkId, StarkKeypair, StarkSignature,
    StarkXOnlyKeypair,
};
pub use krusty_kms_common::SecretFelt;
pub use nostr::{
    derive_nostr_public_key, open_nostr_application_data, open_nostr_event,
    wrap_nostr_application_data, NostrApplicationData, NostrEvent,
};
/// The field element the transaction API takes: Krusty's, so a caller never holds a second one.
pub use starknet_types_core::felt::Felt;
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
    /// The passphrase does not open the vault (or the envelope was tampered with; by design the
    /// two are indistinguishable).
    #[error("wrong password")]
    WrongPassword,
    #[error("storage failure: {reason}")]
    Storage { reason: String },
    /// The vault has no biometric slot.
    #[error("biometric unlock is not enabled")]
    BiometricUnavailable,
    /// The host's wrap secret does not open the biometric slot (stale or tampered).
    #[error("biometric unlock was invalidated")]
    BiometricInvalidated,
    /// A node reply the core will not interpret (a fixed reason, never node text).
    #[error("rpc failure: {reason}")]
    Rpc { reason: String },
}

impl Error {
    pub(crate) fn invalid(reason: &str) -> Self {
        Self::InvalidInput {
            reason: reason.into(),
        }
    }

    /// A storage failure with a fixed reason. Public so a host's `Storage` reports the same way.
    pub fn storage(reason: &str) -> Self {
        Self::Storage {
            reason: reason.into(),
        }
    }

    pub(crate) fn rpc(reason: &str) -> Self {
        Self::Rpc {
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
        // The workspace lockfile in this repo; the crate's own copy in a published package.
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let lock = ["../../Cargo.lock", "Cargo.lock"]
            .iter()
            .find_map(|path| std::fs::read_to_string(manifest.join(path)).ok())
            .expect("a Cargo.lock beside the crate or at the workspace root");
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
