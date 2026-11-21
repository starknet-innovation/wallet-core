//! P7: the extension's v13 account validators, case by case, on the record kinds the reader
//! used to refuse (a Ledger account, one derived from an imported phrase, one on the extension's
//! devnet) and the Ledger viewing keys bound to them. Each case is the extension's own test (file
//! named beside it, under `packages/platform/vault/tests/` at `a01b240c`), moved onto
//! `vault_v13_support::full()`. The other collections are `vault_v13_rules.rs`.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

#[path = "vault_v13_support/mod.rs"]
mod support;

use serde_json::{json, Value};
use support::{at, check, full, kept_ids, text, Change, LEDGERW_PATH_12, MAINNET_CHAIN};

#[test]
fn ledger_accounts() {
    // `ledger-accounts.test.ts` ("keyless account in the vault schema") and `schema.test.ts`
    // ("mnemonic-less Ledger-only vault"), on the kept Ledger record.
    let full = full();
    let ledger = kept_ids(&full).ledger;
    let index = at(&full, &ledger);
    let derivation = move |change: Value| -> Change {
        Box::new(move |p: &mut Value| {
            for (key, value) in change.as_object().unwrap() {
                p["accounts"][index]["derivation"][key] = value.clone();
            }
        })
    };
    let ledger_key = ledger.clone();
    check(vec![
        (
            "Bramble's own scheme",
            true,
            derivation(json!({ "eip2645application": "LedgerW", "path": LEDGERW_PATH_12 })),
        ),
        (
            "an OpenZeppelin device account at any class",
            true,
            Box::new(move |p: &mut Value| {
                let account = &mut p["accounts"][index];
                account["accountContractType"] = "openzeppelin".into();
                account["classHash"] = "0x111111".into();
                account.as_object_mut().unwrap().remove("deploymentSalt");
            }),
        ),
        (
            "Bramble's marker over Ready's path",
            false,
            derivation(json!({ "eip2645application": "LedgerW" })),
        ),
        (
            "Ready's marker over Bramble's path",
            false,
            derivation(json!({ "path": LEDGERW_PATH_12 })),
        ),
        (
            "a path for another index",
            false,
            derivation(json!({ "accountIndex": 1 })),
        ),
        (
            "an unknown application",
            false,
            derivation(json!({ "eip2645application": "Other" })),
        ),
        (
            "a non-canonical path",
            false,
            derivation(json!({ "path": "m/44'/0'/0" })),
        ),
        (
            "an index at 2^31",
            false,
            derivation(
                json!({ "accountIndex": 2_147_483_648u64, "path": "m/2645'/1195502025'/1148870696'/0'/0'/2147483648" }),
            ),
        ),
        (
            "a signing key for a device account",
            false,
            Box::new(move |p: &mut Value| {
                let key = p["signingKeys"][0]["privateKey"].clone();
                p["signingKeys"]
                    .as_array_mut()
                    .unwrap()
                    .push(json!({ "accountId": ledger_key, "privateKey": key }));
            }),
        ),
        (
            "a y-coordinate",
            false,
            Box::new(move |p: &mut Value| p["accounts"][index]["publicKeyY"] = "0x1".into()),
        ),
        (
            "an Argent class this family was never deployed with",
            false,
            Box::new(move |p: &mut Value| p["accounts"][index]["classHash"] = "0x123".into()),
        ),
        (
            "Argent without its salt",
            false,
            Box::new(move |p: &mut Value| {
                p["accounts"][index]
                    .as_object_mut()
                    .unwrap()
                    .remove("deploymentSalt");
            }),
        ),
    ]);
}

#[test]
fn imported_phrase_and_devnet_accounts() {
    // `ledger-accounts.test.ts`: "a non-Ledger account still requires its signing key";
    // `schema.test.ts`: "rejects non-canonical account aliases".
    let full = full();
    let kept = kept_ids(&full);
    let (seeded, devnet) = (at(&full, &kept.seeded), at(&full, &kept.devnet));
    let seeded_id = kept.seeded.clone();
    check(vec![
        (
            "an imported-phrase account without its key",
            false,
            Box::new(move |p: &mut Value| {
                p["signingKeys"]
                    .as_array_mut()
                    .unwrap()
                    .retain(|key| key["accountId"] != seeded_id);
            }),
        ),
        (
            "an imported-phrase account without its y-coordinate",
            false,
            Box::new(move |p: &mut Value| {
                p["accounts"][seeded]
                    .as_object_mut()
                    .unwrap()
                    .remove("publicKeyY");
            }),
        ),
        (
            "a seed handle outside the grammar",
            false,
            Box::new(move |p: &mut Value| {
                p["accounts"][seeded]["derivation"]["seedId"] = "seed-1".into();
            }),
        ),
        (
            "an index past Number.MAX_SAFE_INTEGER",
            false,
            Box::new(move |p: &mut Value| {
                p["accounts"][seeded]["derivation"]["accountIndex"] = (1u64 << 53).into();
            }),
        ),
        (
            "an index at Number.MAX_SAFE_INTEGER",
            true,
            Box::new(move |p: &mut Value| {
                p["accounts"][seeded]["derivation"]["accountIndex"] = ((1u64 << 53) - 1).into();
            }),
        ),
        (
            "a devnet id that is not network:address",
            false,
            Box::new(move |p: &mut Value| {
                p["accounts"][devnet]["id"] = "SN_DEVNET:0x1".into();
            }),
        ),
        (
            "an unknown network",
            false,
            Box::new(move |p: &mut Value| {
                p["accounts"][devnet]["networkId"] = "SN_GOERLI".into();
            }),
        ),
    ]);
}

#[test]
fn account_secrets() {
    // `schema.test.ts`: "accepts only canonical Ledger-owned account-secret scopes", "rejects
    // optional viewing keys on software accounts or duplicate scopes".
    let full = full();
    let ledger = kept_ids(&full).ledger;
    let main = text(&full["accounts"][0]["id"]);
    let key = full["accountSecrets"][0]["ledgerStrk20ViewingKeys"][0].clone();
    let with = |id: &str, keys: Value| -> Change {
        let secrets = json!([{ "accountId": id, "ledgerStrk20ViewingKeys": keys }]);
        Box::new(move |p: &mut Value| p["accountSecrets"] = secrets.clone())
    };
    let changed = |field: &str, value: &str| {
        let mut key = key.clone();
        key[field] = value.into();
        key
    };
    let eight: Vec<Value> = (1..=8)
        .map(|n| changed("poolAddress", &format!("0x{n}")))
        .collect();
    let nine: Vec<Value> = (1..=9)
        .map(|n| changed("poolAddress", &format!("0x{n}")))
        .collect();
    check(vec![
        ("eight scopes", true, with(&ledger, json!(eight))),
        ("nine scopes", false, with(&ledger, json!(nine))),
        ("no scope", false, with(&ledger, json!([]))),
        (
            "an unknown account",
            false,
            with("SN_MAIN:0x999", json!([key])),
        ),
        ("a software account", false, with(&main, json!([key]))),
        (
            "another network's chain",
            false,
            with(&ledger, json!([changed("chainId", MAINNET_CHAIN)])),
        ),
        (
            "a non-canonical pool",
            false,
            with(&ledger, json!([changed("poolAddress", "0x0123")])),
        ),
        ("a repeated scope", false, with(&ledger, json!([key, key]))),
        (
            "another scheme",
            false,
            with(
                &ledger,
                json!([changed("scheme", "multisig-strk20-shared-v1")]),
            ),
        ),
        (
            "a non-canonical key",
            false,
            with(
                &ledger,
                json!([changed(
                    "viewingKey",
                    &format!("0x0{}", &text(&key["viewingKey"])[2..])
                )]),
            ),
        ),
    ]);
}
