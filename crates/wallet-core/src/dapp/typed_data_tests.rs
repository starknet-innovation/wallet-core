// The extension's `tests/starknet/typed-data.test.ts`, `typed-data-revisions.test.ts` and
// `typed-data-value-conformance.test.ts`, plus `describeTypedData`. `H_*` literals are
// starknet.js 10.8's `typedData.getMessageHash` of the `P_*` payload beside them for `ACCOUNT`,
// so each pins Krusty's hash to the extension's.

use super::*;
use crate::js::json;
use crate::kms::NetworkId;
use serde_json::json;

pub(super) const ACCOUNT: &str =
    "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcde";

/// The extension's `JSON_BUDGETS.typedData`.
pub(in crate::dapp) const BUDGET: JsonBudget = JsonBudget {
    max_array_length: 1_024,
    max_bytes: 256 * 1024,
    max_depth: 24,
    max_nodes: 10_000,
    max_object_keys: 512,
    max_string_length: 64 * 1024,
};

pub(super) fn felt(hex: &str) -> Felt {
    Felt::from_hex(hex).unwrap()
}

pub(super) fn account() -> Felt {
    felt(ACCOUNT)
}

pub(super) fn accepts(text: &str) -> bool {
    CheckedTypedData::check(&json::parse(text).unwrap(), &BUDGET).is_ok()
}

pub(super) fn accepts_json(value: &serde_json::Value) -> bool {
    accepts(&value.to_string())
}

/// `typedDataHash`: the gate, then Krusty.
pub(super) fn hash(text: &str) -> Result<Felt, TypedDataRefusal> {
    let value = json::parse(text).unwrap();
    CheckedTypedData::check(&value, &BUDGET)?.message_hash(&account())
}

/// Krusty alone, as the TS tests' `rawHash` is the library alone.
pub(super) fn raw_hash(text: &str) -> Option<Felt> {
    kms::typed_data_message_hash(text, &account()).ok()
}

/// The domain Krusty hashes, `(value, declaration)`: exactly `name`, `version`, `chainId` (and
/// `revision` on revision 1), declared in that order. The extension's fixtures often use a
/// one-field `{ name }` domain, which starknet.js hashes and Krusty refuses; hashed fixtures here
/// use this one instead (see `krusty_refuses_shapes_the_extension_signs`).
pub(super) fn domain(revision_one: bool) -> (serde_json::Value, serde_json::Value) {
    let ty = if revision_one { "shortstring" } else { "felt" };
    let mut fields = vec![
        json!({ "name": "name", "type": ty }),
        json!({ "name": "version", "type": ty }),
        json!({ "name": "chainId", "type": ty }),
    ];
    let mut value = json!({ "chainId": "SN_MAIN", "name": "Bramble Test", "version": "1" });
    if revision_one {
        fields.push(json!({ "name": "revision", "type": ty }));
        value["revision"] = json!("1");
    }
    (value, serde_json::Value::Array(fields))
}

pub(super) fn for_chain(text: &str, network: NetworkId) -> Result<Felt, TypedDataRefusal> {
    let value = json::parse(text).unwrap();
    CheckedTypedData::check(&value, &BUDGET)?
        .message_hash_for_chain(&account(), &network.chain_id())
}

const P_BASIC: &str = r#"{"domain":{"chainId":"SN_MAIN","name":"Bramble Test","revision":"1","version":"1"},"message":{"content":"hello"},"primaryType":"Message","types":{"Message":[{"name":"content","type":"felt"}],"StarknetDomain":[{"name":"name","type":"shortstring"},{"name":"version","type":"shortstring"},{"name":"chainId","type":"shortstring"},{"name":"revision","type":"shortstring"}]}}"#;
const H_BASIC: &str = "0x60e9a763f427418a815ac3cb180e69fd86e0d18b5647060df6567991e07d6ab";

// The numeric felt spelling of `SN_MAIN` under the same `shortstring` chain id.
const P_NUMERIC_CHAIN: &str = r#"{"domain":{"chainId":"0x534e5f4d41494e","name":"Bramble Test","revision":"1","version":"1"},"message":{"content":"hello"},"primaryType":"Message","types":{"Message":[{"name":"content","type":"felt"}],"StarknetDomain":[{"name":"name","type":"shortstring"},{"name":"version","type":"shortstring"},{"name":"chainId","type":"shortstring"},{"name":"revision","type":"shortstring"}]}}"#;
const H_NUMERIC_CHAIN: &str = "0x60e9a763f427418a815ac3cb180e69fd86e0d18b5647060df6567991e07d6ab";

fn basic() -> serde_json::Value {
    serde_json::from_str(P_BASIC).unwrap()
}

// typed-data.test.ts: "validates the SNIP-12 envelope".
#[test]
fn validates_the_snip12_envelope() {
    assert!(accepts(P_BASIC));
    let mut extra = basic();
    extra["extra"] = json!(1);
    assert!(!accepts_json(&extra));
    assert!(!accepts(r#"{"primaryType":"Message"}"#));
    assert!(!accepts(r#""hello""#));
    let mut long = basic();
    long["message"] = json!({ "body": "x".repeat(64 * 1024 + 1) });
    assert!(!accepts_json(&long));
}

// typed-data.test.ts: "hashes valid typed data deterministically".
#[test]
fn hashes_valid_typed_data_deterministically() {
    assert_eq!(hash(P_BASIC), Ok(felt(H_BASIC)));
    assert_eq!(hash(P_BASIC), hash(P_BASIC));
}

// typed-data.test.ts: "rejects unhashable typed data".
#[test]
fn rejects_unhashable_typed_data() {
    let mut empty = basic();
    empty["types"] = json!({});
    assert_eq!(hash(&empty.to_string()), Err(TypedDataRefusal::Invalid));
    assert_eq!(hash("null"), Err(TypedDataRefusal::Invalid));
}

// typed-data.test.ts: "binds dapp signing to the active chain".
#[test]
fn binds_dapp_signing_to_the_active_chain() {
    assert_eq!(for_chain(P_BASIC, NetworkId::Mainnet), Ok(felt(H_BASIC)));
    assert_eq!(
        for_chain(P_BASIC, NetworkId::Sepolia),
        Err(TypedDataRefusal::ChainMismatch)
    );
    // The extension answers every refusal, a foreign chain included, as "invalid typed data".
    let error = WalletRpcError::from(TypedDataRefusal::ChainMismatch);
    assert_eq!(error, WalletRpcError::invalid_request_payload());
}

// typedDataHashForChain's doc: the numeric felt spelling binds the same chain.
#[test]
fn the_numeric_chain_spelling_binds_the_same_chain() {
    assert_eq!(
        for_chain(P_NUMERIC_CHAIN, NetworkId::Mainnet),
        Ok(felt(H_NUMERIC_CHAIN))
    );
    assert_eq!(
        for_chain(P_NUMERIC_CHAIN, NetworkId::Sepolia),
        Err(TypedDataRefusal::ChainMismatch)
    );
    // Any other short string is another chain.
    let mut text = basic();
    text["domain"]["chainId"] = json!("SN MAIN!");
    assert_eq!(
        for_chain(&text.to_string(), NetworkId::Mainnet),
        Err(TypedDataRefusal::ChainMismatch)
    );
}

// typed-data.test.ts: "refuses a typed-data domain with no signed chain id".
#[test]
fn refuses_a_typed_data_domain_with_no_signed_chain_id() {
    let chainless = json!({
        "domain": { "name": "Bramble Test", "revision": "1", "version": "1" },
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
    .to_string();
    assert!(accepts(&chainless));
    assert_eq!(
        for_chain(&chainless, NetworkId::Mainnet),
        Err(TypedDataRefusal::ChainMismatch)
    );
}

// typed-data.test.ts: "rejects unconnected origins and malformed data" (the malformed half).
#[test]
fn rejects_malformed_data() {
    assert!(!accepts(r#"{"nope":1}"#));
}

const ACTIVE_DOMAIN_TYPE: &str = r#"[{"name":"name","type":"shortstring"},{"name":"version","type":"shortstring"},{"name":"chainId","type":"shortstring"}]"#;
const LEGACY_DOMAIN_TYPE: &str = r#"[{"name":"name","type":"felt"},{"name":"version","type":"felt"},{"name":"chainId","type":"felt"}]"#;

/// typed-data-revisions.test.ts's `envelope`: `revision` is JSON text or absent.
fn revision_envelope(active: bool, legacy: bool, revision: Option<&str>) -> String {
    let mut types = String::from(r#""Message":[{"name":"content","type":"felt"}]"#);
    if active {
        types += &format!(r#","StarknetDomain":{ACTIVE_DOMAIN_TYPE}"#);
    }
    if legacy {
        types += &format!(r#","StarkNetDomain":{LEGACY_DOMAIN_TYPE}"#);
    }
    let revision = revision.map_or(String::new(), |r| format!(r#","revision":{r}"#));
    format!(
        r#"{{"domain":{{"chainId":"SN_MAIN","name":"Bramble Test"{revision},"version":"1"}},"message":{{"content":"hello"}},"primaryType":"Message","types":{{{types}}}}}"#
    )
}

// typed-data-revisions.test.ts: "acceptance matches the library case for case".
#[test]
fn acceptance_matches_the_library_case_for_case() {
    let cases = [
        ("revision 1 as a string", true, false, Some(r#""1""#), true),
        ("revision 1 as a number", true, false, Some("1"), true),
        ("revision 1 with no revision key", true, false, None, false),
        (
            "revision 1 spelling with revision 0",
            true,
            false,
            Some(r#""0""#),
            false,
        ),
        ("revision 0 with no revision key", false, true, None, true),
        ("revision 0 as a string", false, true, Some(r#""0""#), true),
        ("revision 0 as a number", false, true, Some("0"), false),
        (
            "revision 0 spelling with revision 1",
            false,
            true,
            Some(r#""1""#),
            false,
        ),
        (
            "both spellings, revision 1",
            true,
            true,
            Some(r#""1""#),
            true,
        ),
        (
            "both spellings, revision 0",
            true,
            true,
            Some(r#""0""#),
            true,
        ),
        ("both spellings, no revision key", true, true, None, true),
        ("no domain type at all", false, false, None, false),
    ];
    for (label, active, legacy, revision, expected) in cases {
        let payload = revision_envelope(active, legacy, revision);
        assert_eq!(accepts(&payload), expected, "{label}");
    }
    let both = revision_envelope(true, true, Some(r#""1""#));
    let value = json::parse(&both).unwrap();
    let checked = CheckedTypedData::check(&value, &BUDGET).unwrap();
    assert_eq!(checked.revision(), Revision::One);
    let legacy = revision_envelope(false, true, None);
    let value = json::parse(&legacy).unwrap();
    let checked = CheckedTypedData::check(&value, &BUDGET).unwrap();
    assert_eq!(checked.revision(), Revision::Zero);
}

const P_REV1_NUMERIC: &str = r#"{"domain":{"chainId":"SN_MAIN","name":"Bramble Test","revision":1,"version":"1"},"message":{"content":"hello"},"primaryType":"Message","types":{"Message":[{"name":"content","type":"felt"}],"StarknetDomain":[{"name":"name","type":"shortstring"},{"name":"version","type":"shortstring"},{"name":"chainId","type":"shortstring"},{"name":"revision","type":"shortstring"}]}}"#;
const P_REV1_STRING: &str = r#"{"domain":{"chainId":"SN_MAIN","name":"Bramble Test","revision":"1","version":"1"},"message":{"content":"hello"},"primaryType":"Message","types":{"Message":[{"name":"content","type":"felt"}],"StarknetDomain":[{"name":"name","type":"shortstring"},{"name":"version","type":"shortstring"},{"name":"chainId","type":"shortstring"},{"name":"revision","type":"shortstring"}]}}"#;
const H_REV1_NUMERIC: &str = "0x60e9a763f427418a815ac3cb180e69fd86e0d18b5647060df6567991e07d6ab";

// typed-data-revisions.test.ts: "a numeric revision 1 hashes identically to the string form".
#[test]
fn a_numeric_revision_1_hashes_identically_to_the_string_form() {
    // The test's envelope declares no `revision` in the domain type, which Krusty refuses; these
    // two declare it.
    assert_eq!(hash(P_REV1_NUMERIC), hash(P_REV1_STRING));
    assert_eq!(hash(P_REV1_NUMERIC), Ok(felt(H_REV1_NUMERIC)));
}

// typed-data-revisions.test.ts: "a revision that is neither identifies nothing".
#[test]
fn a_revision_that_is_neither_identifies_nothing() {
    for revision in ["[1]", r#""01""#, r#""1.0""#, "true", "null", r#""2""#] {
        assert!(
            !accepts(&revision_envelope(true, true, Some(revision))),
            "{revision}"
        );
    }
}

/// A revision 0 envelope (`legacy` in typed-data-revisions.test.ts).
fn legacy(types: &str, message: &str, primary: &str) -> String {
    format!(
        r#"{{"domain":{{"chainId":"SN_MAIN","name":"Bramble Test","version":"1"}},"message":{message},"primaryType":"{primary}","types":{{"StarkNetDomain":{LEGACY_DOMAIN_TYPE},{types}}}}}"#
    )
}

const P_REV0_ORDER: &str = r#"{"domain":{"chainId":"SN_MAIN","name":"Bramble Test","version":"1"},"message":{"details":{"amount":"0x3","token":"0x2"},"extras":[{"amount":"0x5","token":"0x4"}],"maker":"0x1","memo":"hello"},"primaryType":"Order","types":{"StarkNetDomain":[{"name":"name","type":"felt"},{"name":"version","type":"felt"},{"name":"chainId","type":"felt"}],"Details":[{"name":"token","type":"felt"},{"name":"amount","type":"felt"}],"Order":[{"name":"maker","type":"felt"},{"name":"memo","type":"shortstring"},{"name":"details","type":"Details"},{"name":"extras","type":"Details*"}]}}"#;
const H_REV0_ORDER: &str = "0x731029357c7fbe6fc4bfd862c1c4d2d25e51e14869ea14056fccb0e8a9e0082";

const P_REV0_MERKLE: &str = r#"{"domain":{"chainId":"SN_MAIN","name":"Bramble Test","version":"1"},"message":{"tree":[{"leaf":"0x1"},{"leaf":"0x2"}]},"primaryType":"Message","types":{"StarkNetDomain":[{"name":"name","type":"felt"},{"name":"version","type":"felt"},{"name":"chainId","type":"felt"}],"Inner":[{"name":"leaf","type":"felt"}],"Message":[{"name":"tree","contains":"Inner","type":"merkletree"}]}}"#;
const H_REV0_MERKLE: &str = "0x93feb38db510b70e5a8a5af2a5081aea1d2dade0cf25bf553d0688443bfbbd";

const P_REV0_U256: &str = r#"{"domain":{"chainId":"SN_MAIN","name":"Bramble Test","version":"1"},"message":{"amount":{"high":"0x0","low":"0x64"}},"primaryType":"Message","types":{"StarkNetDomain":[{"name":"name","type":"felt"},{"name":"version","type":"felt"},{"name":"chainId","type":"felt"}],"u256":[{"name":"low","type":"felt"},{"name":"high","type":"felt"}],"Message":[{"name":"amount","type":"u256"}]}}"#;
const H_REV0_U256: &str = "0x78e46e260156bff8302724f1d76a812dfae62e235ab3151be806d4cdee6ea00";

// typed-data-revisions.test.ts: "a benign revision 0 payload signs, and matches the library
// hash" and "a merkletree still resolves, because revision 0 encodes its leaves".
#[test]
fn a_benign_revision_0_payload_signs_and_matches_the_library_hash() {
    assert_eq!(hash(P_REV0_ORDER), Ok(felt(H_REV0_ORDER)));
    assert_eq!(hash(P_REV0_MERKLE), Ok(felt(H_REV0_MERKLE)));
}

// typed-data-revisions.test.ts: "constructs revision 0 cannot encode are refused, not coerced to
// a felt".
#[test]
fn constructs_revision_0_cannot_encode_are_refused() {
    let refused = [
        r#""Message":[{"name":"amount","type":"u256"}]"#,
        r#""Message":[{"name":"asset","type":"TokenAmount"}]"#,
        r#""Message":[{"name":"nft","type":"NftId"}]"#,
        r#""Choice":[{"name":"Some","type":"(felt)"}],"Message":[{"name":"c","contains":"Choice","type":"enum"}]"#,
        r#""Message":[{"name":"c","type":"(felt,felt)"}]"#,
        r#""Message":[{"name":"c","type":"Ghost"}]"#,
    ];
    for types in refused {
        assert!(
            !accepts(&legacy(types, r#"{"amount":"0x1"}"#, "Message")),
            "{types}"
        );
    }
}

// typed-data-revisions.test.ts: "a preset name is an ordinary struct on revision 0 and reserved
// on revision 1".
#[test]
fn a_preset_name_is_an_ordinary_struct_on_revision_0_and_reserved_on_revision_1() {
    // Deviation: starknet.js hashes the declaration (`H_REV0_U256`); Krusty hashes something else,
    // so a signature would not verify against the dapp's hash. The gate refuses it instead.
    // starknet-innovation/krusty-kms#275.
    assert!(raw_hash(P_REV0_U256).is_some_and(|h| h != felt(H_REV0_U256)));
    assert_eq!(hash(P_REV0_U256), Err(TypedDataRefusal::Invalid));
    let rev1 = format!(
        r#"{{"domain":{{"chainId":"SN_MAIN","name":"Bramble Test","revision":"1","version":"1"}},"message":{{"amount":{{"high":"0x0","low":"0x64"}}}},"primaryType":"Message","types":{{"u256":[{{"name":"low","type":"felt"}},{{"name":"high","type":"felt"}}],"Message":[{{"name":"amount","type":"u256"}}],"StarknetDomain":{ACTIVE_DOMAIN_TYPE}}}}}"#
    );
    assert!(!accepts(&rev1));
}

// typed-data-revisions.test.ts: "an undeclared revision 0 domain key is refused, as on
// revision 1".
#[test]
fn an_undeclared_revision_0_domain_key_is_refused() {
    let trimmed = |chain: &str, version: &str| {
        format!(
            r#"{{"domain":{{"chainId":"{chain}","name":"Bramble Test","version":"{version}"}},"message":{{"content":"0x1"}},"primaryType":"Message","types":{{"Message":[{{"name":"content","type":"felt"}}],"StarkNetDomain":[{{"name":"name","type":"felt"}}]}}}}"#
        )
    };
    // starknet.js hashes the two alike, since neither key reaches the domain hash; Krusty refuses
    // any domain type but its own three fields.
    assert_eq!(raw_hash(&trimmed("SN_MAIN", "1")), None);
    assert!(!accepts(&trimmed("SN_MAIN", "1")));
}

#[test]
fn describes_the_request_for_the_card() {
    let describe = |text: &str| describe_typed_data(&json::parse(text).unwrap());
    assert_eq!(describe(P_BASIC), "Message · Bramble Test");
    assert_eq!(describe(r#"{"primaryType":"Mail"}"#), "Mail");
    assert_eq!(describe(r#"{"domain":{"name":"App"}}"#), "message · App");
    assert_eq!(
        describe(r#"{"primaryType":"Mail","domain":{"name":""}}"#),
        "Mail"
    );
    assert_eq!(describe(r#"{"primaryType":7,"domain":[]}"#), "message");
    assert_eq!(describe("[]"), "message");
    assert_eq!(describe(r#""hello""#), "typed data");
    assert_eq!(describe("null"), "typed data");
    // Each part is cut to 64 code points, an astral character counting once.
    let long = "😀".repeat(70);
    let text = json!({ "primaryType": "a".repeat(65), "domain": { "name": long } }).to_string();
    assert_eq!(
        describe(&text),
        format!("{}… · {}…", "a".repeat(64), "😀".repeat(64))
    );
}

// Shapes the extension accepts and starknet.js hashes that Krusty refuses: the gate passes them,
// hashing fails closed, and the page sees 114 where the extension would have signed.
// starknet-innovation/krusty-kms#276: when Krusty hashes these as starknet.js does, flip each
// case to the starknet.js digest.
#[test]
fn krusty_refuses_shapes_the_extension_signs() {
    let (rev1_domain, rev1_type) = domain(true);
    let with = |domain: serde_json::Value,
                domain_type: serde_json::Value,
                message: serde_json::Value,
                extra: serde_json::Value| {
        let mut types =
            json!({ "Msg": [{ "name": "a", "type": "felt" }], "StarknetDomain": domain_type });
        if let Some(entries) = extra.as_object() {
            for (k, v) in entries {
                types[k] = v.clone();
            }
        }
        json!({ "domain": domain, "message": message, "primaryType": "Msg", "types": types })
            .to_string()
    };
    let field = |name: &str| json!({ "name": name, "type": "shortstring" });
    let ok = json!({ "a": "0x1" });
    let cases = [
        // The baseline Krusty hashes.
        (
            with(
                rev1_domain.clone(),
                rev1_type.clone(),
                ok.clone(),
                json!({}),
            ),
            true,
        ),
        // Domain fields declared in another order.
        (
            with(
                rev1_domain.clone(),
                json!([
                    field("name"),
                    field("chainId"),
                    field("version"),
                    field("revision")
                ]),
                ok.clone(),
                json!({}),
            ),
            false,
        ),
        // A revision 1 domain type that leaves `revision` undeclared.
        (
            with(
                rev1_domain.clone(),
                json!([field("name"), field("version"), field("chainId")]),
                ok.clone(),
                json!({}),
            ),
            false,
        ),
        // A one-field domain.
        (
            with(
                json!({ "name": "App", "revision": "1" }),
                json!([field("name")]),
                ok.clone(),
                json!({}),
            ),
            false,
        ),
        // An undeclared message key.
        (
            with(
                rev1_domain.clone(),
                rev1_type.clone(),
                json!({ "a": "0x1", "note": "x" }),
                json!({}),
            ),
            false,
        ),
        // A declared struct with no fields.
        (
            with(
                rev1_domain.clone(),
                rev1_type.clone(),
                ok.clone(),
                json!({ "Unused": [] }),
            ),
            false,
        ),
        // The back-compat legacy domain declared beside the revision 1 one.
        (
            with(
                rev1_domain,
                rev1_type,
                ok,
                json!({ "StarkNetDomain": [{ "name": "name", "type": "felt" }] }),
            ),
            false,
        ),
    ];
    for (payload, hashes) in cases {
        assert!(accepts(&payload), "{payload}");
        let expected = if hashes {
            Ok(())
        } else {
            Err(TypedDataRefusal::Unhashable)
        };
        assert_eq!(hash(&payload).map(|_| ()), expected, "{payload}");
    }
}

#[path = "typed_data_value_tests.rs"]
mod value_tests;
