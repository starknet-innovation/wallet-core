//! Shared by the multisig proposal tests: `extension-multisig-transactions.json`, Bramble's
//! pinned class and the published phrase's OpenZeppelin keys. Selectors and signature checks are
//! Krusty's own (`kms`), so every signature here, the extension's included, is verified for real.
#![allow(dead_code)] // Each multisig suite reads a different subset.

use bramble_core::catalogs::multisig;
use serde_json::{json, Value};
use starknet_wallet_core::accounts::multisig::{ArgentMultisigClass, ArgentMultisigConfig};
use starknet_wallet_core::accounts::multisig_records::Record;
use starknet_wallet_core::accounts::{AccountFamily, Ladder};
use starknet_wallet_core::send::multisig_invoke_plan::MultisigStep;
use starknet_wallet_core::send::types::StepReply;
use starknet_wallet_core::{account_keypair, SecretFelt};

pub fn fixture() -> Value {
    let path = format!(
        "{}/tests/fixtures/extension-multisig-transactions.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// A compact JSON string the generator stored, decoded.
pub fn decode(value: &Value) -> Value {
    serde_json::from_str(value.as_str().unwrap()).unwrap()
}

pub fn class() -> ArgentMultisigClass {
    multisig::argent_multisig_class().clone()
}

pub fn summary_length() -> usize {
    multisig::get().limits.summary_length
}

pub fn record(network: &Value) -> Record {
    serde_json::from_value(network["account"].clone()).unwrap()
}

pub fn current_config(record: &Record) -> ArgentMultisigConfig {
    let config = record.current_config.as_ref().unwrap();
    ArgentMultisigConfig {
        threshold: config.threshold,
        signer_public_keys: config.signer_public_keys.clone(),
    }
}

/// OpenZeppelin account `index` of the fixture's phrase: its signing key and public key.
pub fn signer(index: u32) -> (SecretFelt, String) {
    let phrase = fixture()["mnemonic"].as_str().unwrap().to_string();
    let ladder = Ladder::vendor(AccountFamily::OpenZeppelin);
    let (pair, _) = account_keypair(&phrase, None, ladder, index).unwrap();
    (pair.private_key, pair.public_key_x)
}

pub fn rpc_reply(id: u64, recorded: &Value) -> StepReply {
    let mut body = json!({ "jsonrpc": "2.0", "id": id });
    for (key, value) in recorded.as_object().unwrap() {
        body[key] = value.clone();
    }
    StepReply::Rpc {
        id,
        body: body.to_string(),
        now_ms: 0,
    }
}

/// (id, method, params, is_write) of an RPC step.
pub fn rpc_parts(step: &MultisigStep) -> (u64, String, Value, bool) {
    let MultisigStep::Rpc {
        id, body, is_write, ..
    } = step
    else {
        panic!("expected an RPC step, got {step:?}");
    };
    let sent: Value = serde_json::from_str(body).unwrap();
    assert_eq!((&sent["jsonrpc"], &sent["id"]), (&json!("2.0"), &json!(id)));
    (
        *id,
        sent["method"].as_str().unwrap().to_string(),
        sent["params"].clone(),
        *is_write,
    )
}

/// Answers every RPC step from `exchanges`, in order, holding each body to the recorded one;
/// returns the first step that is not an RPC, after every exchange was used.
pub fn drive(
    mut step: MultisigStep,
    exchanges: &[Value],
    mut advance: impl FnMut(StepReply) -> MultisigStep,
) -> MultisigStep {
    let mut next = 0;
    while let MultisigStep::Rpc { .. } = step {
        let (id, method, params, _) = rpc_parts(&step);
        let exchange = exchanges
            .get(next)
            .unwrap_or_else(|| panic!("core sent {method} past the capture"));
        assert_eq!(method, exchange["method"].as_str().unwrap());
        let recorded = match &exchange["params"] {
            Value::Null => json!([]),
            params => params.clone(),
        };
        assert_eq!(params, recorded, "{method}");
        next += 1;
        step = advance(rpc_reply(id, &exchange["reply"]));
    }
    assert_eq!(next, exchanges.len(), "core stopped early at {step:?}");
    step
}
