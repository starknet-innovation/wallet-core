// The extension's `tests/starknet/typed-data-graph-forgery.test.ts` (MCW-V09-101): a name
// carrying an `encodeType` separator lets two type graphs reach one hash. `H_*` literals are
// starknet.js 10.8's hash of the `P_*` payload beside them; `raw_hash` is Krusty unguarded.

use super::tests::{accepts, accepts_json, felt, hash, raw_hash};
use super::TypedDataRefusal;
use serde_json::{json, Value};

const SEPARATORS: [&str; 9] = ["\"", "(", ")", ",", ":", "\\", "\n", "\u{202e}", "é"];

const P_REV0_VICTIM: &str = r#"{"domain":{"chainId":"SN_MAIN","name":"Bramble Test","version":"1"},"message":{"details":{"amount":"0x3","token":"0x2"},"maker":"0x1"},"primaryType":"Order","types":{"Details":[{"name":"token","type":"felt"},{"name":"amount","type":"felt"}],"Order":[{"name":"maker","type":"felt"},{"name":"details","type":"Details"}],"StarkNetDomain":[{"name":"name","type":"felt"},{"name":"version","type":"felt"},{"name":"chainId","type":"felt"}]}}"#;
const H_REV0_VICTIM: &str = "0x5327e5d2f5794d5ae2ab9231b4b5f69da6c854a085b6c86409abe7281362dd5";
// `details` is the victim's `Details` struct hash (starknet.js `getStructHash`, revision 0).
const P_REV0_ATTACKER: &str = r#"{"domain":{"chainId":"SN_MAIN","name":"Bramble Test","version":"1"},"message":{"details":"0x15f4d4b198334822adf1bc8bbd4cd5ad7be88de61287e39f67c6b4d0cea87b6","maker":"0x1"},"primaryType":"Order","types":{"Order":[{"name":"maker","type":"felt"},{"name":"details","type":"Details)Details(token:felt,amount:felt"}],"StarkNetDomain":[{"name":"name","type":"felt"},{"name":"version","type":"felt"},{"name":"chainId","type":"felt"}]}}"#;

// "the two revision 0 graphs really do collide in the pinned library" and "the honest graph
// still signs and the forged one is refused".
#[test]
fn revision_0_type_name_injection_is_refused() {
    // In starknet.js the two hash identically; Krusty refuses the forged half outright.
    assert_eq!(raw_hash(P_REV0_ATTACKER), None);
    assert_eq!(hash(P_REV0_VICTIM), Ok(felt(H_REV0_VICTIM)));
    assert_eq!(hash(P_REV0_ATTACKER), Err(TypedDataRefusal::Invalid));
}

fn with_types(revision_one: bool, mut types: Value) -> Value {
    let domain = if revision_one {
        types["StarknetDomain"] = rev1_domain_type();
        rev1_domain()
    } else {
        types["StarkNetDomain"] = json!([
            { "name": "name", "type": "felt" },
            { "name": "version", "type": "felt" },
            { "name": "chainId", "type": "felt" }
        ]);
        json!({ "chainId": "SN_MAIN", "name": "Bramble Test", "version": "1" })
    };
    json!({
        "domain": domain,
        "message": { "content": "hello" },
        "primaryType": "Message",
        "types": types
    })
}

fn rev1_domain() -> Value {
    json!({ "chainId": "SN_MAIN", "name": "Bramble Test", "revision": "1", "version": "1" })
}

fn rev1_domain_type() -> Value {
    json!([
        { "name": "name", "type": "shortstring" },
        { "name": "version", "type": "shortstring" },
        { "name": "chainId", "type": "shortstring" },
        { "name": "revision", "type": "shortstring" }
    ])
}

// "every separator is refused in a revision 0 type name, field name, and field type" and its
// revision 1 twin.
#[test]
fn every_separator_is_refused_in_a_type_name_field_name_and_field_type() {
    for revision_one in [false, true] {
        assert!(accepts_json(&with_types(
            revision_one,
            json!({ "Message": [{ "name": "content", "type": "felt" }] })
        )));
        for separator in SEPARATORS {
            let field_name =
                json!({ "Message": [{ "name": format!("content{separator}"), "type": "felt" }] });
            let field_type =
                json!({ "Message": [{ "name": "content", "type": format!("felt{separator}") }] });
            let inner = format!("Inner{separator}");
            let mut type_name =
                json!({ "Message": [{ "name": "content", "type": inner.clone() }] });
            type_name[inner] = json!([{ "name": "leaf", "type": "felt" }]);
            for types in [field_name, field_type, type_name] {
                assert!(
                    !accepts_json(&with_types(revision_one, types)),
                    "{separator:?}"
                );
            }
        }
    }
}

const P_REV1_VICTIM: &str = r#"{"domain":{"chainId":"SN_MAIN","name":"Bramble Test","revision":"1","version":"1"},"message":{"details":{"amount":"0x3","token":"0x2"},"maker":"0x1"},"primaryType":"Order","types":{"Details":[{"name":"token","type":"felt"},{"name":"amount","type":"felt"}],"Order":[{"name":"maker","type":"felt"},{"name":"details","type":"Details"}],"StarknetDomain":[{"name":"name","type":"shortstring"},{"name":"version","type":"shortstring"},{"name":"chainId","type":"shortstring"},{"name":"revision","type":"shortstring"}]}}"#;
const H_REV1_VICTIM: &str = "0x63efeda74205937d1673469d9d93786cbb13f6482a2ae0b43889c7035303391";
const P_REV1_ATTACKER: &str = r#"{"domain":{"chainId":"SN_MAIN","name":"Bramble Test","revision":"1","version":"1"},"message":{"maker":"0x1","details\":\"Details\")\"Details\"(\"token\":\"felt\",\"amount":"0x68cf273dffeb0f44c5ba1cd6917b383ad732065eaeb32d73ffc2036cca0b3bd"},"primaryType":"Order","types":{"Order":[{"name":"maker","type":"felt"},{"name":"details\":\"Details\")\"Details\"(\"token\":\"felt\",\"amount","type":"felt"}],"StarknetDomain":[{"name":"name","type":"shortstring"},{"name":"version","type":"shortstring"},{"name":"chainId","type":"shortstring"},{"name":"revision","type":"shortstring"}]}}"#;

// "the revision 1 field-name forgery really does collide too" and "the honest graph still signs
// and the forged one is refused".
#[test]
fn revision_1_delimiter_injection_is_refused() {
    // starknet.js hashes the two alike. Krusty's encoder does not: it hashes the forged half to
    // another value, so the gate is the only defence in starknet.js and a second one here.
    assert!(raw_hash(P_REV1_ATTACKER).is_some());
    assert_ne!(raw_hash(P_REV1_ATTACKER), raw_hash(P_REV1_VICTIM));
    assert_eq!(hash(P_REV1_VICTIM), Ok(felt(H_REV1_VICTIM)));
    assert_eq!(hash(P_REV1_ATTACKER), Err(TypedDataRefusal::Invalid));
}

// "the array suffix is accepted only as a single trailing star".
#[test]
fn the_array_suffix_is_accepted_only_as_a_single_trailing_star() {
    let with_field = |field: Value| {
        accepts_json(&json!({
            "domain": rev1_domain(),
            "message": { "content": ["0x1", "0x2"] },
            "primaryType": "Message",
            "types": { "Message": [field], "StarknetDomain": rev1_domain_type() }
        }))
    };
    assert!(with_field(json!({ "name": "content", "type": "felt*" })));
    for ty in ["felt**", "*felt", "*", "fe*lt"] {
        assert!(
            !with_field(json!({ "name": "content", "type": ty })),
            "{ty}"
        );
    }
    assert!(!with_field(
        json!({ "name": "content", "contains": "Message", "type": "enum*" })
    ));
}

fn resolves(types: Value) -> bool {
    accepts_json(&with_types(true, types))
}

const P_RICH: &str = r#"{"domain":{"chainId":"SN_MAIN","name":"Bramble Test","revision":"1","version":"1"},"message":{"amount":{"high":"0x0","low":"0x64"},"choice":{"Some":["0x1","0x2"]},"items":[{"leaf":"0x1"},{"leaf":"0x2"}],"who":"0x5"},"primaryType":"Order","types":{"Choice":[{"name":"Nothing","type":"()"},{"name":"Some","type":"(felt,u128)"}],"Inner":[{"name":"leaf","type":"felt"}],"Order":[{"name":"amount","type":"u256"},{"name":"who","type":"ContractAddress"},{"name":"items","type":"Inner*"},{"name":"choice","contains":"Choice","type":"enum"}],"StarknetDomain":[{"name":"name","type":"shortstring"},{"name":"version","type":"shortstring"},{"name":"chainId","type":"shortstring"},{"name":"revision","type":"shortstring"}]}}"#;
const H_RICH: &str = "0x568d40a77949f45e91730dc39005e8031a00f6d443ccee847a44f72c735d796";

const P_SPACES: &str = r#"{"domain":{"chainId":"SN_MAIN","name":"Bramble Test","revision":"1","version":"1"},"message":{"Allowed Method":"transfer","Session Key":"0x9"},"primaryType":"Session","types":{"Session":[{"name":"Allowed Method","type":"shortstring"},{"name":"Session Key","type":"felt"}],"StarknetDomain":[{"name":"name","type":"shortstring"},{"name":"version","type":"shortstring"},{"name":"chainId","type":"shortstring"},{"name":"revision","type":"shortstring"}]}}"#;
const H_SPACES: &str = "0x25cffd08efb110df7cdacca489a929a5c7f1b24792ba13633107f276af095c";

// "accepts the full revision 1 vocabulary", "a rich revision 1 payload still hashes, so the
// guard is not over-tight" and "a field name containing spaces is still accepted".
#[test]
fn accepts_the_full_revision_1_vocabulary() {
    let u256 = json!({ "high": "0x0", "low": "0x1" });
    assert!(accepts_json(&json!({
        "domain": rev1_domain(),
        "message": {
            "amount": u256,
            "asset": { "amount": u256, "token_address": "0x2" },
            "choice": { "Nothing": [] },
            "content": "0x1",
            "many": [{ "leaf": "0x5" }],
            "nft": { "collection_address": "0x3", "token_id": u256 },
            "tree": [{ "leaf": "0x6" }],
            "who": "0x4"
        },
        "primaryType": "Message",
        "types": {
            "StarknetDomain": rev1_domain_type(),
            "Inner": [{ "name": "leaf", "type": "felt" }],
            "Message": [
                { "name": "content", "type": "felt" },
                { "name": "amount", "type": "u256" },
                { "name": "asset", "type": "TokenAmount" },
                { "name": "nft", "type": "NftId" },
                { "name": "who", "type": "ContractAddress" },
                { "name": "many", "type": "Inner*" },
                { "name": "choice", "contains": "Choice", "type": "enum" },
                { "name": "tree", "contains": "Inner", "type": "merkletree" }
            ],
            "Choice": [
                { "name": "Nothing", "type": "()" },
                { "name": "Some", "type": "(felt,u128)" }
            ]
        }
    })));
    assert_eq!(hash(P_RICH), Ok(felt(H_RICH)));
    assert_eq!(hash(P_SPACES), Ok(felt(H_SPACES)));
}

// "rejects an unresolvable field type instead of coercing it to a felt" and "rejects a
// container field whose contains does not resolve".
#[test]
fn rejects_unresolvable_types_and_contains() {
    for field in [
        json!({ "name": "content", "type": "Ghost" }),
        json!({ "name": "content", "type": "Ghost*" }),
        json!({ "name": "c", "type": "(felt,Ghost)" }),
        json!({ "name": "c", "contains": "Ghost", "type": "enum" }),
        json!({ "name": "c", "contains": "Message", "type": "felt" }),
        json!({ "name": "c", "contains": "felt", "type": "enum" }),
        json!({ "name": "c", "contains": "u256", "type": "enum" }),
    ] {
        assert!(!resolves(json!({ "Message": [field.clone()] })), "{field}");
    }
}

// "rejects redeclaring a reserved or preset type name".
#[test]
fn rejects_redeclaring_a_reserved_or_preset_type_name() {
    for reserved in ["u256", "TokenAmount", "NftId", "felt", "enum", "merkletree"] {
        let mut types = json!({ "Message": [{ "name": "content", "type": "felt" }] });
        types[reserved] = json!([]);
        assert!(!resolves(types), "{reserved}");
    }
}

const P_WITH_LEGACY: &str = r#"{"domain":{"chainId":"SN_MAIN","name":"Bramble Test","revision":"1","version":"1"},"message":{"content":"hello"},"primaryType":"Message","types":{"Message":[{"name":"content","type":"felt"}],"StarkNetDomain":[{"name":"name","type":"felt"}],"StarknetDomain":[{"name":"name","type":"shortstring"},{"name":"version","type":"shortstring"},{"name":"chainId","type":"shortstring"},{"name":"revision","type":"shortstring"}]}}"#;
const P_WITHOUT_LEGACY: &str = r#"{"domain":{"chainId":"SN_MAIN","name":"Bramble Test","revision":"1","version":"1"},"message":{"content":"hello"},"primaryType":"Message","types":{"Message":[{"name":"content","type":"felt"}],"StarknetDomain":[{"name":"name","type":"shortstring"},{"name":"version","type":"shortstring"},{"name":"chainId","type":"shortstring"},{"name":"revision","type":"shortstring"}]}}"#;
const H_WITHOUT_LEGACY: &str = "0x60e9a763f427418a815ac3cb180e69fd86e0d18b5647060df6567991e07d6ab";

// "the legacy domain name is an ordinary struct, so back-compat payloads pass".
#[test]
fn the_legacy_domain_name_is_an_ordinary_struct() {
    assert!(accepts(P_WITH_LEGACY));
    // starknet.js hashes it as `P_WITHOUT_LEGACY`; Krusty reads the domain type from
    // `StarkNetDomain` whenever it is declared, and refuses.
    assert_eq!(hash(P_WITH_LEGACY), Err(TypedDataRefusal::Unhashable));
    assert_eq!(hash(P_WITHOUT_LEGACY), Ok(felt(H_WITHOUT_LEGACY)));
}

// "a merkletree leaf must be a struct, not a raw felt" (the hashes are in
// `typed_data_container_tests`).
#[test]
fn a_merkletree_leaf_must_be_a_struct_not_a_raw_felt() {
    let tree = |contains: &str, leaves: Value| {
        accepts_json(&json!({
            "domain": rev1_domain(),
            "message": { "tree": leaves },
            "primaryType": "Message",
            "types": {
                "Inner": [{ "name": "leaf", "type": "felt" }],
                "Message": [{ "name": "tree", "contains": contains, "type": "merkletree" }],
                "StarknetDomain": rev1_domain_type()
            }
        }))
    };
    assert!(tree(
        "u256",
        json!([{ "high": "0x0", "low": "0x1" }, { "high": "0x0", "low": "0x2" }])
    ));
    assert!(tree("Inner", json!([{ "leaf": "0x1" }, { "leaf": "0x2" }])));
    assert!(!tree("felt", json!(["0x1", "0x2", "0x3"])));
    assert!(!tree("Inner*", json!([[{ "leaf": "0x1" }]])));
    assert!(!tree("Ghost", json!(["0x1"])));
    assert!(!tree("enum", json!(["0x1"])));
}

// "rejects an undeclared primary type and duplicate field names" and "rejects a malformed field
// entry".
#[test]
fn rejects_undeclared_primary_duplicates_and_malformed_fields() {
    let mut missing = with_types(true, json!({ "Message": [] }));
    missing["primaryType"] = json!("Missing");
    assert!(!accepts_json(&missing));
    assert!(!resolves(json!({ "Message": [
        { "name": "content", "type": "felt" },
        { "name": "content", "type": "u128" }
    ] })));
    for message in [
        json!([{ "name": "content" }]),
        json!([{ "type": "felt" }]),
        json!([{ "extra": 1, "name": "content", "type": "felt" }]),
        json!("not-a-list"),
    ] {
        assert!(!resolves(json!({ "Message": message })));
    }
}

// "an undeclared domain key is refused rather than silently unhashed".
#[test]
fn an_undeclared_domain_key_is_refused() {
    let payload = |chain: &str| {
        json!({
            "domain": { "chainId": chain, "name": "Bramble Test", "revision": "1", "version": "1" },
            "message": { "content": "hello" },
            "primaryType": "Message",
            "types": {
                "Message": [{ "name": "content", "type": "felt" }],
                "StarknetDomain": [
                    { "name": "name", "type": "shortstring" },
                    { "name": "version", "type": "shortstring" },
                    { "name": "revision", "type": "shortstring" }
                ]
            }
        })
        .to_string()
    };
    assert_eq!(
        raw_hash(&payload("SN_MAIN")),
        raw_hash(&payload("SN_SEPOLIA"))
    );
    assert!(!accepts(&payload("SN_MAIN")));
    assert_eq!(hash(&payload("SN_SEPOLIA")), Err(TypedDataRefusal::Invalid));
}

// "a domain type may omit revision, since revision selects the hash method" and "a missing
// StarknetDomain declaration is refused".
#[test]
fn the_domain_type_may_omit_revision_but_must_be_declared() {
    assert!(accepts_json(&json!({
        "domain": { "name": "Bramble Test", "revision": "1", "version": "1" },
        "message": { "content": "hello" },
        "primaryType": "Message",
        "types": {
            "Message": [{ "name": "content", "type": "felt" }],
            "StarknetDomain": [
                { "name": "name", "type": "shortstring" },
                { "name": "version", "type": "shortstring" }
            ]
        }
    })));
    assert!(!accepts_json(&json!({
        "domain": rev1_domain(),
        "message": { "content": "hello" },
        "primaryType": "Message",
        "types": { "Message": [{ "name": "content", "type": "felt" }] }
    })));
}
