// The extension's `tests/starknet/typed-data-containers.test.ts` (MCW-V09-101, container half):
// `merkletree` keeps its leaf type out of `encodeType`, and an `enum` field spells like a plain
// reference. `H_*` literals are starknet.js 10.8's hash of the `P_*` payload beside them.

use super::tests::{accepts_json, domain, felt, hash, raw_hash};
use super::TypedDataRefusal;
use serde_json::{json, Value};

/// The test's `envelope`, over the domain Krusty hashes rather than its one-field domain.
fn envelope(primary: &str, mut types: Value, message: Value, revision_one: bool) -> Value {
    let (domain, declaration) = domain(revision_one);
    let name = if revision_one {
        "StarknetDomain"
    } else {
        "StarkNetDomain"
    };
    types[name] = declaration;
    json!({ "domain": domain, "message": message, "primaryType": primary, "types": types })
}

// `merklePair`: honest struct leaves, and a forged tree of felts that are those leaves' struct
// hashes (starknet.js `getStructHash`).
const P_MERKLE_HONEST_0: &str = r#"{"domain":{"chainId":"SN_MAIN","name":"Bramble Test","version":"1"},"message":{"root":[{"contract_address":"0xdead","selector":"0x1"},{"contract_address":"0xdead","selector":"0x2"}]},"primaryType":"Session","types":{"Policy":[{"name":"contract_address","type":"felt"},{"name":"selector","type":"felt"}],"Session":[{"contains":"Policy","name":"root","type":"merkletree"}],"StarkNetDomain":[{"name":"name","type":"felt"},{"name":"version","type":"felt"},{"name":"chainId","type":"felt"}]}}"#;
const H_MERKLE_HONEST_0: &str = "0x365d1654d081f0216abd74a3f9d76843a403d229005aadfe79be556ee7390ae";
const P_MERKLE_FORGED_0: &str = r#"{"domain":{"chainId":"SN_MAIN","name":"Bramble Test","version":"1"},"message":{"root":["0x7b5496d0b49b65e4a3ab032a32f58b9bb17c190dc6e3a6716e2c4c4c8b69525","0x5722b55208db94e1fda0568dbcab1063fd92bb5451b4f5b612576b7a21127d2"]},"primaryType":"Session","types":{"Session":[{"contains":"felt","name":"root","type":"merkletree"}],"StarkNetDomain":[{"name":"name","type":"felt"},{"name":"version","type":"felt"},{"name":"chainId","type":"felt"}]}}"#;
const P_MERKLE_HONEST_1: &str = r#"{"domain":{"chainId":"SN_MAIN","name":"Bramble Test","revision":"1","version":"1"},"message":{"root":[{"contract_address":"0xdead","selector":"0x1"},{"contract_address":"0xdead","selector":"0x2"}]},"primaryType":"Session","types":{"Policy":[{"name":"contract_address","type":"ContractAddress"},{"name":"selector","type":"selector"}],"Session":[{"contains":"Policy","name":"root","type":"merkletree"}],"StarknetDomain":[{"name":"name","type":"shortstring"},{"name":"version","type":"shortstring"},{"name":"chainId","type":"shortstring"},{"name":"revision","type":"shortstring"}]}}"#;
const H_MERKLE_HONEST_1: &str = "0x4e99a631dbe0a71b3615262db7269b1fa9ee59f14d691944d3701cfc6212d3a";
const P_MERKLE_FORGED_1: &str = r#"{"domain":{"chainId":"SN_MAIN","name":"Bramble Test","revision":"1","version":"1"},"message":{"root":["0x1ddc354a793f66cd044dc9e7fe7c18455a714fca1374b031409c0c03b7e0e5e","0x4d5ddecdcc68290982724de7c9888b0af9021009234fd18efcbdb95f8a8b031"]},"primaryType":"Session","types":{"Session":[{"contains":"felt","name":"root","type":"merkletree"}],"StarknetDomain":[{"name":"name","type":"shortstring"},{"name":"version","type":"shortstring"},{"name":"chainId","type":"shortstring"},{"name":"revision","type":"shortstring"}]}}"#;

// "the basic-leaf substitution really collides on revision 0" and "... on revision 1".
#[test]
fn the_basic_leaf_substitution_is_refused_on_both_revisions() {
    for (honest, forged, expected) in [
        (P_MERKLE_HONEST_0, P_MERKLE_FORGED_0, H_MERKLE_HONEST_0),
        (P_MERKLE_HONEST_1, P_MERKLE_FORGED_1, H_MERKLE_HONEST_1),
    ] {
        // Krusty hashes the forged tree to the honest root too: only the gate tells them apart.
        assert_eq!(raw_hash(forged), raw_hash(honest));
        assert_eq!(hash(honest), Ok(felt(expected)));
        assert_eq!(hash(forged), Err(TypedDataRefusal::Invalid));
    }
}

fn tree(contains: &str, leaves: Value, revision_one: bool) -> Value {
    envelope(
        "Message",
        json!({
            "Inner": [{ "name": "leaf", "type": "felt" }],
            "Message": [{ "contains": contains, "name": "tree", "type": "merkletree" }]
        }),
        json!({ "tree": leaves }),
        revision_one,
    )
}

const P_TREE_INNER_1: &str = r#"{"domain":{"chainId":"SN_MAIN","name":"Bramble Test","revision":"1","version":"1"},"message":{"tree":[{"leaf":"0x1"},{"leaf":"0x2"}]},"primaryType":"Message","types":{"Inner":[{"name":"leaf","type":"felt"}],"Message":[{"contains":"Inner","name":"tree","type":"merkletree"}],"StarknetDomain":[{"name":"name","type":"shortstring"},{"name":"version","type":"shortstring"},{"name":"chainId","type":"shortstring"},{"name":"revision","type":"shortstring"}]}}"#;
const H_TREE_INNER_1: &str = "0x1a887448ab809cb122b95685588cf1e3fe433cb787dc872a27273a3e025c1a2";
const P_TREE_U256_1: &str = r#"{"domain":{"chainId":"SN_MAIN","name":"Bramble Test","revision":"1","version":"1"},"message":{"tree":[{"high":"0x0","low":"0x1"},{"high":"0x0","low":"0x2"}]},"primaryType":"Message","types":{"Inner":[{"name":"leaf","type":"felt"}],"Message":[{"contains":"u256","name":"tree","type":"merkletree"}],"StarknetDomain":[{"name":"name","type":"shortstring"},{"name":"version","type":"shortstring"},{"name":"chainId","type":"shortstring"},{"name":"revision","type":"shortstring"}]}}"#;
const H_TREE_U256_1: &str = "0x66483baca2982e83c4b49d0dcc47aab6c4ce016aea19bb00883732c3a550e35";
const P_TREE_INNER_0: &str = r#"{"domain":{"chainId":"SN_MAIN","name":"Bramble Test","version":"1"},"message":{"tree":[{"leaf":"0x1"},{"leaf":"0x2"}]},"primaryType":"Message","types":{"Inner":[{"name":"leaf","type":"felt"}],"Message":[{"contains":"Inner","name":"tree","type":"merkletree"}],"StarkNetDomain":[{"name":"name","type":"felt"},{"name":"version","type":"felt"},{"name":"chainId","type":"felt"}]}}"#;
const H_TREE_INNER_0: &str = "0x93feb38db510b70e5a8a5af2a5081aea1d2dade0cf25bf553d0688443bfbbd";

// "a declared struct leaf and a preset leaf still hash".
#[test]
fn a_declared_struct_leaf_and_a_preset_leaf_still_hash() {
    assert_eq!(hash(P_TREE_INNER_1), Ok(felt(H_TREE_INNER_1)));
    assert_eq!(hash(P_TREE_U256_1), Ok(felt(H_TREE_U256_1)));
    assert_eq!(hash(P_TREE_INNER_0), Ok(felt(H_TREE_INNER_0)));
    let legacy_preset = tree("u256", json!([{ "high": "0x0", "low": "0x1" }]), false);
    assert!(!accepts_json(&legacy_preset));
}

// "every basic leaf type is refused, on both revisions".
#[test]
fn every_basic_leaf_type_is_refused_on_both_revisions() {
    for revision_one in [false, true] {
        for basic in [
            "felt",
            "u128",
            "i128",
            "bool",
            "selector",
            "shortstring",
            "string",
            "timestamp",
            "ContractAddress",
            "ClassHash",
        ] {
            assert!(!accepts_json(&tree(
                basic,
                json!(["0x1", "0x2"]),
                revision_one
            )));
        }
        for bad in ["Inner*", "Ghost", "enum", "merkletree"] {
            assert!(!accepts_json(&tree(bad, json!(["0x1"]), revision_one)));
        }
    }
}

// "a variant list is not a leaf either".
#[test]
fn a_variant_list_is_not_a_leaf_either() {
    assert!(!accepts_json(&envelope(
        "Message",
        json!({
            "Message": [
                { "contains": "Variants", "name": "choice", "type": "enum" },
                { "contains": "Variants", "name": "tree", "type": "merkletree" }
            ],
            "Variants": [{ "name": "Some", "type": "(felt)" }]
        }),
        json!({ "choice": { "Some": ["0x1"] }, "tree": [{ "Some": ["0x1"] }] }),
        true,
    )));
}

fn msg(types: Value, message: Value) -> bool {
    accepts_json(&envelope("Msg", types, message, true))
}

const P_AS_ENUM: &str = r#"{"domain":{"chainId":"SN_MAIN","name":"Bramble Test","revision":"1","version":"1"},"message":{"f":{"V1":["0x5"]}},"primaryType":"Msg","types":{"Msg":[{"contains":"Variants","name":"f","type":"enum"}],"Variants":[{"name":"V1","type":"(felt)"}],"StarknetDomain":[{"name":"name","type":"shortstring"},{"name":"version","type":"shortstring"},{"name":"chainId","type":"shortstring"},{"name":"revision","type":"shortstring"}]}}"#;
const H_AS_ENUM: &str = "0x5286ba22795f3a246af9418637bc1e76b56709e0888c5d973283350988206d1";

// "the enum field and the plain reference share one encodeType" (the shared `encodeType` itself
// is starknet.js's and is not re-derived here).
#[test]
fn the_enum_field_and_the_plain_reference_are_not_both_signable() {
    assert_eq!(hash(P_AS_ENUM), Ok(felt(H_AS_ENUM)));
    assert!(!msg(
        json!({
            "Msg": [{ "name": "f", "type": "Variants" }],
            "Variants": [{ "name": "V1", "type": "(felt)" }]
        }),
        json!({}),
    ));
}

const P_ENUM_TUPLES: &str = r#"{"domain":{"chainId":"SN_MAIN","name":"Bramble Test","revision":"1","version":"1"},"message":{"f":{"V1":["0x5","0x6"]}},"primaryType":"Msg","types":{"Msg":[{"contains":"Variants","name":"f","type":"enum"}],"Variants":[{"name":"V1","type":"(felt,u128)"},{"name":"V2","type":"()"}],"StarknetDomain":[{"name":"name","type":"shortstring"},{"name":"version","type":"shortstring"},{"name":"chainId","type":"shortstring"},{"name":"revision","type":"shortstring"}]}}"#;
const H_ENUM_TUPLES: &str = "0x6a8e84da63820d14227286690ee99c278e63dc7532e326a83412e4c2983b574";

// "a variant list holds nothing but non-empty tuple payloads".
#[test]
fn a_variant_list_holds_nothing_but_tuple_payloads() {
    let enum_field = json!({ "contains": "Variants", "name": "f", "type": "enum" });
    for variants in [
        json!([]),
        json!([{ "name": "V1", "type": "felt" }]),
        json!([{ "name": "V1", "type": "(felt)" }, { "name": "V2", "type": "felt" }]),
        json!([{ "contains": "Msg", "name": "V1", "type": "merkletree" }]),
    ] {
        assert!(!msg(
            json!({ "Msg": [enum_field.clone()], "Variants": variants }),
            json!({ "f": { "V1": ["0x5"] } }),
        ));
    }
    assert_eq!(hash(P_ENUM_TUPLES), Ok(felt(H_ENUM_TUPLES)));
}

// "a variant list is reachable only through the enum field naming it".
#[test]
fn a_variant_list_is_reachable_only_through_the_enum_field_naming_it() {
    let enum_field = json!({ "contains": "Variants", "name": "f", "type": "enum" });
    let variants = json!([{ "name": "V1", "type": "(felt)" }]);
    for types in [
        json!({ "Msg": [enum_field.clone(), { "name": "g", "type": "Variants" }], "Variants": variants.clone() }),
        json!({ "Msg": [enum_field.clone(), { "name": "g", "type": "Variants*" }], "Variants": variants.clone() }),
        json!({
            "Msg": [enum_field.clone(), { "contains": "Variants2", "name": "g", "type": "enum" }],
            "Variants": variants.clone(),
            "Variants2": [{ "name": "W", "type": "(Variants)" }]
        }),
    ] {
        assert!(!msg(types, json!({ "f": { "V1": ["0x5"] } })));
    }
    // Nor as the primary type, nor as the domain struct.
    assert!(!accepts_json(&envelope(
        "Variants",
        json!({ "Msg": [enum_field], "Variants": variants }),
        json!({ "V1": ["0x5"] }),
        true,
    )));
    assert!(!msg(
        json!({ "Msg": [{ "contains": "StarknetDomain", "name": "f", "type": "enum" }] }),
        json!({}),
    ));
}
