//! The Rust port of the STRK20 privacy SDK's primitives (#182, S1) against the SDK itself, from
//! `extension-strk20-primitives.json` (written by `bun run mobile:parity` from the vendored SDK):
//! the tagged hashes the client path uses, viewing public keys, `generateRandom` and
//! `generateRandom120` over a fixed byte stream, and `compile_actions` calldata, byte for byte.

use crate::privacy_support::decode_rows;

use serde_json::Value;
use starknet_wallet_core::privacy::client_actions::{compile_actions_calldata, ClientAction};
use starknet_wallet_core::privacy::entropy::{generate_random, generate_random120, Entropy};
use starknet_wallet_core::privacy::hashes::compute_note_id;
use starknet_wallet_core::{
    compute_channel_key, compute_identity_key, stark_public_key, Error, Felt, SecretFelt,
};

fn fixture() -> Value {
    let mut value: Value =
        serde_json::from_str(include_str!("fixtures/extension-strk20-primitives.json")).unwrap();
    decode_rows(
        &mut value,
        &["clientActions", "hashes", "publicKeys", "random"],
    );
    value
}

fn felt(value: &Value) -> Felt {
    Felt::from_hex(value.as_str().unwrap()).unwrap()
}

fn secret(value: &Value) -> SecretFelt {
    SecretFelt::new(felt(value))
}

fn u32_of(value: &Value) -> u32 {
    u32::try_from(felt(value)).unwrap()
}

fn u128_of(value: &Value) -> u128 {
    u128::try_from(felt(value)).unwrap()
}

/// The SDK's spelling of a felt: `0x`, lowercase, no leading zeros.
fn hex(felt: &Felt) -> String {
    format!("{felt:#x}")
}

#[test]
fn hashes_match_the_sdk() {
    let mut checked = 0;
    for row in fixture()["hashes"].as_array().unwrap() {
        let args = row["args"].as_array().unwrap();
        let result = match row["function"].as_str().unwrap() {
            "compute_channel_key" => *compute_channel_key(
                &felt(&args[0]),
                &secret(&args[1]),
                &felt(&args[2]),
                &felt(&args[3]),
            )
            .expose_secret(),
            "compute_identity_key" => {
                *compute_identity_key(&felt(&args[0]), &secret(&args[1]), &felt(&args[2]))
                    .expose_secret()
            }
            "compute_note_id" => {
                compute_note_id(&secret(&args[0]), &felt(&args[1]), u32_of(&args[2]))
            }
            // The nullifier, marker, subchannel, outgoing-channel and encryption hashes, and the
            // bare `hash`, are not on the client path, so they are not ported.
            _ => continue,
        };
        assert_eq!(hex(&result), row["result"].as_str().unwrap(), "{row}");
        checked += 1;
    }
    assert_eq!(checked, 8);
}

#[test]
fn viewing_public_keys_match_the_sdk() {
    let rows = fixture()["publicKeys"].as_array().unwrap().clone();
    assert!(!rows.is_empty());
    for row in rows {
        let public = stark_public_key(&secret(&row["privateKey"])).unwrap();
        assert_eq!(
            felt(&Value::String(public)),
            felt(&row["publicKey"]),
            "{row}"
        );
    }
}

/// Hands out the fixture's bytes and records the length of each read.
struct Stream(Vec<u8>, Vec<usize>);

impl Entropy for Stream {
    fn fill(&mut self, bytes: &mut [u8]) -> Result<(), Error> {
        let rest = self.0.split_off(bytes.len().min(self.0.len()));
        bytes.copy_from_slice(&self.0);
        self.0 = rest;
        self.1.push(bytes.len());
        Ok(())
    }
}

#[test]
fn randomness_matches_the_sdk_over_the_same_bytes() {
    let rows = fixture()["random"].as_array().unwrap().clone();
    assert!(!rows.is_empty());
    for row in rows {
        let text = row["bytes"].as_str().unwrap();
        let bytes = (0..text.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap())
            .collect();
        let mut stream = Stream(bytes, vec![]);
        let result = match row["function"].as_str().unwrap() {
            "generateRandom" => generate_random(&mut stream).unwrap(),
            "generateRandom120" => Felt::from(generate_random120(&mut stream).unwrap()),
            other => panic!("unknown function {other}"),
        };
        assert_eq!(hex(&result), row["result"].as_str().unwrap(), "{row}");
        let reads: Vec<usize> = serde_json::from_value(row["reads"].clone()).unwrap();
        assert_eq!(stream.1, reads, "{row}");
        assert!(stream.0.is_empty(), "every byte is consumed: {row}");
    }
}

fn action(value: &Value) -> ClientAction {
    let input = &value["input"];
    let felt = |name: &str| felt(&input[name]);
    let index = || u32_of(&input["index"]);
    match value["type"].as_str().unwrap() {
        "SetViewingKey" => ClientAction::SetViewingKey {
            random: felt("random"),
        },
        "OpenChannel" => ClientAction::OpenChannel {
            recipient_addr: felt("recipient_addr"),
            index: index(),
            random: felt("random"),
            salt: felt("salt"),
        },
        "OpenSubchannel" => ClientAction::OpenSubchannel {
            recipient_addr: felt("recipient_addr"),
            recipient_public_key: felt("recipient_public_key"),
            channel_key: secret(&input["channel_key"]),
            index: index(),
            token: felt("token"),
            salt: felt("salt"),
        },
        "CreateEncNote" => ClientAction::CreateEncNote {
            recipient_addr: felt("recipient_addr"),
            recipient_public_key: felt("recipient_public_key"),
            token: felt("token"),
            amount: u128_of(&input["amount"]),
            index: index(),
            salt: u128_of(&input["salt"]),
        },
        "CreateOpenNote" => ClientAction::CreateOpenNote {
            recipient_addr: felt("recipient_addr"),
            recipient_public_key: felt("recipient_public_key"),
            token: felt("token"),
            index: index(),
            random: felt("random"),
        },
        "Deposit" => ClientAction::Deposit {
            token: felt("token"),
            amount: u128_of(&input["amount"]),
        },
        "UseNote" => ClientAction::UseNote {
            channel_key: secret(&input["channel_key"]),
            token: felt("token"),
            index: index(),
        },
        "Withdraw" => ClientAction::Withdraw {
            to_addr: felt("to_addr"),
            token: felt("token"),
            amount: u128_of(&input["amount"]),
            random: felt("random"),
        },
        other => panic!("unknown client action {other}"),
    }
}

#[test]
fn compile_actions_calldata_matches_the_sdk_byte_for_byte() {
    let rows = fixture()["clientActions"].as_array().unwrap().clone();
    assert!(!rows.is_empty());
    for row in rows {
        let actions: Vec<ClientAction> = row["actions"]
            .as_array()
            .unwrap()
            .iter()
            .map(action)
            .collect();
        let calldata =
            compile_actions_calldata(&felt(&row["user"]), &secret(&row["viewingKey"]), &actions)
                .unwrap();
        let spelled: Vec<String> = calldata
            .iter()
            .map(|felt| felt.expose_secret_hex().to_string())
            .collect();
        let expected: Vec<String> = serde_json::from_value(row["calldata"].clone()).unwrap();
        assert_eq!(spelled, expected, "{row}");
    }
}
