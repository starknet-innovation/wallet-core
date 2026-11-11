//! STRK20 privacy, read-only: private balance discovery through the privacy service as a sans-IO
//! plan (`plan`), and the extension's privacy gates for Home, Receive, token detail and the
//! journey (`gates`, `home`). Nothing is submitted: registration, shielding, private sends and
//! unshielding need an upstream privacy client that Krusty 2.0.0 does not provide (Bramble's
//! `mobile/docs/privacy.md` §1, §9), so `CAPABILITIES` is all false.
//!
//! The viewing key is never held here. The plan asks the wallet for it (`plan::PrivacyHost`)
//! while one discovery body is built, and it leaves the core only inside that zeroizing body,
//! for a non-OHTTP discovery URL (§1.1). Routing, limits and the network catalog are the
//! wallet's, passed in (`config`, `crate::catalog`).

mod bodies;
pub mod config;
pub mod gates;
pub mod home;
mod json;
mod judge;
mod notes;
pub mod plan;
mod plan_steps;
pub mod types;

use gates::Outcome;
use types::{PrivacyBlocker, PrivacyCapabilities, PrivacyEntry, PrivacyReading, PrivacyState};

use self::config::Route;
use crate::send::types::DeploymentState;
use crate::NetworkId;

/// Longest `Post` reply the core parses. Hosts stop reading at this many bytes and report a longer
/// reply as a transport failure.
pub use judge::MAX_RESPONSE_BYTES;

/// What a wallet on this core can submit today: nothing (Bramble's `mobile/docs/privacy.md` §9).
pub const CAPABILITIES: PrivacyCapabilities = PrivacyCapabilities {
    can_register: false,
    can_shield: false,
    can_send_privately: false,
    can_unshield: false,
};

/// Where a privacy entry point leads. `after_read` is false on the tap and true once the host has
/// run the read a `Discovering` answer asked for. `strk_funded` is the loaded public STRK balance
/// being above zero, `None` while it has not loaded: only a loaded zero turns an undeployed
/// account's `Deploy` blocker into `Fund`. The extension's "Send in Shield mode" and "activation"
/// become the `Unsupported` blocker while the capabilities are false.
pub fn privacy_entry(
    routes: &[Route],
    network: NetworkId,
    deployment: DeploymentState,
    strk_funded: Option<bool>,
    reading: PrivacyReading,
    after_read: bool,
) -> PrivacyEntry {
    match gates::entry_outcome(
        routes,
        network,
        deployment,
        strk_funded,
        reading,
        after_read,
    ) {
        Outcome::Blocked(blocker) => PrivacyEntry::Blocked { blocker },
        Outcome::CheckDeployment => PrivacyEntry::CheckDeployment,
        Outcome::Discovering => PrivacyEntry::Discovering,
        // When §9 lands these gain their own variants (open Send in Shield mode, run activation)
        // behind `CAPABILITIES`; until then there is nothing a wallet can do but say so.
        Outcome::OpenShield | Outcome::Activate => PrivacyEntry::Blocked {
            blocker: PrivacyBlocker::Unsupported,
        },
    }
}

/// Whether Home shows "Includes shielded balances".
pub fn shielded_balances_included(
    reading: PrivacyReading,
    state: Option<&PrivacyState>,
    hidden_token_addresses: &[String],
) -> bool {
    home::visible_private(reading, state, hidden_token_addresses)
        .iter()
        .any(|entry| !entry.balance.trim_start_matches('0').is_empty())
}

#[cfg(test)]
mod tests {
    /// The equivalent of Bramble mobile's `privacy_key_boundary.rs` source scan for this module:
    /// no privacy file derives a viewing key, reaches Krusty or touches the phrase or an account
    /// key. The key arrives only through `PrivacyHost::viewing_key`, as a `SecretFelt`, and only
    /// `plan_steps` (which asks for it) and `bodies` (which writes it) name it.
    #[test]
    fn privacy_code_never_derives_or_stores_key_material() {
        let sources = [
            ("bodies.rs", include_str!("bodies.rs")),
            ("config.rs", include_str!("config.rs")),
            ("gates.rs", include_str!("gates.rs")),
            ("home.rs", include_str!("home.rs")),
            ("json.rs", include_str!("json.rs")),
            ("judge.rs", include_str!("judge.rs")),
            ("notes.rs", include_str!("notes.rs")),
            ("plan.rs", include_str!("plan.rs")),
            ("plan_steps.rs", include_str!("plan_steps.rs")),
            ("types.rs", include_str!("types.rs")),
        ];
        for (name, source) in sources {
            for forbidden in [
                "derive_strk20_viewing_key(",
                "krusty_kms",
                "account_key",
                "mnemonic",
                "private_key",
                "crate::kms",
                "WalletCore",
            ] {
                assert!(
                    !source.contains(forbidden),
                    "{name} must not reference {forbidden}"
                );
            }
            let names_key = source.contains("SecretFelt") || source.contains("viewing_key(");
            assert_eq!(
                names_key,
                matches!(name, "bodies.rs" | "plan.rs" | "plan_steps.rs"),
                "{name}: only the plan asks for the key and only the bodies write it"
            );
        }
    }
}
