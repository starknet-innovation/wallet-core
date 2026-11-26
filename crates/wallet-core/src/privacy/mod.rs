//! STRK20 privacy, read-only: private balance discovery through the privacy service as a sans-IO
//! plan (`plan`), and the extension's privacy gates for Home, Receive, token detail and the
//! journey (`gates`, `home`). Nothing is submitted: registration, shielding, private sends and
//! unshielding need an upstream privacy client that Krusty 3.2.0 does not provide (Bramble's
//! `mobile/docs/privacy.md` §1, §9), so `CAPABILITIES` is all false.
//!
//! The viewing key is never held here. The plan asks the wallet for it (`plan::PrivacyHost`)
//! while one discovery body is built, and it leaves the core only inside that zeroizing body,
//! for a non-OHTTP discovery URL (§1.1). Routing, limits and the network catalog are the
//! wallet's, passed in (`config`, `crate::catalog`).
//!
//! Proofs (P15) go through an engine the wallet supplies (`engine::PrivacyEngine`): the core signs
//! the `CallSet` the SDK asks for (`call_set`) once its calls are bound to the reviewed actions
//! (`pool_actions`), and judges what comes back (`engine_judge`). No flow uses it yet, so
//! `CAPABILITIES` stays false.
//!
//! The port of the SDK's client (P15, issue #182) adds the proof job's discovery walks as sans-IO
//! pieces: the outgoing channels (`outgoing`) and the spendable notes with their witnesses
//! (`incoming`), over the types in `discovery`.

mod bodies;
pub mod call_set;
pub mod config;
pub mod discovery;
pub mod engine;
mod engine_judge;
pub mod gates;
pub mod home;
pub mod incoming;
mod json;
mod judge;
mod notes;
pub mod outgoing;
pub mod plan;
mod plan_steps;
mod pool_actions;
pub mod types;

use gates::Outcome;
use types::{PrivacyBlocker, PrivacyCapabilities, PrivacyEntry, PrivacyReading, PrivacyState};

use self::config::Route;
use crate::send::types::DeploymentState;
use crate::NetworkId;

/// Longest `Post` reply the core parses. Hosts stop reading at this many bytes and report a longer
/// reply as a transport failure.
pub use judge::MAX_RESPONSE_BYTES;

/// A discovery block, as the walks take the proving block and echo the service's.
pub use bodies::BlockRef;

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
    /// `plan_steps` (which asks for it) and `bodies` (which writes it) name it, besides `engine`,
    /// whose job hands it to the engine, `pool_actions`, which compares the key slot with it, and
    /// the discovery walks (`discovery`, `incoming`, `outgoing`), which lend it to `bodies` and
    /// hold the channel keys and witnesses the service derives from it as `SecretFelt`s.
    /// The `CallSet` signature is the wallet's (`engine::prove` takes its signing closure), so the
    /// engine files reach no key-taking Krusty export.
    #[test]
    fn privacy_code_never_derives_or_stores_key_material() {
        let sources = [
            ("bodies.rs", include_str!("bodies.rs")),
            ("call_set.rs", include_str!("call_set.rs")),
            ("config.rs", include_str!("config.rs")),
            ("discovery.rs", include_str!("discovery.rs")),
            ("engine.rs", include_str!("engine.rs")),
            ("engine_judge.rs", include_str!("engine_judge.rs")),
            ("gates.rs", include_str!("gates.rs")),
            ("home.rs", include_str!("home.rs")),
            ("incoming.rs", include_str!("incoming.rs")),
            ("json.rs", include_str!("json.rs")),
            ("judge.rs", include_str!("judge.rs")),
            ("notes.rs", include_str!("notes.rs")),
            ("outgoing.rs", include_str!("outgoing.rs")),
            ("plan.rs", include_str!("plan.rs")),
            ("plan_steps.rs", include_str!("plan_steps.rs")),
            ("pool_actions.rs", include_str!("pool_actions.rs")),
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
                matches!(
                    name,
                    "bodies.rs"
                        | "discovery.rs"
                        | "engine.rs"
                        | "incoming.rs"
                        | "outgoing.rs"
                        | "plan.rs"
                        | "plan_steps.rs"
                        | "pool_actions.rs"
                ),
                "{name}: only the plan asks for the key, only the bodies write it, only the \
                 engine's job carries it to the engine, only the pool decoder compares it, and \
                 only the discovery walks lend it and hold what is derived from it"
            );
        }
        // The `CallSet` is signed by the wallet's signing path, through the closure `engine::prove`
        // is given; outside tests, no privacy file reaches a key-taking Krusty export.
        // Nor do the discovery walks, which never read a secret back out either.
        for (name, source) in [
            ("call_set.rs", include_str!("call_set.rs")),
            ("discovery.rs", include_str!("discovery.rs")),
            ("incoming.rs", include_str!("incoming.rs")),
            ("outgoing.rs", include_str!("outgoing.rs")),
            ("engine.rs", include_str!("engine.rs")),
            ("engine_judge.rs", include_str!("engine_judge.rs")),
            ("pool_actions.rs", include_str!("pool_actions.rs")),
        ] {
            let body = source.split("#[cfg(test)]").next().unwrap_or_default();
            for forbidden in [
                "sign_stark_hash",
                "account_keypair",
                "derive_",
                "stark_public_key",
                "expose_secret",
            ] {
                assert!(
                    !body.contains(forbidden),
                    "{name} must not reference {forbidden}"
                );
            }
        }
    }
}
