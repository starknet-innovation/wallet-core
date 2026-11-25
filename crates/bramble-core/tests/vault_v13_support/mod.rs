//! A v13 vault with every collection filled, built in test code over G4's payload
//! (`fixtures/vault-v13.json`, read, never edited). No Bramble generator writes one yet (PLAN.md
//! P7), so the records follow the extension's validators and its own tests' values. Every key,
//! public key and address comes from G4, so from the published parity phrase; the pool,
//! anonymizer and multisig address are the extension's test felts (`schema-v12.test.ts`,
//! `multisig-strk20-key.test.ts`), public values, not keys.
#![allow(dead_code)] // Each test crate uses a different subset.

use bramble_core::testing::v13_round_trip;
use bramble_core::CoreError;
use serde_json::{json, Value};

pub const POOL: &str = "0x40337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a";
pub const ANONYMIZER: &str = "0x4f33230dc57855c6e7eabe66dfa0fde82c5458fd0e54827cdb7cb4c474888a7";
pub const MAINNET_CHAIN: &str = "0x534e5f4d41494e";
pub const SEPOLIA_CHAIN: &str = "0x534e5f5345504f4c4941";
pub const ORIGIN: &str = "https://app.example";
/// `ledgerAccountPath(12, "argentx")` and `ledgerAccountPath(12)`.
pub const READY_PATH_12: &str = "m/2645'/1195502025'/1148870696'/0'/0'/12";
pub const LEDGERW_PATH_12: &str = "m/2645'/1195502025'/734980059'/0'/12'/0";

pub fn multisig_id() -> String {
    format!("SN_MAIN:0x{}", "ab".repeat(16))
}

pub fn fixture() -> Value {
    let path = format!(
        "{}/tests/fixtures/vault-v13.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// G4's payload as the extension sealed it.
pub fn g4() -> Value {
    fixture()["payload"].clone()
}

pub fn text(value: &Value) -> String {
    value.as_str().unwrap().to_string()
}

/// G4's record `index` placed on `network` (same key, class, salt and address).
fn moved(payload: &Value, index: usize, network: &str) -> Value {
    let mut account = payload["accounts"][index].clone();
    account["networkId"] = network.into();
    account["id"] = format!("{network}:{}", text(&account["address"])).into();
    account
}

/// G4's sealed key for record `index`, under `id`.
fn key_for(payload: &Value, index: usize, id: &Value) -> Value {
    json!({ "accountId": id, "privateKey": payload["signingKeys"][index]["privateKey"] })
}

/// The kept records `full` adds, by id: a Ledger account (Ready's scheme), an account derived
/// from an imported phrase, and one on the extension's devnet.
pub struct Kept {
    pub ledger: String,
    pub seeded: String,
    pub devnet: String,
}

pub fn kept_ids(payload: &Value) -> Kept {
    let find = |predicate: &dyn Fn(&Value) -> bool| {
        text(
            &payload["accounts"]
                .as_array()
                .unwrap()
                .iter()
                .find(|account| predicate(account))
                .unwrap()["id"],
        )
    };
    Kept {
        ledger: find(&|a| a["derivation"]["kind"] == "ledger"),
        seeded: find(&|a| a["derivation"].get("seedId").is_some()),
        devnet: find(&|a| a["networkId"] == "SN_DEVNET"),
    }
}

/// G4 with every v13 collection filled and the kept records placed between mobile's, their
/// keys out of account order, so a write back must put each back where it was.
pub fn full() -> Value {
    let mut payload = g4();
    let g4 = payload.clone();
    // G4's Argent at account 2 as a Ledger account on Sepolia (`ledger-accounts.test.ts`'s
    // Ready account: Ready's marker and path at index 12, no y-coordinate, no key).
    let mut ledger = moved(&g4, 4, "SN_SEPOLIA");
    ledger["derivation"] = json!({
        "accountIndex": 12, "eip2645application": "argentx", "kind": "ledger", "path": READY_PATH_12,
    });
    ledger.as_object_mut().unwrap().remove("publicKeyY");
    // G4's OZ 1 on Sepolia, derived from an imported phrase (`schema.test.ts`'s seed binding);
    // the imported phrase is the published one.
    let mut seeded = moved(&g4, 3, "SN_SEPOLIA");
    seeded["derivation"]["seedId"] = "phrase-1".into();
    // G4's OZ 0 on the extension's devnet.
    let devnet = moved(&g4, 0, "SN_DEVNET");
    let (ledger_id, seeded_id, devnet_id) = (
        ledger["id"].clone(),
        seeded["id"].clone(),
        devnet["id"].clone(),
    );
    let main_id = g4["accounts"][0]["id"].clone();
    let accounts = payload["accounts"].as_array_mut().unwrap();
    accounts.insert(2, ledger);
    accounts.insert(5, devnet);
    accounts.push(seeded);
    let keys = payload["signingKeys"].as_array_mut().unwrap();
    keys.insert(1, key_for(&g4, 3, &seeded_id));
    keys.push(key_for(&g4, 0, &devnet_id));
    payload["importedSeeds"] = json!([{ "id": "phrase-1", "mnemonic": fixture()["mnemonic"] }]);
    payload["accountSecrets"] = json!([{
        "accountId": ledger_id,
        "ledgerStrk20ViewingKeys": [{
            "chainId": SEPOLIA_CHAIN, "poolAddress": POOL,
            "scheme": "ledger-strk20-typed-data-v1", "viewingKey": g4["signingKeys"][1]["privateKey"],
        }],
    }]);
    payload["originGrants"] = json!([
        { "accountIds": [main_id, ledger_id], "origin": ORIGIN },
        { "accountIds": [devnet_id], "origin": "http://localhost:3000" },
    ]);
    payload["shadowAccountScopes"] = json!([
        {
            "accountId": main_id, "anonymizer": ANONYMIZER, "chainId": MAINNET_CHAIN,
            "label": "Market", "nonces": ["0x0", "0x2", "0x10"], "origin": ORIGIN, "poolAddress": POOL,
        },
        {
            "accountId": ledger_id, "anonymizer": ANONYMIZER, "chainId": SEPOLIA_CHAIN,
            "label": "Other", "nonces": [], "origin": ORIGIN, "poolAddress": POOL,
        },
    ]);
    payload["multisigStrk20ViewingKeys"] = json!([{
        "chainId": MAINNET_CHAIN, "multisigId": multisig_id(), "poolAddress": POOL,
        "scheme": "multisig-strk20-shared-v1", "viewingKey": g4["signingKeys"][2]["privateKey"],
    }]);
    payload["multisigStrk20KeyConflicts"] =
        json!([{ "chainId": MAINNET_CHAIN, "multisigId": multisig_id(), "poolAddress": POOL }]);
    payload
}

/// Reads `payload` as the core does on unlock and writes it back.
/// `payload` as the extension's 13→14 step writes it back (`schema/migrations.ts`): the version
/// alone; what an unlock of a v13 vault leaves on disk.
pub fn migrated(payload: &Value) -> Value {
    let mut payload = payload.clone();
    payload["schemaVersion"] = 14.into();
    payload
}

pub fn round_trip(payload: &Value) -> Result<Value, CoreError> {
    v13_round_trip(&payload.to_string()).map(|json| serde_json::from_str(&json).unwrap())
}

/// Whether the core opens `payload`; a refusal must be the vault's storage error.
pub fn opens(payload: &Value) -> bool {
    match round_trip(payload) {
        Ok(_) => true,
        Err(CoreError::Storage { .. }) => false,
        Err(other) => panic!("unexpected refusal: {other:?}"),
    }
}

/// Index of the record `id` in `payload["accounts"]`.
pub fn at(payload: &Value, id: &str) -> usize {
    payload["accounts"]
        .as_array()
        .unwrap()
        .iter()
        .position(|account| account["id"] == id)
        .unwrap()
}

/// A change to `full()`.
pub type Change = Box<dyn Fn(&mut Value)>;

/// Asserts the core opens `full()` changed by each mutation exactly when `accepted` says so.
pub fn check(cases: Vec<(&str, bool, Change)>) {
    for (why, accepted, mutate) in cases {
        let mut payload = full();
        mutate(&mut payload);
        assert_eq!(opens(&payload), accepted, "{why}");
    }
}
