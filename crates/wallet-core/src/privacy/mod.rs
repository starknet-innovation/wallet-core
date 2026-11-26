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
//! The Rust port of the SDK's client (#182) starts from its primitives: the domain-tagged hashes
//! (`hashes`, with the viewing-key ones in `kms.rs`), the `ClientAction` encoding
//! (`client_actions`, sharing `pool_actions`' variant table) and its randomness (`entropy`).
//!
//! Its proof job's discovery walks are sans-IO pieces too: the outgoing channels (`outgoing`) and
//! the spendable notes with their witnesses (`incoming`), over the types in `discovery`. Between
//! and after them, the compiler (`compiler`, over the pool simulator in `simulator`) says what
//! the walks must fetch and turns what they found into the job's client actions.
//!
//! It builds the proof invocation itself (`invocation`) and talks to the prover as plain bodies
//! and judged replies (`prover`); the job that drives them replaces the engine.

mod bodies;
pub mod call_set;
pub mod client_actions;
pub mod compiler;
pub mod config;
pub mod discovery;
pub mod engine;
mod engine_judge;
pub mod entropy;
pub mod gates;
pub mod hashes;
pub mod home;
pub mod incoming;
pub mod invocation;
mod json;
mod judge;
mod notes;
pub mod outgoing;
pub mod plan;
mod plan_steps;
mod pool_actions;
pub mod prover;
mod simulator;
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
    /// `plan_steps` (which asks for it), `bodies` and `invocation` (which write it) name it, besides
    /// `engine`, whose job hands it to the engine, `pool_actions`, which compares the key slot with
    /// it, `client_actions`, which writes it into the `compile_actions` calldata, and the discovery
    /// walks (`discovery`, `incoming`, `outgoing`), which lend it to `bodies` and hold the channel
    /// keys and witnesses the service derives from it as `SecretFelt`s. Channel keys are secrets
    /// too: `client_actions` carries them and `hashes` hashes one into a note id. The hashes that
    /// consume the viewing key itself are `kms.rs`'s, reached through the crate root. Like
    /// `bodies`, `invocation` exposes the secret to serialise it, so it is not in the second list.
    /// The compiler and its simulator hold the viewing key to derive channel keys through the crate
    /// root (`compute_channel_key`), and the simulator its public key (`stark_public_key`).
    /// The `CallSet` signature is the wallet's (`engine::prove` takes its signing closure), so the
    /// engine files reach no key-taking Krusty export.
    #[test]
    fn privacy_code_never_derives_or_stores_key_material() {
        let sources = [
            ("bodies.rs", include_str!("bodies.rs")),
            ("call_set.rs", include_str!("call_set.rs")),
            ("client_actions.rs", include_str!("client_actions.rs")),
            ("compiler.rs", include_str!("compiler.rs")),
            ("config.rs", include_str!("config.rs")),
            ("discovery.rs", include_str!("discovery.rs")),
            ("engine.rs", include_str!("engine.rs")),
            ("engine_judge.rs", include_str!("engine_judge.rs")),
            ("entropy.rs", include_str!("entropy.rs")),
            ("gates.rs", include_str!("gates.rs")),
            ("hashes.rs", include_str!("hashes.rs")),
            ("home.rs", include_str!("home.rs")),
            ("incoming.rs", include_str!("incoming.rs")),
            ("invocation.rs", include_str!("invocation.rs")),
            ("json.rs", include_str!("json.rs")),
            ("judge.rs", include_str!("judge.rs")),
            ("notes.rs", include_str!("notes.rs")),
            ("outgoing.rs", include_str!("outgoing.rs")),
            ("plan.rs", include_str!("plan.rs")),
            ("plan_steps.rs", include_str!("plan_steps.rs")),
            ("pool_actions.rs", include_str!("pool_actions.rs")),
            ("prover.rs", include_str!("prover.rs")),
            ("simulator.rs", include_str!("simulator.rs")),
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
                        | "client_actions.rs"
                        | "compiler.rs"
                        | "discovery.rs"
                        | "engine.rs"
                        | "hashes.rs"
                        | "incoming.rs"
                        | "invocation.rs"
                        | "outgoing.rs"
                        | "plan.rs"
                        | "plan_steps.rs"
                        | "pool_actions.rs"
                        | "simulator.rs"
                ),
                "{name}: only the plan asks for the key, only the bodies, the client actions and \
                 the invocation write it, only the engine's job carries it to the engine, only the \
                 pool decoder compares it, only the note id hashes a channel key, only the \
                 discovery walks lend it and hold what is derived from it, and only the compiler \
                 and its simulator derive the channel keys from it"
            );
        }
        // The `CallSet` is signed by the wallet's signing path, through the closure `engine::prove`
        // is given; outside tests, no privacy file reaches a key-taking Krusty export.
        // Nor do the discovery walks or the compiler, which never read a secret back out either.
        for (name, source) in [
            ("call_set.rs", include_str!("call_set.rs")),
            ("client_actions.rs", include_str!("client_actions.rs")),
            ("compiler.rs", include_str!("compiler.rs")),
            ("discovery.rs", include_str!("discovery.rs")),
            ("incoming.rs", include_str!("incoming.rs")),
            ("outgoing.rs", include_str!("outgoing.rs")),
            ("engine.rs", include_str!("engine.rs")),
            ("engine_judge.rs", include_str!("engine_judge.rs")),
            ("entropy.rs", include_str!("entropy.rs")),
            ("pool_actions.rs", include_str!("pool_actions.rs")),
            ("prover.rs", include_str!("prover.rs")),
            ("simulator.rs", include_str!("simulator.rs")),
        ] {
            let body = source.split("#[cfg(test)]").next().unwrap_or_default();
            for forbidden in [
                "sign_stark_hash",
                "account_keypair",
                "derive_",
                "stark_public_key",
                "expose_secret",
            ] {
                // The one exception: registering, the simulator puts the account's viewing public
                // key on its own channel (`derivePublicKey`), as the pool will.
                if (name, forbidden) == ("simulator.rs", "stark_public_key") {
                    continue;
                }
                assert!(
                    !body.contains(forbidden),
                    "{name} must not reference {forbidden}"
                );
            }
        }
    }
}
