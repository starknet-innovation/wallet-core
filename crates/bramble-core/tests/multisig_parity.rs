//! The Argent multisig account the extension builds, reproduced by
//! `starknet_wallet_core::accounts::multisig` on Bramble's pinned class:
//! `nostr-multisig-parity.json` holds the extension's signer GUIDs, normalised configurations,
//! constructor calldata, counterfactual addresses, refusals and joined records
//! (`account-contracts.ts`, `repository-schema.ts`); `multisig.json` is Bramble's pinned class and
//! limits as `bun run mobile:assets` renders them. A wrong address here is lost funds.

use serde_json::{json, Value};
use starknet_wallet_core::accounts::multisig::{
    normalize, predict, signer_guid_hex, ArgentMultisigClass, ArgentMultisigConfig,
};
use starknet_wallet_core::accounts::multisig_read::{
    MultisigAuthorityRead, MultisigAuthorityStatus,
};
use starknet_wallet_core::accounts::multisig_records::{
    authority, is_authority, is_record, record_id, Record,
};
use starknet_wallet_core::{felt, NetworkId};

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn text(value: &Value) -> String {
    value.as_str().unwrap().to_string()
}

fn strings(value: &Value) -> Vec<String> {
    value.as_array().unwrap().iter().map(text).collect()
}

/// Bramble's pinned Argent Multisig 0.2 class and signer limit.
fn class() -> ArgentMultisigClass {
    let constants = fixture("multisig.json");
    ArgentMultisigClass::new(
        constants["argentMultisig"]["classHash"].as_str().unwrap(),
        constants["limits"]["signers"].as_u64().unwrap() as usize,
    )
    .unwrap()
}

fn name_length() -> usize {
    fixture("multisig.json")["limits"]["nameLength"]
        .as_u64()
        .unwrap() as usize
}

fn config(vector: &Value) -> ArgentMultisigConfig {
    ArgentMultisigConfig {
        threshold: u32::try_from(vector["threshold"].as_u64().unwrap()).unwrap(),
        signer_public_keys: strings(&vector["signerPublicKeys"]),
    }
}

/// Mobile's `argent_multisig_layout_matches_the_extension`.
#[test]
fn argent_multisig_layout_matches_the_extension() {
    let class = class();
    let argent = &fixture("nostr-multisig-parity.json")["argent"];
    for vector in argent["guids"].as_array().unwrap() {
        assert_eq!(
            signer_guid_hex(&text(&vector["publicKey"])).unwrap(),
            text(&vector["guid"])
        );
    }
    for vector in argent["configs"].as_array().unwrap() {
        let config = config(vector);
        assert_eq!(
            normalize(&config, &class).unwrap().0.signer_public_keys,
            strings(&vector["normalized"])
        );
        let prediction = predict(&config, &text(&vector["salt"]), &class).unwrap();
        assert_eq!(
            prediction.constructor_calldata,
            strings(&vector["calldata"])
        );
        assert_eq!(prediction.address, text(&vector["address"]));
    }
    for vector in argent["rejected"].as_array().unwrap() {
        assert!(normalize(&config(vector), &class).is_err(), "{vector}");
    }
}

fn respond(read: &mut MultisigAuthorityRead, result: Value) {
    let request = read.next_request().unwrap();
    let body = json!({"jsonrpc": "2.0", "id": request.id, "result": result}).to_string();
    read.accept_response(request.id, &body).unwrap();
}

/// The chain half of mobile's `joined_records_are_what_the_extension_stores`: the authority read
/// over the extension's joined accounts, and the records it stored for them, which the registry
/// rules accept and which, when resolved, commit to the authority the chain reported. Writing the
/// registry file is Bramble's and stays with that test in mobile.
#[test]
fn joined_records_are_valid_and_carry_the_chain_authority() {
    let fixture = fixture("nostr-multisig-parity.json");
    let class = class();
    assert_eq!(text(&fixture["classHash"]), class.class_hash());
    for vector in fixture["joins"].as_array().unwrap() {
        let mut read =
            MultisigAuthorityRead::new(NetworkId::Sepolia, &text(&vector["address"]), &class)
                .unwrap();
        respond(&mut read, json!("0x534e5f5345504f4c4941"));
        respond(&mut read, json!({"block_hash": "0x1", "block_number": 1}));
        respond(&mut read, fixture["classHash"].clone());
        respond(
            &mut read,
            json!([format!("0x{:x}", vector["threshold"].as_u64().unwrap())]),
        );
        let guids = strings(&vector["signerGuids"]);
        let mut reply = vec![json!(format!("0x{:x}", guids.len()))];
        reply.extend(guids.iter().rev().map(|guid| json!(guid)));
        respond(&mut read, Value::Array(reply));
        assert_eq!(read.status(), Some(MultisigAuthorityStatus::Available));
        let (threshold, read_guids) = read.authority().unwrap();
        let read_guids: Vec<String> = read_guids.iter().map(felt::canonical).collect();
        assert_eq!(u64::from(threshold), vector["threshold"].as_u64().unwrap());
        assert_eq!(read_guids, guids, "ascending, as the extension stores them");
        assert!(is_authority(&read_guids, threshold, &class));

        let record: Record = serde_json::from_value(vector["record"].clone()).unwrap();
        assert_eq!(serde_json::to_value(&record).unwrap(), vector["record"]);
        assert!(is_record(&record, &class, name_length()), "{vector}");
        assert_eq!(
            record_id(NetworkId::Sepolia, &read.address()).unwrap(),
            record.id
        );
        // A join whose GUIDs did not all resolve to Stark keys stores no `currentConfig`, so the
        // record itself commits to no authority.
        match &record.current_config {
            Some(_) => {
                let committed = authority(&record, &class).unwrap();
                assert_eq!(committed.threshold, threshold);
                assert_eq!(committed.signer_guids, guids);
            }
            None => assert_eq!(authority(&record, &class), None),
        }
    }
}
