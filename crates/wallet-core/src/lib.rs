//! Starknet wallet core.
//!
//! Key custody through Krusty KMS (`kms`), Nostr coordination identity and sans-IO relay work
//! (`nostr`), the felt grammar they share (`felt`), where an account lives (`accounts`), the
//! encrypted vault (`envelope`, `container`) and the host storage and clock it runs on (`host`),
//! sans-IO JSON-RPC (`rpc`, with Starknet ID names in `stark_name`) and the transfer mechanism
//! (`send`), portfolio reads and display (`portfolio`), activity history (`history`, over the
//! activity log in `activity`) and STRK20 private-balance reads (`privacy`) over caller-given
//! network, token and privacy catalogs (`catalog`, `privacy::config`), and the JavaScript
//! semantics (`js`) display output depends on, as plain Rust. This crate holds mechanism only:
//! product choices (which account version a wallet deploys, which salt, what crosses an FFI
//! boundary) are arguments here and constants in the wallet's own crate (`bramble-core` for
//! Bramble). It has no FFI of its own; a wallet exports what it needs from its crate.
//!
//! Errors carry an operation name or a fixed reason, never input text, so nothing a user typed
//! can reach a log through an error message.
//!
//! ERC-20 allowance discovery and the allowance log (`allowances`) follow the same rules: sans-IO
//! reads over caller-given tokens, the log over the host's storage, the wallet's numbers passed in.
//! So does native staking (`staking`): pinned chain reads, Starkscan's directory and positions,
//! and actions built into a reviewed `send::invoke::Invoke` for the plan to sign. The Ledger
//! device protocol (`ledger`) is sans-IO APDU bytes, the wallet's minimum versions passed in.

#![forbid(unsafe_code)]
// A panic in a wallet is a crash: library code returns errors (unit tests may unwrap: clippy.toml).
#![deny(clippy::unwrap_used, clippy::expect_used)]

pub mod accounts;
pub mod activity;
pub mod allowances;
pub mod catalog;
pub mod container;
pub mod dapp;
pub mod envelope;
pub mod felt;
pub mod gasless;
pub mod history;
pub mod host;
pub mod js;
pub mod kms;
pub mod ledger;
pub mod nostr;
pub mod portfolio;
pub mod privacy;
pub mod rpc;
mod scrub;
pub mod send;
pub mod staking;
pub mod stark_name;
pub mod swap;

pub use kms::{
    account_key, account_keypair, account_public_key, calculate_contract_address,
    compute_channel_key, compute_identity_key, derive_argent_legacy_keypair,
    derive_oz_account_address, derive_starknet_keypair, derive_strk20_viewing_key,
    encode_short_string, generate_mnemonic, oz_account_class_hash, selector, sign_stark_hash,
    stark_public_key, stark_signature_is_valid, validate_mnemonic, NetworkId, StarkKeypair,
    StarkSignature, StarkXOnlyKeypair,
};
pub(crate) use kms::{poseidon_hash_many, tagged_hash};
pub use krusty_kms_common::SecretFelt;
pub use nostr::{
    derive_nostr_public_key, open_nostr_application_data, open_nostr_event,
    wrap_nostr_application_data, NostrApplicationData, NostrEvent,
};
/// The field element the transaction API takes: Krusty's, so a caller never holds a second one.
pub use starknet_types_core::felt::Felt;
/// A non-zero [`Felt`], the divisor `Felt`'s division and remainder take.
pub use starknet_types_core::felt::NonZeroFelt;
pub use zeroize::Zeroizing;

/// The Krusty KMS release compiled into this crate. `krusty_version_matches_the_lockfile` keeps
/// it true.
pub const KRUSTY_VERSION: &str = "3.2.0";

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
