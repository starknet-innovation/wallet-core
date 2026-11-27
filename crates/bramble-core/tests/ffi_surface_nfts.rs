//! P17's NFT exports, held to the FFI surface's no-key rules. Reads the exports as `ffi_surface`
//! does.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

use crate::ffi_surface::{exported_signatures, sources};

/// P17's NFT exports read public chain and HTTP data and send one NFT only through the reviewed
/// plan: none takes a key, a signature, a hash or a call (the send takes a token and a recipient,
/// and the core builds the call), and none hands one out. A watch is stored from the core's own
/// ownership read, never a host's `owned` flag. The metadata's HTML animation never crosses:
/// mobile has no sandbox to run it in. Their files are named so a new export is scanned here.
#[test]
fn nfts_take_no_key_hash_or_call_and_carry_no_animation() {
    let mut names = Vec::new();
    for (file, source) in sources() {
        if !file.starts_with("ffi_nft") {
            continue;
        }
        assert!(
            !source.contains("NftAnimation") && !source.contains("pub animation"),
            "{file}: an animation crosses the FFI"
        );
        for exported in exported_signatures(&source) {
            let signature = exported.signature.to_lowercase();
            let incoming = signature.split("->").next().unwrap_or("");
            let arguments = incoming.split_once('(').map_or("", |(_, rest)| rest);
            for word in [
                "private",
                "secret",
                "mnemonic",
                "phrase",
                "signature",
                "hash",
                "typed",
                "owned",
            ] {
                assert!(
                    !signature.contains(word),
                    "{file}: `{}` carries `{word}`",
                    exported.signature
                );
            }
            assert!(
                !arguments.contains("call") && !arguments.contains("felt"),
                "{file}: `{}` takes a call",
                exported.signature
            );
            names.push(exported.name().to_string());
        }
    }
    names.sort();
    assert_eq!(
        names.join(" "),
        "accept accept accept finish finish judge new next_calls next_calls next_calls \
         nft_discovery nft_indexer_url nft_metadata_judge nft_ownership_read nfts_shown plan \
         prepare_nft_transfer remove_nft restart restart restart set_nft_indexer_url \
         set_nft_spam set_nfts_shown watch_nft watched_nft_candidates watched_nfts"
    );
}
