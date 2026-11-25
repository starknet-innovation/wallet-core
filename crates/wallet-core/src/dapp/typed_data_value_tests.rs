// The extension's `tests/starknet/typed-data-value-conformance.test.ts`: a payload whose type
// graph is unambiguous must also hold that graph in its values, or starknet.js would hash (or
// hang on) something the card cannot describe.

use super::{accepts_json, domain, hash};
use crate::dapp::typed_data::TypedDataRefusal;
use serde_json::{json, Value};

/// The test's envelope (a Sepolia `{ chainId, name, revision }` domain there), over the domain
/// Krusty hashes.
fn envelope(message: Value, mut types: Value) -> Value {
    let (domain, declaration) = domain(true);
    types["StarknetDomain"] = declaration;
    json!({ "domain": domain, "message": message, "primaryType": "Msg", "types": types })
}

fn accepts(message: Value, types: &Value) -> bool {
    accepts_json(&envelope(message, types.clone()))
}

fn hashes(message: Value, types: &Value) -> bool {
    hash(&envelope(message, types.clone()).to_string()).is_ok()
}

fn leaves() -> Value {
    json!({
        "Leaf": [{ "name": "a", "type": "felt" }],
        "Msg": [{ "contains": "Leaf", "name": "leaves", "type": "merkletree" }]
    })
}

// "an empty leaf list is refused before it reaches the library".
#[test]
fn an_empty_leaf_list_is_refused_before_it_reaches_the_library() {
    assert!(!accepts(json!({ "leaves": [] }), &leaves()));
    assert!(hashes(json!({ "leaves": [{ "a": "0x1" }] }), &leaves()));
}

// "a merkle value that is not an array is refused too".
#[test]
fn a_merkle_value_that_is_not_an_array_is_refused_too() {
    for value in [
        json!("0x1"),
        json!({ "0": { "a": "0x1" } }),
        Value::Null,
        json!(3),
    ] {
        assert!(!accepts(json!({ "leaves": value }), &leaves()));
    }
}

fn action_types() -> Value {
    json!({
        "Action": [
            { "name": "Pay", "type": "(felt)" },
            { "name": "Nothing", "type": "()" }
        ],
        "Msg": [{ "contains": "Action", "name": "action", "type": "enum" }]
    })
}

fn action(value: Value) -> bool {
    accepts(json!({ "action": value }), &action_types())
}

// "an indexable non-array payload is refused, not hashed character by character".
#[test]
fn an_indexable_non_array_payload_is_refused() {
    assert!(!action(json!({ "Pay": "SN_MAIN" })));
    assert!(!action(
        json!({ "Pay": { "0": "1", "note": "you are safe" } })
    ));
    assert!(hashes(
        json!({ "action": { "Pay": ["1"] } }),
        &action_types()
    ));
}

// "a payload longer or shorter than the declared tuple is refused".
#[test]
fn a_payload_longer_or_shorter_than_the_declared_tuple_is_refused() {
    assert!(!action(json!({ "Pay": ["1", "SN_MAIN"] })));
    assert!(!action(json!({ "Pay": [] })));
    assert!(hashes(
        json!({ "action": { "Nothing": [] } }),
        &action_types()
    ));
    assert!(!action(json!({ "Nothing": ["Approve 1 STRK for swap"] })));
}

// "a second variant, or an undeclared one, is refused".
#[test]
fn a_second_variant_or_an_undeclared_one_is_refused() {
    assert!(!action(json!({ "Nothing": [], "Pay": ["1"] })));
    assert!(!action(json!({ "Withdraw": ["everything"] })));
    assert!(!action(json!({})));
}

// "a field named after a prototype member is refused, not hashed off the prototype".
#[test]
fn a_field_named_after_a_prototype_member_is_read_as_an_own_property() {
    let types = json!({
        "Foo": [{ "name": "name", "type": "shortstring" }],
        "Msg": [{ "name": "constructor", "type": "Foo" }]
    });
    assert!(!accepts(json!({}), &types));
    assert!(hashes(json!({ "constructor": { "name": "ok" } }), &types));
}

// "a declared field the message omits is refused".
#[test]
fn a_declared_field_the_message_omits_is_refused() {
    let types = json!({ "Msg": [{ "name": "amount", "type": "felt" }] });
    assert!(!accepts(json!({}), &types));
    assert!(!accepts(json!({ "other": "1" }), &types));
    // Undeclared keys beside the declared ones pass the gate (the card shows them as unsigned),
    // but Krusty refuses to hash them, so mobile answers 114 where the extension signs.
    let extra = json!({ "amount": "1", "other": "1" });
    assert!(accepts(extra.clone(), &types));
    let text = envelope(extra, types).to_string();
    assert_eq!(hash(&text), Err(TypedDataRefusal::Unhashable));
    assert!(hashes(
        json!({ "amount": "1" }),
        &json!({ "Msg": [{ "name": "amount", "type": "felt" }] })
    ));
}

// "a leaf type may not hold an object, and a struct type may not hold a scalar".
#[test]
fn a_leaf_may_not_hold_an_object_nor_a_struct_a_scalar() {
    let felt = json!({ "Msg": [{ "name": "amount", "type": "felt" }] });
    assert!(!accepts(json!({ "amount": { "a": 1 } }), &felt));
    let details = json!({
        "Details": [{ "name": "amount", "type": "u128" }],
        "Msg": [{ "name": "details", "type": "Details" }]
    });
    assert!(!accepts(json!({ "details": "1000 USDC" }), &details));
}

// "a dotted field name is refused, so a flat field cannot imitate a nested one".
#[test]
fn a_dotted_field_name_is_refused() {
    let flat = json!({
        "Msg": [
            { "name": "to.address", "type": "ContractAddress" },
            { "name": "amount", "type": "u128" }
        ]
    });
    assert!(!accepts(
        json!({ "amount": "5", "to.address": "0x1111" }),
        &flat
    ));
    let nested = json!({
        "Msg": [
            { "name": "to", "type": "Recipient" },
            { "name": "amount", "type": "u128" }
        ],
        "Recipient": [{ "name": "address", "type": "ContractAddress" }]
    });
    assert!(hashes(
        json!({ "amount": "5", "to": { "address": "0x1111" } }),
        &nested
    ));
    let dotted_type = json!({
        "Re.cipient": [{ "name": "address", "type": "felt" }],
        "Msg": [{ "name": "to", "type": "Re.cipient" }]
    });
    assert!(!accepts(
        json!({ "to": { "address": "0x1" } }),
        &dotted_type
    ));
    let spaced = json!({ "Msg": [{ "name": "Session Key", "type": "felt" }] });
    assert!(hashes(json!({ "Session Key": "0x9" }), &spaced));
}

// Nesting stops at the typed-data budget's depth (the budget and the value walk's `MAX_DEPTH`
// are the same number, so either refuses first).
#[test]
fn values_nest_no_deeper_than_the_budget() {
    let types = json!({
        "Msg": [{ "name": "next", "type": "Node*" }],
        "Node": [{ "name": "next", "type": "Node*" }]
    });
    let nested = |levels: usize| {
        let mut deep = json!([]);
        for _ in 0..levels {
            deep = json!([{ "next": deep }]);
        }
        json!({ "next": deep })
    };
    // A container must sit above the budget's depth: eleven `[{…}]` levels put the innermost
    // array at depth 24 of the envelope, one too deep; ten fit.
    assert!(!accepts(nested(11), &types));
    assert!(accepts(nested(10), &types));
}
