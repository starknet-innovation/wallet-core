//! Tests for vault component transitions based on Quint spec.
//! Each test exercises a specific path (transition) through a vault operation.

use starknet_wallet_core::container::{
    new_dek, seal_new, Randomness, DEK_SCOPE, FORMAT, PAYLOAD_SCOPE,
};
use starknet_wallet_core::envelope::SealingKey;
use starknet_wallet_core::Error;

/// A payload document carrying its own version, as the vault's caller writes it. The Quint
/// model's `payloadVersion(payload)` is this field; the container never reads it.
fn doc(version: u32, body: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({ "version": version, "body": body })).unwrap()
}

/// The version a payload document claims.
fn doc_version(bytes: &[u8]) -> u32 {
    let value: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    u32::try_from(value["version"].as_u64().unwrap()).unwrap()
}

// ================================================================
// Tests for VaultFile::with_payload transitions
// ================================================================

/// VaultFile_with_payload under a DEK the slots do not wrap, with a version change.
/// Transition: "Derive file with new dek, version mismatch" (1234bc4bc5bec1db)
///
/// Refused: the kept password slot would no longer open the payload.
#[test]
fn derive_file_with_new_dek_version_mismatch_is_refused() {
    let key = SealingKey::from_parts("fixture passphrase", [1; 16], &"ef".repeat(16));
    let file = seal_new(
        &key,
        &[3; 32],
        2,
        &doc(2, "a"),
        &Randomness::draw().unwrap(),
    )
    .unwrap();

    assert!(matches!(
        file.with_payload(&[4; 32], 3, &doc(2, "b")),
        Err(Error::InvalidInput { .. })
    ));
}

/// VaultFile_with_payload: when dek already bound to file and payload version matches.
/// Transition: "Derive file with existing dek, version match" (3b4f8f595e649744)
#[test]
fn derive_file_with_existing_dek_version_match() {
    let key = SealingKey::from_parts("fixture passphrase", [1; 16], &"ef".repeat(16));
    let dek = [3u8; 32];
    let version = 4;
    let file = seal_new(
        &key,
        &dek,
        version,
        &doc(4, "a"),
        &Randomness::draw().unwrap(),
    )
    .unwrap();

    // Verify initial state
    assert_eq!(file.version, version);
    let (opened_dek, _) = file.open_dek("fixture passphrase").unwrap();
    assert_eq!(*opened_dek, dek);

    // Now re-seal with the SAME dek and SAME version
    let new_payload = doc(version, "b");

    let result = file.with_payload(&dek, version, &new_payload);
    assert!(result.is_ok());

    let new_file = result.unwrap();
    // Verify the changes:
    // - version is unchanged
    assert_eq!(new_file.version, version);
    // - payload is re-sealed under same dek
    let opened_payload = new_file.open_payload(&dek).unwrap();
    assert_eq!(opened_payload.as_slice(), new_payload);
    // - password slot unchanged (same dek binding)
    assert_eq!(new_file.password_slot, file.password_slot);
    // - payload IV changed (fresh IV)
    assert_ne!(new_file.payload.iv, file.payload.iv);
}

/// VaultFile_with_payload under a DEK the slots do not wrap, same version.
/// Transition: "Derive file with new dek, version match" (f85f6b43856ae536)
///
/// Refused, as above, whether or not the version changes.
#[test]
fn derive_file_with_new_dek_version_match_is_refused() {
    let key = SealingKey::from_parts("fixture passphrase", [1; 16], &"ef".repeat(16));
    let file = seal_new(
        &key,
        &[3; 32],
        2,
        &doc(2, "a"),
        &Randomness::draw().unwrap(),
    )
    .unwrap();

    assert!(matches!(
        file.with_payload(&[5; 32], 2, &doc(2, "b")),
        Err(Error::InvalidInput { .. })
    ));
}

/// VaultFile_with_payload: when dek already bound to file and payload version differs.
/// Transition: "Derive file with existing dek, version mismatch" (b276e2142fd50d5f)
#[test]
fn derive_file_with_existing_dek_version_mismatch() {
    let key = SealingKey::from_parts("fixture passphrase", [1; 16], &"ef".repeat(16));
    let dek = [3u8; 32];
    let file = seal_new(&key, &dek, 2, &doc(2, "a"), &Randomness::draw().unwrap()).unwrap();

    // Re-seal with SAME dek but DIFFERENT version
    let new_version = 3;
    let new_payload = doc(2, "b");

    let result = file.with_payload(&dek, new_version, &new_payload);
    assert!(result.is_ok());

    let new_file = result.unwrap();
    // Verify the changes:
    // - version updated
    assert_eq!(new_file.version, new_version);
    // - payload re-sealed under same dek, still claiming its own version
    let opened_payload = new_file.open_payload(&dek).unwrap();
    assert_eq!(opened_payload.as_slice(), new_payload);
    assert_ne!(doc_version(&opened_payload), new_file.version);
    // - password slot unchanged
    assert_eq!(new_file.password_slot, file.password_slot);
    // - payload IV changed
    assert_ne!(new_file.payload.iv, file.payload.iv);
}

// ================================================================
// Tests for VaultFile::seal_biometric and set_biometric_slot transitions
// ================================================================

/// VaultFile_seal_biometric under a DEK the file does not hold, same version.
/// Transition: "Set biometric slot with only foreign key" (1de29e9fac609a3a)
///
/// Refused: the slot would unwrap a key that opens nothing. No slot is produced.
#[test]
fn set_biometric_slot_with_only_foreign_key_is_refused() {
    let key = SealingKey::from_parts("fixture passphrase", [1; 16], &"ef".repeat(16));
    let file = seal_new(
        &key,
        &[3; 32],
        2,
        &doc(2, "a"),
        &Randomness::draw().unwrap(),
    )
    .unwrap();

    assert!(matches!(
        file.seal_biometric(&[4; 32], &[9; 32], String::new()),
        Err(Error::InvalidInput { .. })
    ));
    assert!(file.biometric_slot.is_none());
}

/// VaultFile_seal_biometric under a foreign DEK on a file whose version changed.
/// Transition: "Set biometric slot with foreign key and version mismatch" (7b52ea80dad3a0e9)
///
/// Refused, as above; the version upgrade under the file's own DEK still goes through.
#[test]
fn set_biometric_slot_with_foreign_key_and_version_mismatch_is_refused() {
    let key = SealingKey::from_parts("fixture passphrase", [1; 16], &"ef".repeat(16));
    let dek = [3u8; 32];
    let file = seal_new(&key, &dek, 2, &doc(2, "a"), &Randomness::draw().unwrap()).unwrap();
    let file = file.with_payload(&dek, 3, &doc(2, "b")).unwrap();
    assert_eq!(file.version, 3);

    assert!(matches!(
        file.seal_biometric(&[4; 32], &[9; 32], String::new()),
        Err(Error::InvalidInput { .. })
    ));
    assert!(file.biometric_slot.is_none());
}

/// VaultFile_set_biometric_slot: when file uses own DEK and version matches.
/// Transition: "Set biometric slot with matching key and version" (7f2f38a1b18ea858)
#[test]
fn set_biometric_slot_with_matching_key_and_version() {
    let key = SealingKey::from_parts("fixture passphrase", [1; 16], &"ef".repeat(16));
    let dek = [3u8; 32];
    let version = 4;
    let mut file = seal_new(
        &key,
        &dek,
        version,
        &doc(4, "a"),
        &Randomness::draw().unwrap(),
    )
    .unwrap();

    // Verify initial state
    assert_eq!(file.version, version);
    let (opened_dek, _) = file.open_dek("fixture passphrase").unwrap();
    assert_eq!(*opened_dek, dek);

    // Create and set biometric slot
    let wrap_secret = [8; 32];
    let slot = file
        .seal_biometric(&dek, &wrap_secret, String::new())
        .unwrap();
    file.biometric_slot = Some(slot);

    // Verify the slot works
    assert!(file.biometric_slot.is_some());
    let bio_dek = file.open_biometric(&wrap_secret).unwrap();
    assert_eq!(*bio_dek, dek);
}

/// VaultFile_set_biometric_slot: when file has version mismatch but uses own DEK.
/// Transition: "Set biometric slot with only version mismatch" (b3d138f5533459ad)
#[test]
fn set_biometric_slot_with_only_version_mismatch() {
    let key = SealingKey::from_parts("fixture passphrase", [1; 16], &"ef".repeat(16));
    let dek = [3u8; 32];
    let mut file = seal_new(&key, &dek, 2, &doc(2, "a"), &Randomness::draw().unwrap()).unwrap();

    // Re-seal to create a version mismatch (same dek, different version)
    file = file.with_payload(&dek, 3, &doc(2, "b")).unwrap();

    // Create and set biometric slot
    let wrap_secret = [7; 32];
    let slot = file
        .seal_biometric(&dek, &wrap_secret, String::new())
        .unwrap();
    file.biometric_slot = Some(slot);

    // Verify setup
    assert_eq!(file.version, 3);
    assert!(file.biometric_slot.is_some());

    // Verify biometric slot works and the payload still claims its own version
    let bio_dek = file.open_biometric(&wrap_secret).unwrap();
    assert_eq!(*bio_dek, dek);
    assert_ne!(
        doc_version(&file.open_payload(&bio_dek).unwrap()),
        file.version
    );
}

// ================================================================
// Tests for seal_new transition with version mismatch
// ================================================================

/// seal_new with a payload whose own version differs from the container version.
/// Transition: "Seal file with payload version mismatch" (ba86f6768395ca4c)
///
/// The container treats the payload as opaque bytes: it neither parses the payload's version
/// nor compares it with `VaultFile::version`. That comparison is the caller's (see
/// `VaultFile::open_payload`), and no code in this repository performs it yet, so this test
/// only pins that `seal_new` passes such a payload through unchanged.
#[test]
fn seal_new_passes_mismatched_payload_version_through() {
    let key = SealingKey::from_parts("fixture passphrase", [1; 16], &"ef".repeat(16));
    let dek = [3u8; 32];
    let payload = br#"{"version":5,"accounts":[]}"#;

    let file = seal_new(&key, &dek, 4, payload, &Randomness::draw().unwrap()).unwrap();

    assert_eq!(file.format, FORMAT);
    assert_eq!(file.version, 4);
    assert!(file.biometric_slot.is_none());
    assert_eq!(file.password_slot.scope, DEK_SCOPE);
    assert_eq!(file.password_slot.lineage, file.lineage);
    assert_eq!(file.payload.scope, PAYLOAD_SCOPE);

    let (opened_dek, _) = file.open_dek("fixture passphrase").unwrap();
    let opened: serde_json::Value =
        serde_json::from_slice(&file.open_payload(&opened_dek).unwrap()).unwrap();
    assert_eq!(opened["version"], 5);
}

// ================================================================
// Tests for new_dek transition
// ================================================================

/// Generate new data encryption key.
/// Transition: "Generate new data encryption key" (62cb62e5d00aafdb)
#[test]
fn generate_new_data_encryption_key() {
    let dek1 = new_dek().unwrap();
    let dek2 = new_dek().unwrap();

    // Verify DEKs are 32 bytes
    assert_eq!(dek1.len(), 32);
    assert_eq!(dek2.len(), 32);

    // Verify DEKs are different (with overwhelming probability)
    assert_ne!(*dek1, *dek2);
}

// ================================================================
// Tests for SealingKey::seal with version mismatches
// ================================================================

/// Seal a new container and verify roundtrip with mismatched version.
#[test]
fn seal_new_roundtrip_preserves_structure() {
    let key = SealingKey::from_parts("passphrase", [2; 16], &"ab".repeat(16));
    let dek = [5u8; 32];
    let version = 5;
    let payload = b"test_payload";

    let randomness = Randomness::draw().unwrap();
    let file = seal_new(&key, &dek, version, payload, &randomness).unwrap();

    // Verify structure
    assert_eq!(file.format, FORMAT);
    assert_eq!(file.version, version);
    assert_eq!(file.payload.scope, PAYLOAD_SCOPE);
    assert_eq!(file.password_slot.scope, DEK_SCOPE);

    // Verify we can open with password
    let (opened_dek, re_key) = file.open_dek("passphrase").unwrap();
    assert_eq!(*opened_dek, dek);

    // Verify we can re-open with the key without KDF
    let resealed = re_key.open_sealed(&file.password_slot, DEK_SCOPE).unwrap();
    // Verify the plaintext is valid JSON with a dek field
    let _: serde_json::Value = serde_json::from_slice(&resealed).unwrap();
}

// ================================================================
// Tests for VaultFile validation
// ================================================================

/// Verify that with_payload preserves all slots and lineage.
#[test]
fn with_payload_preserves_slots_and_lineage() {
    let key = SealingKey::from_parts("passphrase", [3; 16], &"cd".repeat(16));
    let dek = [6u8; 32];
    let mut file = seal_new(&key, &dek, 2, b"original", &Randomness::draw().unwrap()).unwrap();
    let wrap_secret = [7; 32];
    file.biometric_slot = Some(
        file.seal_biometric(&dek, &wrap_secret, String::new())
            .unwrap(),
    );

    let original_lineage = file.lineage.clone();
    let original_slot = file.password_slot.clone();
    let original_biometric = file.biometric_slot.clone();

    let new_file = file.with_payload(&dek, 2, b"updated").unwrap();

    // Verify lineage and both slots are preserved, and both still unwrap the DEK
    assert_eq!(new_file.lineage, original_lineage);
    assert_eq!(new_file.password_slot, original_slot);
    assert_eq!(new_file.biometric_slot, original_biometric);
    assert_eq!(*new_file.open_dek("passphrase").unwrap().0, dek);
    assert_eq!(*new_file.open_biometric(&wrap_secret).unwrap(), dek);

    // Verify payload changed
    assert_ne!(new_file.payload.iv, file.payload.iv);
    let opened = new_file.open_payload(&dek).unwrap();
    assert_eq!(opened.as_slice(), b"updated");
}

// ================================================================
// Additional comprehensive tests
// ================================================================

/// Full vault lifecycle: create, seal, upgrade, add biometric.
#[test]
fn full_vault_lifecycle() {
    let password = "my secure passphrase";
    let key = SealingKey::create(password).unwrap();
    let dek = new_dek().unwrap();
    let wrap_secret = [8; 32];

    // Create initial vault (version 2)
    let file = seal_new(&key, &dek, 2, b"v2_data", &Randomness::draw().unwrap()).unwrap();

    // Verify it can be opened with password
    let (opened_dek, _re_key) = file.open_dek(password).unwrap();
    assert_eq!(*opened_dek, *dek);

    // Upgrade to version 3
    let file_v3 = file.with_payload(&dek, 3, b"v3_data").unwrap();
    assert_eq!(file_v3.version, 3);

    // Add biometric slot
    let slot = file_v3
        .seal_biometric(&dek, &wrap_secret, String::new())
        .unwrap();
    let mut file_with_bio = file_v3.clone();
    file_with_bio.biometric_slot = Some(slot);

    // Verify biometric works
    let bio_dek = file_with_bio.open_biometric(&wrap_secret).unwrap();
    assert_eq!(*bio_dek, *dek);

    // Verify payload still works with either DEK
    let payload = file_with_bio.open_payload(&dek).unwrap();
    assert_eq!(payload.as_slice(), b"v3_data");
}
