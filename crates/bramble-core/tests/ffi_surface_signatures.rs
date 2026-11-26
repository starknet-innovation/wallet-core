//! Which mobile exports take a signature from the host: each is named, so a new one cannot
//! arrive unreviewed. Reads the exports as `ffi_surface` does.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

use crate::ffi_surface::{exported_signatures, sources};

/// The parameter names of an exported function, in order.
fn parameter_names(signature: &str) -> Vec<String> {
    let arguments = signature.split("->").next().unwrap_or("");
    let inner = arguments.split_once('(').map_or("", |(_, rest)| rest);
    inner
        .split(',')
        .filter_map(|parameter| parameter.split_once(':'))
        .map(|(name, _)| name.trim().trim_start_matches("mut ").to_string())
        .collect()
}

/// A signature coming in from the host (a parameter named `*signature`, or both an `r` and an
/// `s`) is one the core verifies and never signs for; every mobile export that takes one is
/// named here, so a new one cannot arrive unreviewed. The only one is a co-signer's signature,
/// already public, over a stored intent's hash (`import_multisig_signature`).
#[test]
fn signature_takers_are_named() {
    let mut takers = Vec::new();
    for (file, source) in sources() {
        for exported in exported_signatures(&source) {
            if exported.foreign || exported.extension_only {
                continue;
            }
            let names = parameter_names(&exported.signature);
            let has = |wanted: &str| names.iter().any(|name| name == wanted);
            if names.iter().any(|name| name.ends_with("signature")) || (has("r") && has("s")) {
                takers.push(format!("{file}: {}", exported.name()));
            }
        }
    }
    assert_eq!(takers, ["multisig_tx_import.rs: import_multisig_signature"]);
}
