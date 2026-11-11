//! The core must reproduce the extension bit-for-bit. The fixtures are written by the extension's
//! real Krusty path (see `fixtures/README.md`); the known-answer vectors are the ones
//! `scripts/check-krusty-adapter.ts` holds every extension build to.

use bramble_core::*;
use serde_json::Value;

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn text(value: &Value) -> String {
    value.as_str().unwrap().to_string()
}

fn network(id: &str) -> NetworkId {
    match id {
        "SN_MAIN" => NetworkId::Mainnet,
        "SN_SEPOLIA" => NetworkId::Sepolia,
        other => panic!("unexpected network {other}"),
    }
}

#[cfg(feature = "raw-keys")]
#[test]
fn accounts_and_signatures_match_the_extension() {
    let fixture = fixture("extension-parity.json");
    let mnemonic = text(&fixture["mnemonic"]);
    let message_hash = text(&fixture["messageHash"]);
    for vector in fixture["accounts"].as_array().unwrap() {
        let index = u32::try_from(vector["accountIndex"].as_u64().unwrap()).unwrap();
        let net = network(vector["networkId"].as_str().unwrap());
        // The extension's `starknet-bip44` ladder: the hardened account field steps.
        let pair = derive_starknet_keypair(mnemonic.clone(), 0, index, None).unwrap();
        assert_eq!(
            pair.public_key_x,
            text(&vector["publicKeyX"]),
            "{index} {net:?}"
        );

        let class_hash = oz_account_class_hash(net).unwrap();
        assert_eq!(class_hash, text(&vector["classHash"]));
        // OpenZeppelin accounts deploy with salt 0x0 in the extension (`account-contract.ts`).
        let address =
            derive_oz_account_address(pair.public_key_x.clone(), class_hash, Some("0x0".into()))
                .unwrap();
        assert_eq!(address, text(&vector["address"]));

        let signature = sign_stark_hash(pair.private_key, message_hash.clone()).unwrap();
        assert_eq!(signature.public_key, text(&vector["publicKeyX"]));
        assert_eq!(signature.r, text(&vector["signatureR"]));
        assert_eq!(signature.s, text(&vector["signatureS"]));
    }
}

/// The reference vectors from `scripts/check-krusty-adapter.ts`: a published BIP-39 test phrase.
#[cfg(feature = "raw-keys")]
#[test]
fn known_answer_vectors_match_the_reviewed_extension_build() {
    let mnemonic = "legal winner thank year wave sausage worth useful legal winner thank yellow";
    let pair = derive_starknet_keypair(mnemonic.into(), 0, 0, None).unwrap();
    assert_eq!(
        pair.public_key_x,
        "0xb7ff7bf7404503b6df17fa28ea02dd529e58ddd0cfb3232db83ef27630f6aa"
    );
    assert_eq!(
        pair.public_key_y,
        "0x403f8f345048a2a3da2f834a14daab97f61e0c0dbb886a90674355f5b29c5d2"
    );
    let class_hash = oz_account_class_hash(NetworkId::Mainnet).unwrap();
    assert_eq!(
        derive_oz_account_address(pair.public_key_x.clone(), class_hash, Some("0x0".into()))
            .unwrap(),
        "0x340cfbd6cf1d384e7029ccd7cdec84b22b53d2fcffdb005d499e580ec361069"
    );
    let signature = sign_stark_hash(pair.private_key, "0x1".into()).unwrap();
    assert_eq!(signature.public_key, pair.public_key_x);
    assert_eq!(
        signature.r,
        "0x4bbe22478347f1672ccd19d540c47d77ec9dc52d8fb21aeacf8889dfd5282d3"
    );
    assert_eq!(
        signature.s,
        "0x303baae9f9559ff686c55436408d771a51777c371f22e4db1e133a6c0384f4b"
    );
    assert_eq!(
        derive_strk20_viewing_key(
            "0x1".into(),
            "0x534e5f5345504f4c4941".into(),
            "0x123".into()
        )
        .unwrap(),
        "0x27cae3ff78010cbec0f29c0c94420cc4d41ca326d664e510cca4dcc6082beb3"
    );
}

#[test]
fn coordination_keys_match_the_extension() {
    for vector in fixture("nostr-multisig-parity.json")["nostrKeys"]
        .as_array()
        .unwrap()
    {
        let index = u32::try_from(vector["index"].as_u64().unwrap()).unwrap();
        assert_eq!(
            derive_nostr_public_key(text(&vector["mnemonic"]), index, 0, None).unwrap(),
            text(&vector["publicKey"])
        );
    }
}

#[test]
fn opens_the_extension_gift_wrap_and_refuses_its_variants() {
    let fixture = fixture("nostr-multisig-parity.json");
    let wrap = &fixture["giftWrap"];
    let mnemonic = text(&fixture["nostrKeys"][0]["mnemonic"]);
    let recipient = u32::try_from(wrap["recipientNostrIndex"].as_u64().unwrap()).unwrap();
    let opened = open_nostr_application_data(
        mnemonic.clone(),
        recipient,
        0,
        text(&wrap["eventJson"]),
        None,
    )
    .unwrap();
    assert_eq!(opened.content, text(&wrap["content"]));
    assert_eq!(opened.identifier, text(&wrap["identifier"]));
    assert_eq!(
        opened.sender_public_key,
        text(&fixture["nostrKeys"][0]["publicKey"])
    );
    assert!(
        open_nostr_application_data(mnemonic.clone(), 0, 0, text(&wrap["eventJson"]), None)
            .is_err(),
        "wrong recipient"
    );
    for rejection in wrap["rejections"].as_array().unwrap() {
        assert!(
            open_nostr_application_data(
                mnemonic.clone(),
                recipient,
                0,
                text(&rejection["eventJson"]),
                None
            )
            .is_err(),
            "{}",
            rejection["name"]
        );
    }
}

#[test]
fn gift_wraps_round_trip_between_the_two_identities() {
    let fixture = fixture("nostr-multisig-parity.json");
    let mnemonic = text(&fixture["nostrKeys"][0]["mnemonic"]);
    let cosigner = u32::try_from(fixture["nostrKeys"][1]["index"].as_u64().unwrap()).unwrap();
    let recipient = text(&fixture["nostrKeys"][1]["publicKey"]);
    let content = text(&fixture["giftWrap"]["content"]);
    let wrapped = wrap_nostr_application_data(
        mnemonic.clone(),
        0,
        0,
        recipient.clone(),
        "bramble.multisig".into(),
        content.clone(),
        1_750_000_000,
        None,
    )
    .unwrap();
    let opened =
        open_nostr_application_data(mnemonic.clone(), cosigner, 0, wrapped.clone(), None).unwrap();
    assert_eq!(opened.content, content);
    assert_eq!(opened.identifier, "bramble.multisig");
    assert_eq!(
        opened.sender_public_key,
        text(&fixture["nostrKeys"][0]["publicKey"])
    );
    assert!(open_nostr_application_data(mnemonic.clone(), 0, 0, wrapped, None).is_err());
    assert!(wrap_nostr_application_data(
        mnemonic.clone(),
        0,
        0,
        "00".into(),
        "x".into(),
        "y".into(),
        1,
        None
    )
    .is_err());
    assert!(wrap_nostr_application_data(
        mnemonic,
        0,
        0,
        recipient,
        "x".into(),
        String::new(),
        1,
        None
    )
    .is_err());
}

/// Every account family's key ladder, with and without a BIP-39 passphrase, as the extension's
/// `deriveAccount` derives it. The extension maps (family, index) onto Krusty's axes in
/// `bip44Axes`: Braavos steps the address field, everything else the hardened account field;
/// Ready (`argentx-bip44`) has its own ETH-seeded ladder and takes no passphrase.
#[cfg(feature = "raw-keys")]
#[test]
fn every_family_ladder_matches_the_extension() {
    let fixture = fixture("accounts-parity-families-sepolia.json");
    let mnemonic = text(&fixture["mnemonic"]);
    let vectors = fixture["vectors"].as_array().unwrap();
    let mut seen = (0, 0, 0, 0);
    for vector in vectors {
        let index = u32::try_from(vector["accountIndex"].as_u64().unwrap()).unwrap();
        let family = vector["accountContractType"].as_str().unwrap();
        let passphrase = vector["passphrase"].as_str().map(str::to_string);
        let public_key_x = if vector["derivation"] == "argentx-bip44" {
            assert!(passphrase.is_none());
            seen.0 += 1;
            derive_argent_legacy_keypair(mnemonic.clone(), index)
                .unwrap()
                .public_key_x
        } else {
            let (address, account) = if family == "braavos" {
                seen.1 += 1;
                (index, 0)
            } else {
                (0, index)
            };
            if passphrase.is_some() {
                seen.2 += 1;
            }
            derive_starknet_keypair(mnemonic.clone(), address, account, passphrase)
                .unwrap()
                .public_key_x
        };
        assert_eq!(
            public_key_x,
            text(&vector["publicKeyX"]),
            "{family} {} {index}",
            vector["derivation"]
        );
        if family == "openzeppelin" {
            seen.3 += 1;
            let address = derive_oz_account_address(
                public_key_x,
                text(&vector["classHash"]),
                Some("0x0".into()),
            )
            .unwrap();
            assert_eq!(address, text(&vector["address"]), "oz {index}");
        }
    }
    // Guard against a fixture that silently stops covering a ladder.
    assert!(
        seen.0 > 0 && seen.1 > 0 && seen.2 > 0 && seen.3 > 0,
        "{seen:?}"
    );
}

/// Mobile's relay code parses events before opening them; the parsed path must open what the
/// JSON path opens and refuse what it refuses.
#[test]
fn opens_the_extension_gift_wrap_from_a_parsed_event() {
    use starknet_wallet_core::nostr::parse_event_json;
    let fixture = fixture("nostr-multisig-parity.json");
    let wrap = &fixture["giftWrap"];
    let mnemonic = text(&fixture["nostrKeys"][0]["mnemonic"]);
    let recipient = u32::try_from(wrap["recipientNostrIndex"].as_u64().unwrap()).unwrap();
    let event = parse_event_json(wrap["eventJson"].as_str().unwrap()).unwrap();
    let opened =
        starknet_wallet_core::open_nostr_event(&mnemonic, recipient, 0, &event, None).unwrap();
    assert_eq!(opened.content, text(&wrap["content"]));
    assert_eq!(opened.identifier, text(&wrap["identifier"]));
    assert_eq!(
        opened.sender_public_key,
        text(&fixture["nostrKeys"][0]["publicKey"])
    );
    assert!(
        starknet_wallet_core::open_nostr_event(&mnemonic, 0, 0, &event, None).is_err(),
        "wrong recipient"
    );
    let mut parsed = 0;
    for rejection in wrap["rejections"].as_array().unwrap() {
        // Some variants (oversize content, an extra key) never parse; the rest must not open.
        if let Some(event) = parse_event_json(rejection["eventJson"].as_str().unwrap()) {
            parsed += 1;
            assert!(
                starknet_wallet_core::open_nostr_event(&mnemonic, recipient, 0, &event, None)
                    .is_err(),
                "{}",
                rejection["name"]
            );
        }
    }
    // Guard against a fixture whose variants all stop parsing, which would test nothing here.
    assert!(parsed > 0);
    // `created_at` spelled as a float opens, as through the extension's `JSON.stringify(event)`:
    // Krusty sees the re-serialised integer, never the relay's spelling (which it refuses).
    let raw = wrap["eventJson"].as_str().unwrap();
    let integer = format!("\"created_at\":{}", event.created_at);
    assert!(raw.contains(&integer));
    let float = raw.replacen(&integer, &format!("{integer}.0"), 1);
    let opened =
        starknet_wallet_core::open_nostr_application_data(&mnemonic, recipient, 0, &float, None)
            .unwrap();
    assert_eq!(opened.content, text(&wrap["content"]));
}

/// The public key of a derived key is the extension's `publicKeyX`, with no raw key crossing.
#[test]
fn public_keys_of_derived_keys_match_the_extension() {
    let fixture = fixture("extension-parity.json");
    let mnemonic = text(&fixture["mnemonic"]);
    for vector in fixture["accounts"].as_array().unwrap() {
        let index = u32::try_from(vector["accountIndex"].as_u64().unwrap()).unwrap();
        let pair =
            starknet_wallet_core::derive_starknet_keypair(&mnemonic, 0, index, None).unwrap();
        assert_eq!(
            starknet_wallet_core::stark_public_key(&pair.private_key).unwrap(),
            text(&vector["publicKeyX"]),
            "{index}"
        );
    }
    assert!(
        starknet_wallet_core::stark_public_key(&starknet_wallet_core::SecretFelt::new(
            Default::default()
        ))
        .is_err()
    );
}

/// The public viewing key the extension would register for an account in the privacy pool
/// (`previewStrk20ViewingPublicKey`), composed two ways: from the account key through the scoped
/// viewing key, and by the recovery scan's `own_viewing_public_key`.
#[test]
fn viewing_public_keys_match_the_extension() {
    use starknet_wallet_core::accounts::{advisory::own_viewing_public_key, Ladder};
    use starknet_wallet_core::{account_key, felt};
    let fixture = fixture("extension-privacy-wire.json");
    let mnemonic = text(&fixture["mnemonic"]);
    let keys = &fixture["viewingKeys"];
    let (chain_id, pool) = (text(&keys["chainId"]), text(&keys["pool"]));
    assert_eq!(chain_id, felt::canonical(&NetworkId::Mainnet.chain_id()));
    let accounts = keys["accounts"].as_array().unwrap();
    assert_eq!(accounts.len(), 2);
    for vector in accounts {
        let index = u32::try_from(vector["accountIndex"].as_u64().unwrap()).unwrap();
        let expected = text(&vector["viewingPublicKey"]);
        let key = account_key(&mnemonic, None, Ladder::Account, index).unwrap();
        let viewing_key =
            starknet_wallet_core::derive_strk20_viewing_key(&key, &chain_id, &pool).unwrap();
        assert_eq!(
            starknet_wallet_core::stark_public_key(&viewing_key).unwrap(),
            expected,
            "{index}"
        );
        let own = own_viewing_public_key(
            &mnemonic,
            None,
            Ladder::Account,
            index,
            NetworkId::Mainnet,
            &felt::parse(&pool).unwrap(),
        )
        .unwrap();
        assert_eq!(felt::canonical(&own), expected, "{index}");
    }
}
