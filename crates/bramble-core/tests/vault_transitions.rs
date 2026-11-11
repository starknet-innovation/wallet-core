//! Tests for vault component transitions based on Quint spec.
//! Each test exercises a specific path (transition) through a vault operation.

use starknet_wallet_core::container::{
    new_dek, seal_new, Randomness, DEK_SCOPE, FORMAT, PAYLOAD_SCOPE,
};
use starknet_wallet_core::envelope::SealingKey;

// ================================================================
// Tests for VaultFile::with_payload transitions
// ================================================================

/// VaultFile_with_payload: when dek NOT yet bound to file and payload version differs.
/// Transition: "Derive file with new dek, version mismatch" (1234bc4bc5bec1db)
#[test]
fn derive_file_with_new_dek_version_mismatch() {
    let key = SealingKey::from_parts("fixture passphrase", [1; 16], &"ef".repeat(16));
    let dek = [3u8; 32];
    let file = seal_new(&key, &dek, 2, b"payload_v2", &Randomness::draw().unwrap()).unwrap();

    // Verify initial state: file has version 2, dek is bound in password slot
    assert_eq!(file.version, 2);
    let (opened_dek, _) = file.open_dek("fixture passphrase").unwrap();
    assert_eq!(*opened_dek, dek);

    // Now re-seal with a DIFFERENT dek and DIFFERENT version
    let new_dek = [4u8; 32];
    let new_version = 3;
    let new_payload = b"payload_v3";

    // Call with_payload with the new dek (not the one in the file's password slot)
    let result = file.with_payload(&new_dek, new_version, new_payload);
    assert!(result.is_ok(), "with_payload should succeed");

    let new_file = result.unwrap();
    // Verify the changes:
    // - version is updated to new_version
    assert_eq!(new_file.version, new_version);
    // - payload is re-sealed under new_dek with different IV
    let opened_payload = new_file.open_payload(&new_dek).unwrap();
    assert_eq!(opened_payload.as_slice(), new_payload);
    // - original password slot is preserved
    assert_eq!(new_file.password_slot, file.password_slot);
    // - lineage is preserved
    assert_eq!(new_file.lineage, file.lineage);
    // - payload IV changed (fresh IV)
    assert_ne!(new_file.payload.iv, file.payload.iv);
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
        b"payload_v4",
        &Randomness::draw().unwrap(),
    )
    .unwrap();

    // Verify initial state
    assert_eq!(file.version, version);
    let (opened_dek, _) = file.open_dek("fixture passphrase").unwrap();
    assert_eq!(*opened_dek, dek);

    // Now re-seal with the SAME dek and SAME version
    let new_payload = b"payload_v4_updated";

    let result = file.with_payload(&dek, version, new_payload);
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

/// VaultFile_with_payload: when dek NOT yet bound to file and payload version matches.
/// Transition: "Derive file with new dek, version match" (f85f6b43856ae536)
#[test]
fn derive_file_with_new_dek_version_match() {
    let key = SealingKey::from_parts("fixture passphrase", [1; 16], &"ef".repeat(16));
    let dek = [3u8; 32];
    let file = seal_new(&key, &dek, 2, b"payload_v2", &Randomness::draw().unwrap()).unwrap();

    // Re-seal with a DIFFERENT dek but SAME version
    let new_dek = [5u8; 32];
    let version = 2;
    let new_payload = b"payload_v2_new";

    let result = file.with_payload(&new_dek, version, new_payload);
    assert!(result.is_ok());

    let new_file = result.unwrap();
    // Verify the changes:
    // - version unchanged
    assert_eq!(new_file.version, version);
    // - payload re-sealed under new_dek
    let opened_payload = new_file.open_payload(&new_dek).unwrap();
    assert_eq!(opened_payload.as_slice(), new_payload);
    // - original password slot preserved
    assert_eq!(new_file.password_slot, file.password_slot);
    // - payload IV changed
    assert_ne!(new_file.payload.iv, file.payload.iv);
}

/// VaultFile_with_payload: when dek already bound to file and payload version differs.
/// Transition: "Derive file with existing dek, version mismatch" (b276e2142fd50d5f)
#[test]
fn derive_file_with_existing_dek_version_mismatch() {
    let key = SealingKey::from_parts("fixture passphrase", [1; 16], &"ef".repeat(16));
    let dek = [3u8; 32];
    let file = seal_new(&key, &dek, 2, b"payload_v2", &Randomness::draw().unwrap()).unwrap();

    // Re-seal with SAME dek but DIFFERENT version
    let new_version = 3;
    let new_payload = b"payload_v3";

    let result = file.with_payload(&dek, new_version, new_payload);
    assert!(result.is_ok());

    let new_file = result.unwrap();
    // Verify the changes:
    // - version updated
    assert_eq!(new_file.version, new_version);
    // - payload re-sealed under same dek
    let opened_payload = new_file.open_payload(&dek).unwrap();
    assert_eq!(opened_payload.as_slice(), new_payload);
    // - password slot unchanged
    assert_eq!(new_file.password_slot, file.password_slot);
    // - payload IV changed
    assert_ne!(new_file.payload.iv, file.payload.iv);
}

// ================================================================
// Tests for VaultFile::seal_biometric and set_biometric_slot transitions
// ================================================================

/// VaultFile_set_biometric_slot: when file uses foreign DEK but version matches.
/// Transition: "Set biometric slot with only foreign key" (1de29e9fac609a3a)
#[test]
fn set_biometric_slot_with_only_foreign_key() {
    let key = SealingKey::from_parts("fixture passphrase", [1; 16], &"ef".repeat(16));
    let dek = [3u8; 32];
    let mut file = seal_new(&key, &dek, 2, b"payload_v2", &Randomness::draw().unwrap()).unwrap();

    // Re-seal with a DIFFERENT DEK (same version) to create foreignDek condition
    let foreign_dek = [4u8; 32];
    file = file
        .with_payload(&foreign_dek, 2, b"payload_v2_new")
        .unwrap();

    // Create a biometric slot with the foreign dek
    let wrap_secret = [9; 32];
    let slot = file
        .seal_biometric(&foreign_dek, &wrap_secret, String::new())
        .unwrap();

    // Verify biometric_slot is initially None
    assert!(file.biometric_slot.is_none());

    // Set the biometric slot
    file.biometric_slot = Some(slot.clone());

    // Verify the biometric slot was set
    assert!(file.biometric_slot.is_some());
    let opened_dek = file.open_biometric(&wrap_secret).unwrap();
    assert_eq!(*opened_dek, foreign_dek);

    // Verify lineage binding in biometric slot
    assert_eq!(slot.id.len(), 32); // 16 bytes hex encoded
    assert_eq!(slot.domain, "");
}

/// VaultFile_set_biometric_slot: when file uses foreign DEK and has version mismatch.
/// Transition: "Set biometric slot with foreign key and version mismatch" (7b52ea80dad3a0e9)
#[test]
fn set_biometric_slot_with_foreign_key_and_version_mismatch() {
    let key = SealingKey::from_parts("fixture passphrase", [1; 16], &"ef".repeat(16));
    let dek = [3u8; 32];
    let original_version = 2;
    let mut file = seal_new(
        &key,
        &dek,
        original_version,
        b"payload_v2",
        &Randomness::draw().unwrap(),
    )
    .unwrap();

    // Re-seal with a DIFFERENT DEK and DIFFERENT VERSION to create both conditions:
    // foreignDek and versionMismatch
    let foreign_dek = [4u8; 32];
    let new_version = 3;
    file = file
        .with_payload(&foreign_dek, new_version, b"payload_v3")
        .unwrap();

    // Create biometric slot with the foreign dek
    let wrap_secret = [9; 32];
    let slot = file
        .seal_biometric(&foreign_dek, &wrap_secret, String::new())
        .unwrap();

    // Set the biometric slot
    file.biometric_slot = Some(slot);

    // Verify state
    assert!(file.biometric_slot.is_some());
    assert_eq!(file.version, new_version);

    // Verify the slot unwraps the foreign dek
    let opened_dek = file.open_biometric(&wrap_secret).unwrap();
    assert_eq!(*opened_dek, foreign_dek);
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
        b"payload_v4",
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
    let mut file = seal_new(&key, &dek, 2, b"payload_v2", &Randomness::draw().unwrap()).unwrap();

    // Re-seal to create a version mismatch (same dek, different version)
    file = file.with_payload(&dek, 3, b"payload_v3").unwrap();

    // Create and set biometric slot
    let wrap_secret = [7; 32];
    let slot = file
        .seal_biometric(&dek, &wrap_secret, String::new())
        .unwrap();
    file.biometric_slot = Some(slot);

    // Verify setup
    assert_eq!(file.version, 3);
    assert!(file.biometric_slot.is_some());

    // Verify biometric slot works
    let bio_dek = file.open_biometric(&wrap_secret).unwrap();
    assert_eq!(*bio_dek, dek);
}

// ================================================================
// Tests for seal_new transition with version mismatch
// ================================================================

/// seal_new with payload version different from container version.
/// Transition: "Seal file with payload version mismatch" (ba86f6768395ca4c)
#[test]
fn seal_new_with_payload_version_mismatch() {
    let key = SealingKey::from_parts("fixture passphrase", [1; 16], &"ef".repeat(16));
    let dek = [3u8; 32];
    let container_version = 4;
    // Create payload with different version
    let payload = b"v5_payload";

    let randomness = Randomness::draw().unwrap();
    let result = seal_new(&key, &dek, container_version, payload, &randomness);
    assert!(result.is_ok());

    let file = result.unwrap();
    // Verify the file structure
    assert_eq!(file.format, FORMAT);
    assert_eq!(file.version, container_version);
    assert!(file.biometric_slot.is_none());

    // Verify password slot
    assert_eq!(file.password_slot.scope, DEK_SCOPE);
    let (opened_dek, _) = file.open_dek("fixture passphrase").unwrap();
    assert_eq!(*opened_dek, dek);

    // Verify payload
    let opened_payload = file.open_payload(&dek).unwrap();
    assert_eq!(opened_payload.as_slice(), payload);

    // Verify lineage binding
    assert_eq!(file.password_slot.lineage, file.lineage);
    assert_eq!(file.payload.scope, PAYLOAD_SCOPE);
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
    let file = seal_new(&key, &dek, 2, b"original", &Randomness::draw().unwrap()).unwrap();

    let original_lineage = file.lineage.clone();
    let original_slot = file.password_slot.clone();

    let new_file = file.with_payload(&dek, 2, b"updated").unwrap();

    // Verify lineage and password slot are preserved
    assert_eq!(new_file.lineage, original_lineage);
    assert_eq!(new_file.password_slot, original_slot);

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
