//! Bramble's catalogs, embedded from `generated/` (rendered from the extension by Bramble's
//! `bun run mobile:assets`, copied here unchanged) and parsed once on first use: the networks and
//! token registry (`networks`), the account classes, families and recovery bounds (`accounts`,
//! with the class rules in `account_address`), and the multisig and coordination constants
//! (`multisig`). `starknet_wallet_core` takes each as an argument. A bundled file that does not
//! parse or validate panics on first use, loudly, as mobile's loaders do. Plain Rust for now; the
//! UniFFI exports arrive with the swap (docs/PLAN.md, 2.7).

pub mod account_address;
pub mod accounts;
pub mod multisig;
pub mod networks;

pub use networks::catalogs;

/// Every file in `generated/`, by the path Bramble's `bun run mobile:assets` writes it to (relative
/// to Bramble's `mobile/`), with the bytes compiled into this crate. Mobile's build holds its own
/// generator output to these, so a catalog Bramble regenerates cannot be silently ignored by the
/// core it pins.
#[cfg(feature = "mobile")]
pub const GENERATED: [(&str, &str); 8] = [
    (
        "core/generated/accounts.json",
        include_str!("../../generated/accounts.json"),
    ),
    (
        "core/generated/common-passwords.txt",
        include_str!("../../generated/common-passwords.txt"),
    ),
    (
        "core/generated/multisig.json",
        include_str!("../../generated/multisig.json"),
    ),
    (
        "core/generated/networks.json",
        include_str!("../../generated/networks.json"),
    ),
    (
        "core/generated/policy.json",
        include_str!("../../generated/policy.json"),
    ),
    (
        "core/generated/privacy.json",
        include_str!("../../generated/privacy.json"),
    ),
    (
        "core/generated/token-registry.json",
        include_str!("../../generated/token-registry.json"),
    ),
    (
        "ios/Bramble/DesignSystem/BrambleTokens.generated.swift",
        include_str!("../../generated/BrambleTokens.generated.swift"),
    ),
];

#[cfg(all(test, feature = "mobile"))]
mod tests {
    #[test]
    fn every_generated_file_is_listed() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/generated");
        let mut on_disk: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        let mut listed: Vec<String> = super::GENERATED
            .iter()
            .map(|(path, _)| path.rsplit('/').next().unwrap().to_string())
            .collect();
        on_disk.sort();
        listed.sort();
        assert_eq!(listed, on_disk);
    }

    // `policy.json` is copied in with the rest, but the code reads compiled constants; a policy
    // change that reaches only the JSON would otherwise pass unnoticed.
    #[test]
    fn policy_matches_the_compiled_constants() {
        let policy: serde_json::Value =
            serde_json::from_str(include_str!("../../generated/policy.json")).unwrap();
        assert_eq!(policy["idleLockMs"], crate::wallet::IDLE_LOCK_MS);
        assert_eq!(policy["kdfIterations"], crate::vault::KDF_ITERATIONS);
        assert_eq!(
            policy["minPassphraseLength"],
            crate::vault::MIN_PASSWORD_LENGTH
        );
    }
}
