//! The vault opens what the extension's `encryptJson` seals: a plain v3 envelope and a DEK
//! passphrase slot (`extension-parity.json`, see `fixtures/README.md`). Moved from mobile's
//! `tests/parity.rs` in Phase 2.2.

use serde_json::Value;
use wallet_core::container::{decode_dek, DEK_SCOPE};
use wallet_core::envelope::{open, Envelope};
use wallet_core::Error;

fn fixture() -> Value {
    let path = format!(
        "{}/tests/fixtures/extension-parity.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn sealed(vector: &Value) -> Envelope {
    serde_json::from_value(vector["sealed"].clone()).unwrap()
}

#[test]
fn opens_an_envelope_the_extension_sealed() {
    let fixture = fixture();
    let vector = &fixture["envelope"];
    let envelope = sealed(vector);
    let password = vector["password"].as_str().unwrap();
    let scope = vector["scope"].as_str().unwrap();

    let (plaintext, _) = open(&envelope, password, scope).unwrap();
    let plaintext: Value = serde_json::from_slice(&plaintext).unwrap();
    assert_eq!(plaintext, vector["plaintext"]);

    assert!(matches!(
        open(&envelope, "not the password", scope),
        Err(Error::WrongPassword)
    ));
    assert!(open(&envelope, password, "other").is_err());
}

#[test]
fn opens_a_dek_slot_the_extension_sealed() {
    let fixture = fixture();
    let vector = &fixture["dekSlot"];
    assert_eq!(vector["scope"], DEK_SCOPE);
    let envelope = sealed(vector);
    let password = vector["password"].as_str().unwrap();

    let (plaintext, key) = open(&envelope, password, DEK_SCOPE).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&plaintext).unwrap(),
        vector["plaintext"]
    );
    decode_dek(&plaintext).unwrap();
    // The returned key re-opens the slot without the KDF, as a container re-seal relies on.
    assert_eq!(*key.open_sealed(&envelope, DEK_SCOPE).unwrap(), *plaintext);

    assert!(matches!(
        open(&envelope, "not the password", DEK_SCOPE),
        Err(Error::WrongPassword)
    ));
}
