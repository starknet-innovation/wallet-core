//! Overwriting the stack a secret computation used. RustCrypto and Krusty wipe the values they
//! hold, but not the temporaries a call leaves behind on the stack (HMAC and SHA-512 states, AES
//! round keys, BIP-32 chain codes, ECDSA nonces), and RustCrypto declines to (`RustCrypto/meta#38`).
//! Every call here that takes a key, a phrase or a passphrase runs through `scrubbed`.

use zeroize::Zeroize;

/// How much stack `scrubbed` overwrites. Measured in a release build, the deepest call is a
/// phrase derivation (BIP-39 seed, then BIP-32) at about 67 KiB; Stark signing takes about 3 KiB
/// and the vault's KDF and AES-GCM calls about 2 KiB. Twice the deepest leaves margin for other
/// compilers and targets; a test runs each call on a stack of exactly this size.
pub(crate) const SCRUB_BYTES: usize = 128 * 1024;

/// Runs `f`, then overwrites the stack it used.
///
/// `f` runs one frame down in `run` so that `scrub_stack`, called from the same depth, lays its
/// buffer over everything `f` touched; were `f` inlined here, its locals would sit above it.
pub(crate) fn scrubbed<T>(f: impl FnOnce() -> T) -> T {
    #[inline(never)]
    fn run<T>(f: impl FnOnce() -> T) -> T {
        f()
    }
    #[inline(never)]
    fn scrub_stack() {
        [0u8; SCRUB_BYTES].zeroize();
    }
    let out = run(f);
    scrub_stack();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHRASE: &str =
        "habit hope tip crystal because grunt nation idea electric witness alert like";

    // The scrub covers a call only if the call fits in it. Krusty's deepest calls, unwrapped, on a
    // stack of exactly `SCRUB_BYTES`: one that outgrows it overflows here instead of leaving its
    // deepest frames unscrubbed.
    #[test]
    fn the_deepest_calls_fit_in_the_scrubbed_stack() {
        std::thread::Builder::new()
            .stack_size(SCRUB_BYTES)
            .spawn(|| {
                let pair = krusty_kms::derive_keypair_with_coin_type(
                    PHRASE,
                    0,
                    0,
                    krusty_kms::STARKNET_COIN_TYPE,
                    Some("passphrase"),
                )
                .unwrap();
                krusty_kms::derive_argent_legacy_private_key(PHRASE, 0, 0).unwrap();
                let hash = crate::Felt::from(7u8);
                krusty_kms::sign_stark_hash(pair.private_key.expose_secret(), &hash).unwrap();
                let key = krusty_kms::derive_nostr_private_key(PHRASE, 0, 0, None).unwrap();
                let peer = krusty_kms::derive_nostr_private_key(PHRASE, 1, 0, None).unwrap();
                let peer = krusty_kms::nostr_public_key(&peer).unwrap();
                let peer: String = peer.iter().map(|b| format!("{b:02x}")).collect();
                krusty_kms::wrap_nostr_application_data_at(&key, &peer, "id", "{}", 1).unwrap();
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn returns_what_the_call_returns() {
        assert_eq!(scrubbed(|| 6 * 7), 42);
    }
}
