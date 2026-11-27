use serde_json::{json, Value};

use super::*;
use crate::host::testing::Memory;

const KEY: &str = "nft-preferences";
const CONTRACT: &str = "0x051a2b3c4d";
const ACCOUNT: &str = "0x000123";

fn felt_of(text: &str) -> Felt {
    Felt::from_hex(text).unwrap()
}

fn target(network: NetworkId, token_id: &str, label: Option<&str>) -> NftTarget {
    NftTarget {
        contract: felt_of(CONTRACT),
        network,
        token_id: U256::from_decimal(token_id).unwrap(),
        label: label.map(str::to_string),
    }
}

fn stored(memory: &Memory, value: Value) {
    memory
        .0
        .borrow_mut()
        .insert(KEY.into(), value.to_string().into_bytes());
}

fn document(memory: &Memory) -> Value {
    serde_json::from_slice(&memory.0.borrow()[KEY]).unwrap()
}

// watched-nft.test.ts: "normalizes contract, token id, and derives a deterministic id".
#[test]
fn a_watch_is_stored_in_canonical_spelling_with_a_derived_id() {
    let memory = Memory::default();
    let store = WatchedNfts::new(&memory, KEY);
    let nft = store
        .watch_owned(
            &target(NetworkId::Mainnet, "007", None),
            NetworkId::Mainnet,
            &felt_of(ACCOUNT),
            true,
        )
        .unwrap();
    assert_eq!(nft.contract, "0x51a2b3c4d");
    assert_eq!(nft.token_id, "7");
    assert_eq!(nft.id, "SN_MAIN:0x51a2b3c4d:7");
    assert_eq!(nft.owner.as_deref(), Some("0x123"));
    assert_eq!(store.list().unwrap(), vec![nft]);
}

// watched-nft.test.ts: "keeps optional label and spam flag when provided".
#[test]
fn a_label_is_trimmed_and_spam_is_kept() {
    let memory = Memory::default();
    let store = WatchedNfts::new(&memory, KEY);
    let nft = store
        .watch_owned(
            &target(NetworkId::Sepolia, "42", Some("  Punk  ")),
            NetworkId::Sepolia,
            &felt_of(ACCOUNT),
            true,
        )
        .unwrap();
    assert_eq!(nft.label.as_deref(), Some("Punk"));
    store.set_spam(&nft.id, true).unwrap();
    assert_eq!(store.list().unwrap()[0].spam, Some(true));
    assert_eq!(
        document(&memory)["nfts"][0],
        json!({ "contract": "0x51a2b3c4d", "id": "SN_SEPOLIA:0x51a2b3c4d:42", "label": "Punk",
                "networkId": "SN_SEPOLIA", "owner": "0x123", "spam": true, "tokenId": "42" })
    );
    for bad in ["   ", &"x".repeat(65)] {
        assert!(store
            .watch_owned(
                &target(NetworkId::Sepolia, "1", Some(bad)),
                NetworkId::Sepolia,
                &felt_of(ACCOUNT),
                true
            )
            .is_err());
    }
}

// watched-nft.test.ts: "rejects malformed records"; one bad record fails the whole document,
// as `isWalletPreferences` does.
#[test]
fn a_malformed_record_fails_the_document_and_is_not_overwritten() {
    let good = json!({ "contract": CONTRACT, "id": format!("SN_MAIN:{CONTRACT}:1"),
                       "networkId": "SN_MAIN", "tokenId": "1" });
    let mut bad = vec![
        json!({ "contract": "0x0", "id": "x", "networkId": "SN_MAIN", "tokenId": "1" }),
        json!({ "contract": CONTRACT, "id": "SN_MAIN:wrong:1", "networkId": "SN_MAIN",
                "tokenId": "1" }),
        json!({ "contract": CONTRACT, "id": "x", "networkId": "SN_MAIN", "tokenId": "1.5" }),
        json!({ "contract": CONTRACT, "id": format!("SN_MAIN:{CONTRACT}:1"),
                "networkId": "SN_MAIN", "owner": "not-an-address", "tokenId": "1" }),
    ];
    for (key, value) in [
        ("label", Value::Null),
        ("spam", json!("yes")),
        ("extra", json!(1)),
    ] {
        let mut record = good.clone();
        record[key] = value;
        bad.push(record);
    }
    let memory = Memory::default();
    let store = WatchedNfts::new(&memory, KEY);
    stored(&memory, json!({ "nfts": [good] }));
    assert_eq!(store.list().unwrap().len(), 1);
    for record in bad {
        let doc = json!({ "nfts": [good, record] });
        stored(&memory, doc.clone());
        assert!(store.list().is_err(), "{record}");
        assert!(store.set_spam("x", true).is_err());
        assert_eq!(document(&memory), doc);
    }
    stored(&memory, json!({ "nfts": [good, good] }));
    assert!(store.list().is_err(), "a repeated id");
}

#[test]
fn an_extension_preferences_record_reads_unchanged() {
    let memory = Memory::default();
    let store = WatchedNfts::new(&memory, KEY);
    let nft = json!({ "contract": "0x51a2b3c4d", "id": "SN_MAIN:0x51a2b3c4d:9", "label": " Ape ",
                      "networkId": "SN_MAIN", "spam": false, "tokenId": "9" });
    stored(
        &memory,
        json!({ "schemaVersion": 1, "tokens": [], "collectiblesPreview": false,
                "nftIndexerUrl": "https://indexer.example/v1", "nfts": [nft] }),
    );
    assert!(!store.shown().unwrap());
    assert_eq!(
        store.indexer_url().unwrap().as_deref(),
        Some("https://indexer.example/v1")
    );
    store.remove("absent").unwrap();
    assert_eq!(
        document(&memory),
        json!({ "collectiblesPreview": false, "nftIndexerUrl": "https://indexer.example/v1",
                "nfts": [nft] })
    );
    // An indexer URL the grammar refuses is dropped rather than failing the document.
    stored(&memory, json!({ "nftIndexerUrl": "ftp://x" }));
    assert_eq!(store.indexer_url().unwrap(), None);
    stored(&memory, json!({ "nftIndexerUrl": 1 }));
    assert!(store.indexer_url().is_err());
}

#[test]
fn the_gallery_is_scoped_by_network_owner_and_spam_but_the_review_is_not() {
    let memory = Memory::default();
    let store = WatchedNfts::new(&memory, KEY);
    let mine = felt_of(ACCOUNT);
    let other = felt_of("0x456");
    let watch = |token: &str, network, account: &Felt| {
        store
            .watch_owned(&target(network, token, None), network, account, true)
            .unwrap()
    };
    let a = watch("1", NetworkId::Mainnet, &mine);
    let b = watch("2", NetworkId::Mainnet, &other);
    let spam = watch("3", NetworkId::Mainnet, &mine);
    store.set_spam(&spam.id, true).unwrap();
    watch("4", NetworkId::Sepolia, &mine);
    let ids = |nfts: Vec<WatchedNft>| nfts.into_iter().map(|n| n.id).collect::<Vec<_>>();
    assert_eq!(
        ids(store.gallery(NetworkId::Mainnet, &mine).unwrap()),
        [a.id]
    );
    assert_eq!(
        ids(store.gallery(NetworkId::Mainnet, &other).unwrap()),
        [b.id]
    );
    assert_eq!(store.collections(NetworkId::Mainnet).unwrap().len(), 3);
    assert_eq!(store.collections(NetworkId::Sepolia).unwrap().len(), 1);
    // Unhiding returns it to the gallery; a record without an owner shows for every account.
    store.set_spam(&spam.id, false).unwrap();
    assert_eq!(store.gallery(NetworkId::Mainnet, &mine).unwrap().len(), 2);
    let unowned = json!({ "contract": CONTRACT, "id": format!("SN_MAIN:{CONTRACT}:5"),
                          "networkId": "SN_MAIN", "tokenId": "5" });
    stored(&memory, json!({ "nfts": [unowned] }));
    assert_eq!(store.gallery(NetworkId::Mainnet, &other).unwrap().len(), 1);
}

#[test]
fn a_rewatch_replaces_the_record_and_remove_drops_it() {
    let memory = Memory::default();
    let store = WatchedNfts::new(&memory, KEY);
    let first = store
        .watch_owned(
            &target(NetworkId::Mainnet, "1", Some("Old")),
            NetworkId::Mainnet,
            &felt_of(ACCOUNT),
            true,
        )
        .unwrap();
    store.set_spam(&first.id, true).unwrap();
    let again = store
        .watch_owned(
            &target(NetworkId::Mainnet, "1", None),
            NetworkId::Mainnet,
            &felt_of("0x456"),
            true,
        )
        .unwrap();
    assert_eq!(store.list().unwrap(), vec![again.clone()]);
    assert_eq!(again.owner.as_deref(), Some("0x456"));
    assert!(store.remove("").is_err());
    store.remove(&again.id).unwrap();
    assert!(store.list().unwrap().is_empty());
}

#[test]
fn a_watch_needs_the_active_network_ownership_and_for_a_site_nfts_on() {
    let memory = Memory::default();
    let store = WatchedNfts::new(&memory, KEY);
    let account = felt_of(ACCOUNT);
    let nft = target(NetworkId::Mainnet, "1", None);
    let refusal = |result: Result<WatchedNft, Error>| match result {
        Err(Error::InvalidInput { reason }) => reason,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        refusal(store.watch_owned(&nft, NetworkId::Sepolia, &account, true)),
        "switch to the NFT's network first"
    );
    assert_eq!(
        refusal(store.watch_owned(&nft, NetworkId::Mainnet, &account, false)),
        "this account does not own that NFT"
    );
    store.set_shown(false).unwrap();
    assert_eq!(
        refusal(store.watch_from_site(&nft, NetworkId::Mainnet, &account, true)),
        "nfts are turned off"
    );
    assert!(store.list().unwrap().is_empty());
    store.set_shown(true).unwrap();
    store
        .watch_from_site(&nft, NetworkId::Mainnet, &account, true)
        .unwrap();
    assert_eq!(store.collections(NetworkId::Mainnet).unwrap().len(), 1);
}

#[test]
fn the_indexer_url_is_trimmed_checked_and_cleared() {
    let memory = Memory::default();
    let store = WatchedNfts::new(&memory, KEY);
    assert!(store.shown().unwrap());
    store
        .set_indexer_url("  https://indexer.example/v1 ")
        .unwrap();
    assert_eq!(
        store.indexer_url().unwrap().as_deref(),
        Some("https://indexer.example/v1")
    );
    assert!(store.set_indexer_url("http://indexer.example").is_err());
    store.set_indexer_url("   ").unwrap();
    assert_eq!(store.indexer_url().unwrap(), None);
    assert_eq!(document(&memory), json!({ "nfts": [] }));
}
