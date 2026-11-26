//! STRK20 privacy, read-only: private balance discovery through the privacy service as a sans-IO
//! plan (`plan`), and the extension's privacy gates for Home, Receive, token detail and the
//! journey (`gates`, `home`). Nothing is submitted: registration, shielding, private sends and
//! unshielding need an upstream privacy client that Krusty 3.2.0 does not provide (Bramble's
//! `mobile/docs/privacy.md` §1, §9), so `CAPABILITIES` is all false.
//!
//! The viewing key is the wallet's (`plan::PrivacyHost`), asked for whenever a body needs it and
//! dropped after. The balance read never holds it past one discovery body. A proof job
//! (`job::ProofJob`) holds it, between compile and the proof, inside the bound `compile_actions`
//! calldata and the proof invocation, as `SecretFelt`s. It leaves the core only inside zeroizing
//! request bodies, to the discovery URL and, in the invocation, to the prover URL, never through
//! OHTTP (§1.1). Routing, limits and the network catalog are the wallet's, passed in (`config`,
//! `crate::catalog`).
//!
//! What is wiped and what is not, in one place. Zeroized on drop: the viewing key, channel keys
//! and witnesses as `SecretFelt`s, the calldata and invocation, every request body, the cursor's
//! echo while a body is built (`json::sized`), and a proof job's `Http` reply bodies once judged.
//! Plain memory until dropped, an accepted limitation: the parsed reply (`json::Ordered`) and the
//! cursor a walk holds between pages, which carry the service's channel keys, salts and amounts as
//! text, the balance read's reply bodies, and what a growing service-sized map leaves behind when
//! it reallocates (`discovery::AddressMap`). `SecretFelt` wipes where it is dropped, not the
//! bitwise copies a move leaves on the stack.
//!
//! Proofs (P15) are the Rust port of the SDK's client (#182). Its records (the reviewed actions,
//! the wallet's bounds, the judged call and proof, the refusals) are `proof`. It starts from the
//! SDK's primitives: the domain-tagged hashes (`hashes`, with the viewing-key ones in `kms.rs`),
//! the `ClientAction` encoding (`client_actions`, sharing `pool_actions`' variant table) and its
//! randomness (`entropy`).
//!
//! Its discovery walks are sans-IO pieces: the outgoing channels (`outgoing`) and the spendable
//! notes with their witnesses (`incoming`), over the types in `discovery`. Between and after them,
//! the compiler (`compiler`, over the pool simulator in `simulator`) says what the walks must
//! fetch and turns what they found into the job's client actions.
//!
//! The compiled `compile_actions` is bound to the reviewed actions (`pool_actions`) before the
//! `CallSet` over it is signed (`call_set`); the proof invocation is built here (`invocation`),
//! and the prover is plain bodies and judged replies (`prover`). No flow proves yet, so
//! `CAPABILITIES` stays false.
//!
//! Shadow accounts (S6): the commitments, addresses and the anonymizer's invoke data (`shadow`),
//! which the compiler's invoke phase uses, and the extension's two shadow-account reads as
//! sans-IO pieces (`shadow_reads`).
//!
//! Multisig (S7): an Argent multisig's `CallSet` aggregate is checked signer by signer
//! (`call_set::authorized_aggregate`). A job short of its threshold ends holding the request
//! (`held::HeldProof`, which holds the key in its calldata as `SecretFelt`s and leaves the core
//! only redacted and sealed), and a later job resumes it (`job_resume`) after reading the pool's
//! proof window.

mod bodies;
pub mod call_set;
pub mod client_actions;
pub mod compiler;
pub mod config;
pub mod discovery;
pub mod entropy;
pub mod gates;
pub mod hashes;
pub mod held;
pub mod home;
pub mod incoming;
pub mod invocation;
pub mod job;
mod job_resume;
mod job_steps;
mod json;
mod judge;
mod notes;
pub mod outgoing;
pub mod plan;
mod plan_steps;
mod pool_actions;
pub mod proof;
pub mod prover;
pub mod shadow;
pub mod shadow_reads;
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
    /// `plan_steps` and the proof job (`job`, `job_steps`, `job_resume`, which ask for it and hold
    /// the bound calldata), `bodies` and `invocation` (which write it) name it, besides `held`,
    /// which takes it out of a held calldata's key slot, puts it back and seals the record under
    /// it (inside `scrubbed`, so it is not in the second list),
    /// `pool_actions`, which compares the key slot with it, `call_set`, which hashes the calldata
    /// that holds it, `client_actions`, which writes it into the `compile_actions` calldata, and the
    /// discovery walks (`discovery`, `incoming`, `outgoing`), which lend it to `bodies` and hold the
    /// channel keys and witnesses the service derives from it as `SecretFelt`s. Channel keys are
    /// secrets too: `client_actions` carries them and `hashes` hashes one into a note id. The
    /// hashes that consume the viewing key itself are `kms.rs`'s, reached through the crate root.
    /// Like `bodies`, `invocation` exposes the secret to serialise it, and `call_set` and
    /// `pool_actions` to hash and decode it (inside `scrubbed`), so they are not in the second
    /// list. The compiler and its simulator hold the viewing key to derive channel keys through
    /// the crate root (`compute_channel_key`), and the simulator its public key
    /// (`stark_public_key`). `shadow` holds the key to derive the identity key through the crate
    /// root (`compute_identity_key`) and hashes it in place into the partial commitment, and
    /// `pool_actions` derives a channel key from it to recompute an open note's id;
    /// `shadow_reads` only lends it to the commitment read's discovery body. The `CallSet`
    /// signature is the wallet's (`call_set::sign_call_set` takes its signing closure), so no
    /// privacy file reaches a key-taking Krusty export.
    /// Every serialisation of a secret runs inside `scrub::scrubbed`, as the hashes do: the
    /// invocation's JSON (calldata hex) and `generateRandom`'s reduction.
    #[test]
    fn secret_serialisation_runs_scrubbed() {
        for (source, function) in [
            (include_str!("invocation.rs"), "fn json_within("),
            (include_str!("entropy.rs"), "fn generate_random("),
            (include_str!("held.rs"), "fn seal("),
            (include_str!("held.rs"), "pub fn redact("),
            (include_str!("held.rs"), "pub fn restore_calldata("),
        ] {
            let body = source.split(function).nth(1).unwrap_or_default();
            let body = body.split("\n}\n").next().unwrap_or_default();
            let body = body.split("\n    }\n").next().unwrap_or_default();
            assert!(body.contains("scrubbed("), "{function} runs scrubbed");
        }
    }

    #[test]
    fn privacy_code_never_derives_or_stores_key_material() {
        let sources = [
            ("bodies.rs", include_str!("bodies.rs")),
            ("call_set.rs", include_str!("call_set.rs")),
            ("client_actions.rs", include_str!("client_actions.rs")),
            ("compiler.rs", include_str!("compiler.rs")),
            ("config.rs", include_str!("config.rs")),
            ("discovery.rs", include_str!("discovery.rs")),
            ("entropy.rs", include_str!("entropy.rs")),
            ("gates.rs", include_str!("gates.rs")),
            ("hashes.rs", include_str!("hashes.rs")),
            ("held.rs", include_str!("held.rs")),
            ("home.rs", include_str!("home.rs")),
            ("incoming.rs", include_str!("incoming.rs")),
            ("invocation.rs", include_str!("invocation.rs")),
            ("job.rs", include_str!("job.rs")),
            ("job_resume.rs", include_str!("job_resume.rs")),
            ("job_steps.rs", include_str!("job_steps.rs")),
            ("json.rs", include_str!("json.rs")),
            ("judge.rs", include_str!("judge.rs")),
            ("notes.rs", include_str!("notes.rs")),
            ("outgoing.rs", include_str!("outgoing.rs")),
            ("plan.rs", include_str!("plan.rs")),
            ("plan_steps.rs", include_str!("plan_steps.rs")),
            ("pool_actions.rs", include_str!("pool_actions.rs")),
            ("proof.rs", include_str!("proof.rs")),
            ("prover.rs", include_str!("prover.rs")),
            ("shadow.rs", include_str!("shadow.rs")),
            ("shadow_reads.rs", include_str!("shadow_reads.rs")),
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
                        | "call_set.rs"
                        | "client_actions.rs"
                        | "compiler.rs"
                        | "discovery.rs"
                        | "hashes.rs"
                        | "held.rs"
                        | "incoming.rs"
                        | "invocation.rs"
                        | "job.rs"
                        | "job_resume.rs"
                        | "job_steps.rs"
                        | "outgoing.rs"
                        | "plan.rs"
                        | "plan_steps.rs"
                        | "pool_actions.rs"
                        | "shadow.rs"
                        | "shadow_reads.rs"
                        | "simulator.rs"
                ),
                "{name}: only the plan and the proof job ask for the key, only the bodies, the client actions and \
                 the invocation write it, only the pool decoder compares it, only the CallSet \
                 hashes the calldata it is in, only the note id hashes a channel key, only the \
                 discovery walks and the shadow reads lend it and hold what is derived from it, \
                 only the compiler and its simulator derive the channel keys from it, only \
                 the shadow commitments derive the identity key from it, and only the held \
                 proof redacts, restores and seals under it"
            );
        }
        // The `CallSet` is signed by the wallet's signing path, through the closure
        // `call_set::sign_call_set` is given; outside tests, no privacy file reaches a key-taking
        // Krusty export.
        // Nor do the discovery walks or the compiler, which never read a secret back out either.
        for (name, source) in [
            ("client_actions.rs", include_str!("client_actions.rs")),
            ("compiler.rs", include_str!("compiler.rs")),
            ("discovery.rs", include_str!("discovery.rs")),
            ("incoming.rs", include_str!("incoming.rs")),
            ("job.rs", include_str!("job.rs")),
            ("job_resume.rs", include_str!("job_resume.rs")),
            ("job_steps.rs", include_str!("job_steps.rs")),
            ("outgoing.rs", include_str!("outgoing.rs")),
            ("entropy.rs", include_str!("entropy.rs")),
            ("proof.rs", include_str!("proof.rs")),
            ("prover.rs", include_str!("prover.rs")),
            ("shadow_reads.rs", include_str!("shadow_reads.rs")),
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
