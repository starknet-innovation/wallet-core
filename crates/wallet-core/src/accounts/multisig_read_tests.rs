//! Mobile's `authority_reads_settle_every_status` (`ffi_nostr_multisig_tests.rs`) against a canned
//! node.

use serde_json::{json, Value};

use super::*;
use crate::accounts::multisig::tests::class;

const SEPOLIA_CHAIN: &str = "0x534e5f5345504f4c4941";

fn fresh() -> MultisigAuthorityRead {
    MultisigAuthorityRead::new(NetworkId::Sepolia, "0x123", &class()).unwrap()
}

fn respond(read: &mut MultisigAuthorityRead, result: Value) {
    let request = read.next_request().unwrap();
    let body = json!({"jsonrpc":"2.0","id":request.id,"result":result}).to_string();
    read.accept_response(request.id, &body).unwrap();
}

fn respond_error(read: &mut MultisigAuthorityRead, code: i64) {
    let request = read.next_request().unwrap();
    let body =
        json!({"jsonrpc":"2.0","id":request.id,"error":{"code":code,"message":"x"}}).to_string();
    read.accept_response(request.id, &body).unwrap();
}

#[test]
fn authority_reads_settle_every_status() {
    let mut read = fresh();
    let first = read.next_request().unwrap();
    assert!(
        read.accept_response(first.id + 1, "{}").is_err(),
        "id mismatch refused"
    );
    respond(&mut read, json!("0x534e5f4d41494e"));
    assert_eq!(
        read.status(),
        Some(MultisigAuthorityStatus::NetworkUnavailable),
        "wrong chain"
    );

    let mut undeployed = fresh();
    respond(&mut undeployed, json!(SEPOLIA_CHAIN));
    respond(
        &mut undeployed,
        json!({"block_hash":"0xabc","block_number":7}),
    );
    respond_error(&mut undeployed, 20);
    assert_eq!(
        undeployed.status(),
        Some(MultisigAuthorityStatus::Undeployed)
    );

    let mut other_class = fresh();
    respond(&mut other_class, json!(SEPOLIA_CHAIN));
    respond(
        &mut other_class,
        json!({"block_hash":"0xabc","block_number":7}),
    );
    respond(
        &mut other_class,
        json!("0x1d1777db36cdd06dd62cfde77b1b6ae06412af95d57a13dc40ac77b8a702381"),
    );
    assert_eq!(
        other_class.status(),
        Some(MultisigAuthorityStatus::UnsupportedClass)
    );

    let mut bad = fresh();
    respond(&mut bad, json!(SEPOLIA_CHAIN));
    respond(&mut bad, json!({"block_hash":"0xabc","block_number":7}));
    respond(&mut bad, json!(class().class_hash()));
    respond(&mut bad, json!(["0x3"]));
    respond(&mut bad, json!(["0x2", "0x1", "0x1"]));
    assert_eq!(
        bad.status(),
        Some(MultisigAuthorityStatus::InvalidConfig),
        "duplicate GUIDs"
    );

    let mut repinned = fresh();
    respond(&mut repinned, json!(SEPOLIA_CHAIN));
    for _ in 0..2 {
        respond(
            &mut repinned,
            json!({"block_hash":"0xabc","block_number":7}),
        );
        respond_error(&mut repinned, 24);
        assert_eq!(repinned.status(), None, "re-pins after Block not found");
    }
    respond(
        &mut repinned,
        json!({"block_hash":"0xabc","block_number":7}),
    );
    respond_error(&mut repinned, 24);
    assert_eq!(
        repinned.status(),
        Some(MultisigAuthorityStatus::NetworkUnavailable),
        "three attempts"
    );

    let mut transport = fresh();
    let request = transport.next_request().unwrap();
    transport.transport_failed(request.id);
    assert_eq!(
        transport.status(),
        Some(MultisigAuthorityStatus::NetworkUnavailable)
    );
}
