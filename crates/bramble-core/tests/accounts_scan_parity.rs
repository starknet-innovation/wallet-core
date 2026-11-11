//! The recovery scan's rules against the extension's own functions:
//! `accounts-parity-scan.json` (the shapes `recoveryScanShapes` walks and which it lists unused,
//! `recoveryScanTarget`, the passphrase path, `strk20RegisteredElsewhere`) and
//! `accounts-parity-scan-signing.json` (`argentSigningRestriction` and
//! `braavosSigningRestriction` over recorded reads). Bramble's choices (the shapes, the class
//! table, the salt) go in as `ScanConfig`; `starknet_wallet_core::accounts` decides. Also
//! Krusty against the class table (`accounts.json`), as mobile's catalog tests checked it.

mod accounts_support;

use std::cell::Cell;

use accounts_support::{authorizations, felt_at, fixture, limits, scan_config, scan_shape, PHRASE};
use bramble_core::OZ_ACCOUNT_VERSION;
use serde_json::{json, Value};
use starknet_wallet_core::accounts::advisory::{self, Gathered, Read, Restriction};
use starknet_wallet_core::accounts::scan::{keep, scan_target, Found, Scan, ScanShape};
use starknet_wallet_core::accounts::{
    constructor_calldata, family_address, AccountFamily, ArgentLayout, ArgentShape,
};
use starknet_wallet_core::{
    account_public_key, felt, oz_account_class_hash, Error, Felt, NetworkId,
};

/// The extension's account tables (`account-types.ts`), as `bun run mobile:assets` renders them:
/// the evidence Krusty is checked against below.
fn catalog() -> Value {
    fixture("accounts.json")
}

fn scan() -> Value {
    fixture("accounts-parity-scan.json")
}

fn shapes(scan: &Value) -> Vec<ScanShape> {
    scan["shapes"]
        .as_array()
        .unwrap()
        .iter()
        .map(scan_shape)
        .collect()
}

fn network(fixture: &Value) -> NetworkId {
    assert_eq!(fixture["networkId"], "SN_SEPOLIA");
    assert_eq!(fixture["mnemonic"], PHRASE);
    NetworkId::Sepolia
}

fn unused(shape: &ScanShape, account_index: u32) -> Found {
    Found {
        family: shape.family,
        ladder: shape.ladder,
        class_hash: shape.class_hash,
        account_index,
        address: Felt::ONE,
        deployed: false,
        funded: false,
        restriction: None,
        privacy_elsewhere: false,
    }
}

/// The public key and address the core places on `shape` at `index`.
fn placed(shape: &ScanShape, index: u32) -> (Felt, Felt) {
    let key = account_public_key(PHRASE, None, shape.ladder, index).unwrap();
    let salt = if shape.family == AccountFamily::OpenZeppelin {
        Felt::ZERO
    } else {
        key
    };
    let address = family_address(
        shape.family,
        &shape.class_hash,
        shape.argent_shape,
        &salt,
        &key,
    )
    .unwrap();
    (key, address)
}

#[test]
fn every_shape_keeps_unused_accounts_like_the_extension() {
    let scan = scan();
    let shapes = shapes(&scan);
    assert_eq!(shapes.len(), 10);
    for (shape, vector) in shapes.iter().zip(scan["shapes"].as_array().unwrap()) {
        let context = format!("{vector}");
        // OpenZeppelin is placed at the release Bramble pins; the others at the class asked for.
        match &vector["requestedClassHash"] {
            Value::Null => assert_eq!(shape.family, AccountFamily::OpenZeppelin, "{context}"),
            requested => assert_eq!(felt_at(requested), shape.class_hash, "{context}"),
        }
        let kept = &vector["keptUnused"];
        assert_eq!(
            keep(&unused(shape, 0), shape.creatable, false),
            kept["indexZero"],
            "{context}"
        );
        assert_eq!(
            keep(&unused(shape, 3), shape.creatable, false),
            kept["indexThree"],
            "{context}"
        );
        assert_eq!(
            keep(&unused(shape, 3), shape.creatable, true),
            kept["indexThreeManual"],
            "{context}"
        );
        // Krusty agrees with the table's layout for every shape, so each places an address.
        placed(shape, 0);
    }
    // The scan walks them in this order: one class-hash probe per shape and index.
    let config = scan_config(network(&scan), Vec::new(), None);
    assert_eq!(config.shapes, shapes);
    let step = Scan::new().next(PHRASE, None, &config, None).unwrap();
    let addresses: Vec<String> = step
        .requests
        .iter()
        .map(|request| {
            let body: Value = serde_json::from_str(&request.body).unwrap();
            body["params"]["contract_address"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    let expected: Vec<String> = (0..4)
        .flat_map(|index| {
            shapes
                .iter()
                .map(move |shape| felt::canonical(&placed(shape, index).1))
        })
        .collect();
    assert_eq!(addresses, expected);
}

#[test]
fn the_scan_target_is_the_extensions() {
    let scan = scan();
    let (shapes, limits) = (shapes(&scan), limits(&scan));
    for case in scan["scanTarget"].as_array().unwrap() {
        let mut kept = Vec::new();
        for candidate in case["candidates"].as_array().unwrap() {
            let shape = &shapes[usize::try_from(candidate["shape"].as_u64().unwrap()).unwrap()];
            let index = u32::try_from(candidate["accountIndex"].as_u64().unwrap()).unwrap();
            let found = Found {
                deployed: candidate["deployed"].as_bool().unwrap(),
                funded: candidate["funded"].as_bool().unwrap(),
                ..unused(shape, index)
            };
            let keeps = keep(
                &found,
                shape.creatable,
                candidate["manual"].as_bool().unwrap(),
            );
            assert_eq!(keeps, candidate["kept"], "{} {candidate}", case["name"]);
            if keeps {
                kept.push(found);
            }
        }
        assert_eq!(
            scan_target(&kept, &limits),
            case["target"],
            "{}",
            case["name"]
        );
    }
}

/// The extension refuses a BIP-39 passphrase on discovery; mobile scans with one and leaves Ready
/// out of the shapes it passes. Either way a Ready shape never derives under one: the core refuses
/// the batch.
#[test]
fn the_passphrase_path() {
    let scan = scan();
    let passphrase = &scan["passphrase"];
    assert_eq!(passphrase["discoveryAcceptsPhraseAlone"], true);
    assert_eq!(passphrase["discoveryAcceptsPassphrase"], false);
    assert_eq!(
        passphrase["readyLadderUnderPassphrase"],
        "Ready derivation does not take a passphrase"
    );
    let mut config = scan_config(network(&scan), Vec::new(), None);
    assert!(Scan::new().next(PHRASE, None, &config, None).is_ok());
    let refused = Scan::new().next(PHRASE, Some("parity passphrase"), &config, None);
    assert!(matches!(refused, Err(Error::InvalidInput { .. })));
    config
        .shapes
        .retain(|shape| shape.ladder != starknet_wallet_core::accounts::Ladder::ArgentX);
    let step = Scan::new().next(PHRASE, Some("parity passphrase"), &config, None);
    assert_eq!(step.unwrap().requests.len(), 4 * 7);
}

fn entry_point(read: Read) -> &'static str {
    match read {
        Read::Owner => "get_owner",
        Read::OwnerType => "get_owner_type",
        Read::Guardian => "get_guardian",
        Read::Signers => "get_signers",
        Read::Deferred => "get_deferred_remove_signers",
        Read::PoolKey => "get_public_key",
    }
}

/// A recorded read as the node's reply body: an array is a result, anything else (`"fails"`,
/// absent) is a read that did not answer, which the host reports as an empty body.
fn reply(id: u64, recorded: &Value) -> String {
    match recorded {
        Value::Array(_) => json!({ "jsonrpc": "2.0", "id": id, "result": recorded }).to_string(),
        _ => String::new(),
    }
}

fn restriction_named(name: &Value) -> Option<Restriction> {
    Some(match name.as_str()? {
        "guardian" => Restriction::Guardian,
        "owner-changed" => Restriction::OwnerChanged,
        "owner-type" => Restriction::OwnerType,
        "release" => Restriction::Release,
        "strong-signer" => Restriction::StrongSigner,
        other => panic!("unknown restriction {other}"),
    })
}

#[test]
fn every_signing_verdict_is_the_extensions() {
    let signing = fixture("accounts-parity-scan-signing.json");
    network(&signing);
    let scan = scan();
    let shapes = shapes(&scan);
    let authorizations = authorizations();
    let mut cases = 0;
    for (name, family) in [
        ("argent", AccountFamily::Argent),
        ("braavos", AccountFamily::Braavos),
    ] {
        let section = &signing[name];
        // The account the verdicts were taken for: the family's first scan shape, at index 0.
        let shape = shapes.iter().find(|shape| shape.family == family).unwrap();
        let (key, address) = placed(shape, 0);
        assert_eq!(key, felt_at(&section["publicKey"]));
        assert_eq!(address, felt_at(&section["address"]));
        for case in section["cases"].as_array().unwrap() {
            let context = format!("{name}: {}", case["name"]);
            let live_class = felt_at(&case["liveClass"]);
            let recorded = case["reads"].as_object().unwrap();
            let reads = advisory::reads(family, &live_class, &authorizations, false);
            // The core reads at least what the extension consulted (Braavos's removal request
            // always; the extension only behind a strong signer).
            for consulted in recorded.keys() {
                assert!(
                    reads.iter().any(|read| entry_point(*read) == consulted),
                    "{context}: {consulted} not read"
                );
            }
            let mut gathered = Gathered {
                live_class: Some(live_class),
                ..Gathered::default()
            };
            for (id, read) in (1u64..).zip(&reads) {
                let body = reply(id, recorded.get(entry_point(*read)).unwrap_or(&Value::Null));
                gathered
                    .reads
                    .insert(*read, advisory::judge_call(&body, id));
            }
            let needs = advisory::needs_block(family, &gathered, &key, &authorizations);
            assert_eq!(
                needs,
                !case["blockTimestamp"].is_null(),
                "{context}: block read"
            );
            let block_time = match &case["blockTimestamp"] {
                Value::Number(time) => {
                    let body = json!({"jsonrpc": "2.0", "id": 9, "result": {"timestamp": time}});
                    advisory::judge_block(&body.to_string(), 9)
                }
                _ => advisory::judge_block("", 9),
            };
            let verdict =
                advisory::restriction(family, &gathered, &key, block_time, &authorizations);
            // A function that threw is no verdict.
            let expected = if case["threw"] == true {
                assert!(case["restriction"].is_null());
                None
            } else {
                restriction_named(&case["restriction"])
            };
            assert_eq!(verdict, expected, "{context}");
            cases += 1;
        }
    }
    assert_eq!(cases, 29);
}

#[test]
fn registered_elsewhere_is_the_extensions() {
    let scan = scan();
    let shapes = shapes(&scan);
    let (_, own_address) = placed(&shapes[0], 0);
    let authorizations = authorizations();
    let mut cases = 0;
    for case in scan["registeredElsewhere"].as_array().unwrap() {
        let context = format!("{}", case["name"]);
        let expected = case["registeredElsewhere"].as_bool().unwrap();
        let own_consulted = case["ownConsulted"].as_bool().unwrap();
        cases += 1;
        // The scan only ever holds a felt it derived; a non-felt address never reaches a read.
        let Some(address) = felt::parse(case["address"].as_str().unwrap()) else {
            assert!(!expected && !own_consulted, "{context}");
            continue;
        };
        assert_eq!(address, own_address, "{context}");
        let pool = !case["poolAddress"].is_null();
        let mut gathered = Gathered {
            live_class: Some(shapes[0].class_hash),
            ..Gathered::default()
        };
        for read in advisory::reads(
            AccountFamily::OpenZeppelin,
            &shapes[0].class_hash,
            &authorizations,
            pool,
        ) {
            let recorded = match &case["registered"] {
                Value::String(key) if key != "fails" => json!([key]),
                _ => Value::Null,
            };
            gathered
                .reads
                .insert(read, advisory::judge_call(&reply(5, &recorded), 5));
        }
        assert_eq!(
            gathered.reads.contains_key(&Read::PoolKey),
            pool,
            "{context}"
        );
        let consulted = Cell::new(false);
        let elsewhere = advisory::registered_elsewhere(&gathered, || {
            consulted.set(true);
            felt::parse(case["ownKey"].as_str().unwrap())
        });
        assert_eq!(elsewhere, expected, "{context}");
        if !own_consulted {
            assert!(!consulted.get(), "{context}: the own key was derived");
        }
    }
    assert_eq!(cases, 10);
}

/// Mobile's `krusty_constructors_agree_with_the_extension_table`: Krusty lists every Argent
/// deployment class at the table's layout except Xverse's build, which the caller vouches for;
/// every Braavos deployment class takes the public key alone.
#[test]
fn krusty_constructors_agree_with_the_extension_table() {
    let catalog = catalog();
    let key = Felt::from(42u64);
    let xverse = felt_at(&catalog["xverseArgentClassHash"]);
    let mut unlisted = Vec::new();
    for entry in catalog["classes"].as_array().unwrap() {
        if entry["deployment"] != true {
            continue;
        }
        let class_hash = felt_at(&entry["classHash"]);
        let shape = entry["constructorShape"].as_str().unwrap();
        if entry["family"] == "braavos" {
            assert_eq!(shape, "public-key");
            let calldata = constructor_calldata(AccountFamily::Braavos, &class_hash, None, &key);
            assert_eq!(calldata.unwrap(), vec![key]);
            continue;
        }
        let layout = ArgentLayout::from_wire(shape).expect("a known Argent layout");
        let other = match layout {
            ArgentLayout::OwnerGuardian => ArgentLayout::SignerOwnerGuardianOption,
            ArgentLayout::SignerOwnerGuardianOption => ArgentLayout::OwnerGuardian,
        };
        let build =
            |shape| constructor_calldata(AccountFamily::Argent, &class_hash, Some(shape), &key);
        let label = entry["label"].as_str().unwrap();
        // Krusty's own layout is the table's, never the other one.
        assert!(build(ArgentShape::Listed(other)).is_err(), "{label}");
        if build(ArgentShape::Listed(layout)).is_err() {
            assert!(build(ArgentShape::Unlisted(layout)).is_ok(), "{label}");
            unlisted.push(class_hash);
        }
    }
    assert_eq!(unlisted, vec![xverse]);
}

/// Mobile's `open_zeppelin_class_is_krustys_on_every_network`.
#[test]
fn open_zeppelin_class_is_krustys_on_every_network() {
    let class = felt_at(&catalog()["openZeppelin"]["classHash"]);
    for network in [NetworkId::Mainnet, NetworkId::Sepolia] {
        let krusty = oz_account_class_hash(network, OZ_ACCOUNT_VERSION).unwrap();
        assert_eq!(felt::parse(&krusty).unwrap(), class);
    }
}
