//! P7: the extension's v13 validators, case by case, on the collections the reader used to
//! refuse. Each case is the extension's own test (file named beside it, under
//! `packages/platform/vault/tests/` at `a01b240c`), moved onto `vault_v13_support::full()`: a
//! vault the extension accepts with the change and the core does too, or one both refuse. The
//! account records are `vault_v13_accounts.rs`.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

#[path = "vault_v13_support/mod.rs"]
mod support;

use serde_json::{json, Value};
use support::{check, full, opens, text, Change, ORIGIN, SEPOLIA_CHAIN};

#[test]
fn origin_grants() {
    // `schema.test.ts`: "rejects duplicate origin grants and duplicate grant accounts",
    // "rejects persisted grants for insecure origins"; `isOriginGrant`'s references.
    let main = text(&full()["accounts"][0]["id"]);
    let grant = |ids: Value, origin: &str| json!([{ "accountIds": ids, "origin": origin }]);
    let set = |grants: Value| -> Change {
        Box::new(move |p: &mut Value| p["originGrants"] = grants.clone())
    };
    check(vec![
        (
            "loopback http on a port",
            true,
            set(grant(json!([main]), "http://127.0.0.1:5173")),
        ),
        ("no accounts", false, set(grant(json!([]), ORIGIN))),
        (
            "a repeated origin",
            false,
            set(json!([
                { "accountIds": [main], "origin": ORIGIN },
                { "accountIds": [main], "origin": ORIGIN },
            ])),
        ),
        (
            "a repeated account",
            false,
            set(grant(json!([main, main]), ORIGIN)),
        ),
        (
            "an insecure origin",
            false,
            set(grant(json!([main]), "http://example.com")),
        ),
        (
            "an origin with a path",
            false,
            set(grant(json!([main]), "https://app.example/x")),
        ),
        (
            "an origin with a slash",
            false,
            set(grant(json!([main]), "https://app.example/")),
        ),
        (
            "an uppercase host",
            false,
            set(grant(json!([main]), "https://App.example")),
        ),
        (
            "a default port",
            false,
            set(grant(json!([main]), "https://app.example:443")),
        ),
        (
            "an unknown account",
            false,
            set(grant(json!(["SN_MAIN:0x999"]), ORIGIN)),
        ),
        (
            "a non-canonical id",
            false,
            set(grant(json!([main.replace(":0x", ":0x0")]), ORIGIN)),
        ),
    ]);
}

#[test]
fn imported_seeds() {
    // `schema.test.ts`: "binds every imported-seed reference or rejects the vault".
    let phrase = support::fixture()["mnemonic"].clone();
    let seeds = |seeds: Value| -> Change {
        Box::new(move |p: &mut Value| p["importedSeeds"] = seeds.clone())
    };
    check(vec![
        (
            "an unreferenced phrase beside the bound one",
            true,
            seeds(
                json!([{ "id": "phrase-1", "mnemonic": phrase }, { "id": "phrase-9999", "mnemonic": "x" }]),
            ),
        ),
        ("a dangling handle", false, seeds(json!([]))),
        (
            "a handle outside the grammar",
            false,
            seeds(json!([{ "id": "anything", "mnemonic": phrase }])),
        ),
        (
            "phrase-0",
            false,
            seeds(json!([{ "id": "phrase-0", "mnemonic": phrase }])),
        ),
        (
            "a repeated handle",
            false,
            seeds(
                json!([{ "id": "phrase-1", "mnemonic": phrase }, { "id": "phrase-1", "mnemonic": phrase }]),
            ),
        ),
        (
            "an empty phrase",
            false,
            seeds(json!([{ "id": "phrase-1", "mnemonic": "" }])),
        ),
        (
            "a blank phrase",
            false,
            seeds(json!([{ "id": "phrase-1", "mnemonic": " \u{a0}" }])),
        ),
        (
            "a phrase past 512 units",
            false,
            seeds(json!([{ "id": "phrase-1", "mnemonic": "a".repeat(513) }])),
        ),
        (
            "an unknown key",
            false,
            seeds(json!([{ "id": "phrase-1", "mnemonic": phrase, "x": 1 }])),
        ),
    ]);
}

#[test]
fn shadow_account_scopes() {
    // `schema-v12.test.ts`: every "invalid: … fails the whole vault closed" case, and "a
    // duplicated scope or a collection past the bound is refused".
    let scope = full()["shadowAccountScopes"][0].clone();
    let one = |field: &str, value: Value| -> Change {
        let mut record = scope.clone();
        record[field] = value;
        Box::new(move |p: &mut Value| p["shadowAccountScopes"] = json!([record]))
    };
    let many = |count: usize| -> Change {
        let records: Vec<Value> = (0..count)
            .map(|n| {
                let mut record = scope.clone();
                record["label"] = format!("Site {n}").into();
                record
            })
            .collect();
        Box::new(move |p: &mut Value| p["shadowAccountScopes"] = json!(records))
    };
    let pair = |second: Value| -> Change {
        let records = json!([scope, second]);
        Box::new(move |p: &mut Value| p["shadowAccountScopes"] = records.clone())
    };
    let mut other = scope.clone();
    other["label"] = "Other".into();
    other["nonces"] = json!([]);
    let mut repeated = scope.clone();
    repeated["nonces"] = json!([]);
    let too_many: Vec<String> = (0..65).map(|n| format!("0x{n:x}")).collect();
    let mut extra = scope.clone();
    extra["extra"] = true.into();
    let mut missing = scope.clone();
    missing.as_object_mut().unwrap().remove("nonces");
    check(vec![
        ("another label", true, pair(other)),
        ("128 scopes", true, many(128)),
        ("129 scopes", false, many(129)),
        ("a duplicated scope", false, pair(repeated)),
        (
            "a dangling account",
            false,
            one("accountId", "SN_MAIN:0x999".into()),
        ),
        (
            "another network's chain",
            false,
            one("chainId", SEPOLIA_CHAIN.into()),
        ),
        (
            "a non-canonical anonymizer",
            false,
            one("anonymizer", "0x04f33230".into()),
        ),
        ("a zero anonymizer", false, one("anonymizer", "0x0".into())),
        (
            "a non-canonical pool",
            false,
            one("poolAddress", "0x0040337b".into()),
        ),
        ("an uppercase nonce", false, one("nonces", json!(["0xA"]))),
        (
            "unsorted nonces",
            false,
            one("nonces", json!(["0x2", "0x0"])),
        ),
        (
            "a repeated nonce",
            false,
            one("nonces", json!(["0x2", "0x2"])),
        ),
        (
            "a nonce outside the field",
            false,
            one("nonces", json!([format!("0x{}", "f".repeat(64))])),
        ),
        (
            "an invalid origin",
            false,
            one("origin", "http://app.example".into()),
        ),
        (
            "an origin with a path",
            false,
            one("origin", "https://app.example/x".into()),
        ),
        ("an empty label", false, one("label", "".into())),
        ("a non-ASCII label", false, one("label", "Märkt".into())),
        (
            "a label past 66 characters",
            false,
            one("label", "x".repeat(67).into()),
        ),
        ("too many nonces", false, one("nonces", json!(too_many))),
        (
            "an unknown key",
            false,
            Box::new(move |p: &mut Value| p["shadowAccountScopes"] = json!([extra])),
        ),
        (
            "a missing key",
            false,
            Box::new(move |p: &mut Value| p["shadowAccountScopes"] = json!([missing])),
        ),
        (
            "not a list",
            false,
            Box::new(|p: &mut Value| p["shadowAccountScopes"] = "none".into()),
        ),
    ]);
}

#[test]
fn multisig_viewing_keys() {
    // `schema-v13.test.ts`: "a canonical key validates, and a conflict requires that key", "a
    // bad key, a foreign chain, or a conflict without a key fails closed";
    // `multisig-strk20-key.test.ts`: the collection limit and the multisig's own chain.
    let key = full()["multisigStrk20ViewingKeys"][0].clone();
    let conflict = full()["multisigStrk20KeyConflicts"][0].clone();
    let set = |keys: Value, conflicts: Value| -> Change {
        Box::new(move |p: &mut Value| {
            p["multisigStrk20ViewingKeys"] = keys.clone();
            p["multisigStrk20KeyConflicts"] = conflicts.clone();
        })
    };
    let changed = |field: &str, value: &str| {
        let mut key = key.clone();
        key[field] = value.into();
        key
    };
    // Both lists sort by scope id (`multisigId:chainId:poolAddress`) as JavaScript compares
    // strings, which for these ASCII ids is byte order.
    let scope_id = |key: &Value| {
        format!(
            "{}:{}:{}",
            text(&key["multisigId"]),
            text(&key["chainId"]),
            text(&key["poolAddress"])
        )
    };
    let keys = |count: usize| -> Vec<Value> {
        let mut keys: Vec<Value> = (1..=count)
            .map(|n| changed("multisigId", &format!("SN_MAIN:0x{n:x}{}", "0".repeat(8))))
            .collect();
        keys.sort_by_key(scope_id);
        keys
    };
    let (sorted, over) = (keys(32), keys(33));
    let mut unsorted = vec![
        changed("multisigId", "SN_MAIN:0x2"),
        changed("multisigId", "SN_MAIN:0x1"),
    ];
    let mut extra = key.clone();
    extra["extra"] = true.into();
    check(vec![
        (
            "a key without a conflict",
            true,
            set(json!([key]), json!([])),
        ),
        ("none", true, set(json!([]), json!([]))),
        ("32 keys, sorted", true, set(json!(sorted), json!([]))),
        ("33 keys", false, set(json!(over), json!([]))),
        (
            "unsorted keys",
            false,
            set(json!(unsorted.clone()), json!([])),
        ),
        (
            "a non-canonical key",
            false,
            set(
                json!([changed(
                    "viewingKey",
                    &format!("0x0{}", &text(&key["viewingKey"])[2..])
                )]),
                json!([]),
            ),
        ),
        (
            "a foreign chain",
            false,
            set(json!([changed("chainId", SEPOLIA_CHAIN)]), json!([])),
        ),
        (
            "a conflict without its key",
            false,
            set(json!([]), json!([conflict])),
        ),
        ("a repeated key", false, set(json!([key, key]), json!([]))),
        ("an unknown key", false, set(json!([extra]), json!([]))),
        (
            "a multisig id that is not an account id",
            false,
            set(json!([changed("multisigId", "SN_MAIN:0x0ab")]), json!([])),
        ),
        (
            "another scheme",
            false,
            set(
                json!([changed("scheme", "ledger-strk20-typed-data-v1")]),
                json!([]),
            ),
        ),
    ]);
    unsorted.reverse();
    assert!(opens(&{
        let mut p = full();
        p["multisigStrk20ViewingKeys"] = json!(unsorted);
        p["multisigStrk20KeyConflicts"] = json!([]);
        p
    }));
}
