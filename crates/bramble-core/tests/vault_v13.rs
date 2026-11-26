//! The vault payload is the extension's schema v13 (`fixtures/vault-v13.json`, G4): the
//! extension's own store sealed it for the published phrase and a fixed account set, one of each
//! kind mobile holds. The core reads that payload under every rule and writes it back unchanged,
//! and a wallet mobile builds for the same phrase and accounts seals an equal payload.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

use crate::common;

use bramble_core::testing::{open_vault, v13_round_trip};
use bramble_core::{AccountFamily, Ladder, NetworkId, RecoverySelection};
use common::{core, PASSWORD};
use serde_json::Value;
use starknet_wallet_core::envelope::{open, Envelope};

fn fixture() -> Value {
    let path = format!(
        "{}/tests/fixtures/vault-v13.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap()
}

#[test]
fn the_extensions_sealed_vault_holds_the_payload_the_core_reads_and_writes() {
    let fixture = fixture();
    let envelope: Envelope = serde_json::from_value(fixture["sealed"].clone()).unwrap();
    let (plaintext, _) = open(
        &envelope,
        text(&fixture["password"]),
        text(&fixture["scope"]),
    )
    .unwrap();
    let sealed: Value = serde_json::from_slice(&plaintext).unwrap();
    assert_eq!(sealed, fixture["payload"]);

    let written = v13_round_trip(std::str::from_utf8(&plaintext).unwrap()).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&written).unwrap(), sealed);
}

#[test]
fn a_wallet_built_here_seals_the_extensions_payload() {
    let fixture = fixture();
    // G4 as the extension's 13→14 step writes it back: a new wallet here is written at 14.
    let mut expected = fixture["payload"].clone();
    expected["schemaVersion"] = 14.into();
    let expected = &expected;
    let (dir, _clock, core) = core();
    core.import_wallet(PASSWORD.into(), text(&fixture["mnemonic"]).into())
        .unwrap();

    // The Mainnet accounts in the extension's order, each at its own coordinate and class.
    let accounts = expected["accounts"].as_array().unwrap();
    let mainnet: Vec<RecoverySelection> = accounts
        .iter()
        .filter(|account| account["networkId"] == "SN_MAIN")
        .map(|account| {
            let derivation = &account["derivation"];
            let index = |field: &str| derivation[field].as_u64().unwrap() as u32;
            let (ladder, account_index) = match text(&derivation["kind"]) {
                "argentx-bip44" => (Ladder::ArgentX, index("addressIndex")),
                _ if index("addressIndex") > 0 => (Ladder::Address, index("addressIndex")),
                _ => (Ladder::Account, index("accountIndex")),
            };
            let family = match text(&account["accountContractType"]) {
                "openzeppelin" => AccountFamily::OpenZeppelin,
                "argent" => AccountFamily::Argent,
                _ => AccountFamily::Braavos,
            };
            RecoverySelection {
                family,
                account_index,
                ladder: Some(ladder),
                class_hash: (family != AccountFamily::OpenZeppelin)
                    .then(|| text(&account["classHash"]).to_string()),
            }
        })
        .collect();
    assert_eq!(mainnet.len(), 5);
    core.restore_recovered_accounts(NetworkId::Mainnet, mainnet)
        .unwrap();
    // Sepolia's OZ 0 is Mainnet's projected; then the extension's active account.
    core.ensure_network_account(NetworkId::Sepolia).unwrap();
    core.select_account(text(&expected["activeAccountId"]).into())
        .unwrap();

    let built: Value =
        serde_json::from_str(&open_vault(&dir.path().to_string_lossy(), PASSWORD).unwrap())
            .unwrap();
    assert_eq!(&built, expected);
}
