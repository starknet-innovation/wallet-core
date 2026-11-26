// The side panel's `sign-request-decode.test.ts` cases that reach an accepted document, with each
// expected text spelled as `signValueText` prints it.

use super::*;
use crate::dapp::typed_data::tests::BUDGET;
use crate::js::json;
use crate::{kms, Felt};
use serde_json::{json, Value as Json};

fn rev1(mut types: Json, message: Json) -> Json {
    types["StarknetDomain"] = json!([
        { "name": "name", "type": "shortstring" },
        { "name": "version", "type": "shortstring" },
        { "name": "chainId", "type": "shortstring" },
        { "name": "revision", "type": "shortstring" },
    ]);
    json!({
        "domain": { "chainId": "SN_MAIN", "name": "Bramble Test", "revision": "1", "version": "1" },
        "message": message,
        "primaryType": "Message",
        "types": types,
    })
}

fn rev0(mut types: Json, message: Json, domain: Json) -> Json {
    types["StarkNetDomain"] = json!([
        { "name": "name", "type": "felt" },
        { "name": "version", "type": "felt" },
        { "name": "chainId", "type": "felt" },
    ]);
    json!({ "domain": domain, "message": message, "primaryType": "Message", "types": types })
}

fn rows(document: &Json) -> Option<SignRows> {
    let value = json::parse(&document.to_string()).unwrap();
    CheckedTypedData::check(&value, &BUDGET)
        .expect("the gate accepts it")
        .sign_rows()
}

fn pairs(rows: &[SignRow]) -> Vec<(&str, &str)> {
    rows.iter()
        .map(|row| (row.key.as_str(), row.value.as_str()))
        .collect()
}

/// The message's signed rows after the primary type, asserting nothing is unsigned.
fn message_rows(decoded: SignRows) -> Vec<(String, String)> {
    assert!(decoded.message.unsigned.is_empty());
    decoded
        .message
        .signed
        .into_iter()
        .skip(1)
        .map(|row| (row.key, row.value))
        .collect()
}

fn message(types: Json, message: Json) -> Vec<(String, String)> {
    message_rows(rows(&rev1(types, message)).unwrap())
}

fn owned(rows: &[(&str, &str)]) -> Vec<(String, String)> {
    rows.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

#[test]
fn a_basic_document_reads_as_the_extension_prints_it() {
    let decoded = rows(&rev1(
        json!({ "Message": [{ "name": "content", "type": "felt" }] }),
        json!({ "content": "hello" }),
    ))
    .unwrap();
    assert_eq!(
        pairs(&decoded.domain.signed),
        [
            (
                "name",
                "\"Bramble Test\" (shortstring = 0x4272616d626c652054657374)"
            ),
            ("version", "0x1 (shortstring)"),
            ("chainId", "\"SN_MAIN\" (shortstring = 0x534e5f4d41494e)"),
            ("revision", "0x1 (shortstring)"),
        ]
    );
    // The primary type is the message block's first row, never a heading.
    assert_eq!(
        pairs(&decoded.message.signed),
        [
            ("⟨primary type⟩", "Message"),
            ("content", "\"hello\" (felt short string = 0x68656c6c6f)"),
        ]
    );
    assert!(decoded.domain.unsigned.is_empty() && decoded.message.unsigned.is_empty());
}

#[test]
fn each_leaf_kind_names_its_type_and_felt() {
    let types = json!({ "Message": [
        { "name": "flag", "type": "bool" },
        { "name": "amount", "type": "u128" },
        { "name": "delta", "type": "i128" },
        { "name": "to", "type": "ContractAddress" },
        { "name": "entry", "type": "selector" },
        { "name": "note", "type": "string" },
        { "name": "items", "type": "felt*" },
    ]});
    let got = message(
        types,
        json!({ "flag": false, "amount": "0x0A", "delta": "-5", "to": "0x00AB",
                "entry": "transfer", "note": "hi \\ there", "items": [] }),
    );
    let minus_five = crate::felt::canonical(&-Felt::from(5u8));
    let transfer = crate::felt::canonical(&kms::selector("transfer"));
    assert_eq!(
        got,
        owned(&[
            ("flag", "false (bool = 0x0)"),
            ("amount", "10 (u128 = 0xa)"),
            ("delta", &format!("-5 (i128 = {minus_five})")),
            ("to", "0xab (ContractAddress)"),
            (
                "entry",
                &format!("\"transfer\" (selector entry point = {transfer})")
            ),
            // Krusty does not export a ByteArray's hash, so none is printed.
            ("note", "\"hi \\\\ there\" (string)"),
            ("items", "[]"),
        ])
    );
}

#[test]
fn the_preset_u256_states_its_total_above_its_limbs() {
    let got = message(
        json!({ "Message": [{ "name": "amount", "type": "u256" }] }),
        json!({ "amount": { "low": "0x64", "high": "0x1" } }),
    );
    assert_eq!(
        got,
        owned(&[
            (
                "amount",
                "340282366920938463463374607431768211556 (u256 total of high and low)"
            ),
            ("amount.low", "100 (u128 = 0x64)"),
            ("amount.high", "1 (u128 = 0x1)"),
        ])
    );
}

#[test]
fn an_enum_states_the_variant_tuple_it_hashes() {
    let types = json!({
        "Message": [{ "name": "action", "type": "enum", "contains": "Action" }],
        "Action": [{ "name": "Pay", "type": "(felt,u128)" }, { "name": "Stop", "type": "()" }],
    });
    assert_eq!(
        message(types.clone(), json!({ "action": { "Pay": ["0x5", 7] } })),
        owned(&[
            ("action.Pay", "(felt,u128)"),
            ("action.Pay[0]", "0x5 (felt)"),
            ("action.Pay[1]", "7 (u128 = 0x7)"),
        ])
    );
    assert_eq!(
        message(types, json!({ "action": { "Stop": [] } })),
        owned(&[("action.Stop", "()")])
    );
}

fn tree(leaves: &[&str]) -> Vec<(String, String)> {
    let leaves: Vec<Json> = leaves.iter().map(|leaf| json!({ "leaf": leaf })).collect();
    message_rows(
        rows(&rev0(
            json!({
                "Inner": [{ "name": "leaf", "type": "felt" }],
                "Message": [{ "name": "tree", "type": "merkletree", "contains": "Inner" }],
            }),
            json!({ "tree": leaves }),
            json!({ "chainId": "SN_MAIN", "name": "Bramble Test", "version": "1" }),
        ))
        .unwrap(),
    )
}

#[test]
fn merkle_leaves_are_addressed_by_the_pair_the_root_hashes_them_in() {
    // One pair: its order is the one thing the root does not fix, so either order reads alike.
    let pair = owned(&[("tree{}.leaf", "0x1 (felt)"), ("tree{}.leaf", "0x2 (felt)")]);
    assert_eq!(tree(&["0x1", "0x2"]), pair);
    assert_eq!(tree(&["0x2", "0x1"]), pair);
    // A third leaf is padded against 0x0: its own group beside the pair.
    assert_eq!(
        tree(&["0x3", "0x1", "0x2"]),
        owned(&[
            ("tree{a}.leaf", "0x1 (felt)"),
            ("tree{a}.leaf", "0x3 (felt)"),
            ("tree{b}.leaf", "0x2 (felt)"),
        ])
    );
}

#[test]
fn a_legacy_domains_undeclared_revision_is_shown_as_not_signed() {
    let decoded = rows(&rev0(
        json!({ "Message": [{ "name": "content", "type": "felt" }] }),
        json!({ "content": "0x1" }),
        json!({ "chainId": "SN_MAIN", "name": "Bramble Test", "revision": "0", "version": "1" }),
    ))
    .unwrap();
    assert_eq!(pairs(&decoded.domain.unsigned), [("revision", "0")]);
    assert_eq!(decoded.domain.signed.len(), 3);
    assert!(decoded.message.unsigned.is_empty());
}

/// `levels` nested structs, each field name 64 characters, so the deepest key is
/// `65 * levels - 1` long.
fn nested(levels: usize) -> Json {
    let name = |level: usize| format!("{level}{}", "n".repeat(63));
    let own = |level: usize| match level {
        0 => "Message".to_string(),
        _ => format!("T{level}"),
    };
    let mut types = json!({});
    let mut message = json!("0x1");
    for level in (0..levels).rev() {
        let ty = if level + 1 == levels {
            "felt".into()
        } else {
            own(level + 1)
        };
        types[own(level)] = json!([{ "name": name(level), "type": ty }]);
        message = json!({ name(level): message });
    }
    rev1(types, message)
}

#[test]
fn a_key_longer_than_the_card_prints_withholds_the_rows() {
    // Four levels spell a 259-character key, the most the card prints; five spell 324.
    let decoded = rows(&nested(4)).expect("a 259-character key is printed");
    assert_eq!(
        decoded.message.signed.last().map(|row| row.key.len()),
        Some(259)
    );
    assert_eq!(rows(&nested(5)), None);
}

#[test]
fn display_escapes_and_truncates_as_the_extension_does() {
    assert_eq!(display("\u{202e}ab\\c\u{7}"), "\\u202eab\\\\c\\u0007");
    assert_eq!(
        display(&"x".repeat(300)),
        format!("{}… (+44 characters, see Raw typed data)", "x".repeat(256))
    );
    assert_eq!(sign_name_text("a\u{202e}b.c"), "«abc»");
    assert_eq!(sign_name_text("\u{202e}"), "«unnamed»");
}
