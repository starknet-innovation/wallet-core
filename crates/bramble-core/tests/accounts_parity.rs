//! Every account the extension derives, placed by `starknet_wallet_core::accounts` on
//! Bramble's choices: `accounts-parity-families-{mainnet,sepolia}.json` (see
//! `fixtures/README.md`) hold the public key, class, salt and address the extension's
//! `deriveAccount` gives each family, ladder, index and passphrase over the real Krusty package.
//! The class, salt and layout are Bramble's and go in as arguments; the key and address come out.

use bramble_core::OZ_ACCOUNT_VERSION;
use serde_json::Value;
use starknet_wallet_core::accounts::{
    family_address, AccountFamily, ArgentLayout, ArgentShape, Ladder,
};
use starknet_wallet_core::{account_public_key, felt, oz_account_class_hash, Felt, NetworkId};

fn fixture(network: &str) -> Value {
    let path = format!(
        "{}/tests/fixtures/accounts-parity-families-{network}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn felt_at(value: &Value) -> Felt {
    felt::parse(value.as_str().unwrap()).unwrap()
}

/// The extension's account tables (`account-types.ts`), as `bun run mobile:assets` renders them.
fn catalog() -> Value {
    let path = format!(
        "{}/tests/fixtures/accounts.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// The layout the extension's table (`constructorShape`) gives an Argent class; Xverse's build
/// is the one Krusty does not list.
fn argent_shape(class_hash: &Felt) -> ArgentShape {
    let catalog = catalog();
    let entry = catalog["classes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["family"] == "argent" && felt_at(&entry["classHash"]) == *class_hash)
        .expect("a class in the extension's Argent table");
    let layout = ArgentLayout::from_wire(entry["constructorShape"].as_str().unwrap()).unwrap();
    if felt_at(&catalog["xverseArgentClassHash"]) == *class_hash {
        ArgentShape::Unlisted(layout)
    } else {
        ArgentShape::Listed(layout)
    }
}

/// The ladder a vector was derived on: the extension's kind, with Braavos's `starknet-bip44`
/// on its vendor's address field (`bip44Axes`).
fn ladder(kind: &str, family: AccountFamily) -> Ladder {
    let ladder = match (kind, family) {
        ("argentx-bip44", AccountFamily::Argent) => Ladder::ArgentX,
        ("starknet-bip44", AccountFamily::Braavos) => Ladder::Address,
        ("starknet-bip44", _) => Ladder::Account,
        other => panic!("unexpected derivation {other:?}"),
    };
    assert_eq!(ladder.kind(), kind);
    assert!(ladder.fits(family));
    ladder
}

#[test]
fn split_family_fixtures_agree_on_the_phrase() {
    let (mainnet, sepolia) = (fixture("mainnet"), fixture("sepolia"));
    assert_eq!(mainnet["mnemonic"], sepolia["mnemonic"]);
    assert_eq!(mainnet["passphrase"], sepolia["passphrase"]);
}

#[test]
fn every_family_and_class_derives_the_extensions_address() {
    let mut seen = std::collections::BTreeSet::new();
    for (name, network, id) in [
        ("mainnet", NetworkId::Mainnet, "SN_MAIN"),
        ("sepolia", NetworkId::Sepolia, "SN_SEPOLIA"),
    ] {
        let fixture = fixture(name);
        let mnemonic = fixture["mnemonic"].as_str().unwrap();
        let oz_class =
            felt::parse(&oz_account_class_hash(network, OZ_ACCOUNT_VERSION).unwrap()).unwrap();
        for vector in fixture["vectors"].as_array().unwrap() {
            assert_eq!(vector["networkId"], id);
            let family =
                AccountFamily::from_wire(vector["accountContractType"].as_str().unwrap()).unwrap();
            let ladder = ladder(vector["derivation"].as_str().unwrap(), family);
            let index = u32::try_from(vector["accountIndex"].as_u64().unwrap()).unwrap();
            let passphrase = vector["passphrase"].as_str();
            let context = format!("{name} {family:?} {ladder:?} {index} {passphrase:?}");

            let public_key = account_public_key(mnemonic, passphrase, ladder, index).unwrap();
            assert_eq!(public_key, felt_at(&vector["publicKeyX"]), "key {context}");

            // Bramble's choices: OpenZeppelin at the pinned release with salt zero; Argent and
            // Braavos at the class asked for, salted with the public key.
            let (class_hash, shape, salt) = match family {
                AccountFamily::OpenZeppelin => {
                    assert!(
                        vector["requestedClassHash"].is_null()
                            && vector["deploymentSalt"].is_null()
                    );
                    (oz_class, None, Felt::ZERO)
                }
                AccountFamily::Argent | AccountFamily::Braavos => {
                    let class_hash = felt_at(&vector["requestedClassHash"]);
                    assert_eq!(felt_at(&vector["deploymentSalt"]), public_key, "{context}");
                    let shape =
                        (family == AccountFamily::Argent).then(|| argent_shape(&class_hash));
                    (class_hash, shape, public_key)
                }
            };
            assert_eq!(class_hash, felt_at(&vector["classHash"]), "class {context}");
            let address = family_address(family, &class_hash, shape, &salt, &public_key).unwrap();
            assert_eq!(address, felt_at(&vector["address"]), "address {context}");
            seen.insert((family.wire(), ladder.kind(), passphrase.is_some()));
        }
    }
    // Guard against fixtures that silently stop covering a family, ladder or passphrase.
    assert_eq!(seen.len(), 7, "{seen:?}");
}
