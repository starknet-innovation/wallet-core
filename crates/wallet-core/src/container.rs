//! The vault container (vault version 2 on): a random data-encryption key (DEK) seals the
//! payload, and the DEK is wrapped by a passphrase slot (the extension's EncryptedEnvelope v3,
//! unchanged) and, optionally, by a biometric slot keyed by a host-held wrap secret W.
//!
//! The container's `version` is the wallet's vault schema version, passed in by the caller, and
//! the wallet keeps it equal to the `version` of the payload it seals. The payload is opaque
//! bytes here; the wallet's migrations parse and upgrade it.
//!
//! The DEK is generated, wrapped, unwrapped and zeroized here. Every part of the file is bound to
//! one lineage; a container of the wrong shape fails closed as `Storage { "vault file is invalid" }`
//! before any key is derived.

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::envelope::{
    self, additional_data, aead_open, aead_seal, decode, is_lower_hex, random, random_hex,
    Envelope, SealingKey,
};
use crate::host::Storage;
use crate::Error;

pub const FORMAT: &str = "bramble.mobile.vault";
/// The oldest container version (vault-security's layout); older vaults are bare envelopes.
pub const FIRST_CONTAINER_VERSION: u32 = 2;
/// Scope of the pre-container file, a bare envelope around the payload (vault version 1, and
/// the accounts branch's records payload before the branches merged).
pub const V1_SCOPE: &str = "bramble.mobile.vault.v1";
pub const DEK_SCOPE: &str = "bramble.mobile.dek.v1";
/// Names the sealed payload's purpose, not its schema: every container version uses it, and the
/// payload's own `version` (bound to the container's) carries the schema.
pub const PAYLOAD_SCOPE: &str = "bramble.mobile.payload.v2";
pub const BIOMETRIC_SCOPE: &str = "bramble.mobile.biometric.v1";
/// The vault's data-encryption key.
pub type Dek = Zeroizing<[u8; 32]>;

/// Passphrase-slot plaintext. JSON so the extension's own `encryptJson` can produce one.
#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(deny_unknown_fields)]
struct DekRecord {
    dek: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BiometricSlot {
    pub id: String,
    pub iv: String,
    pub data: String,
    /// SHA-256 (hex) of the host's biometric domain state; empty when the platform has none.
    pub domain: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SealedPayload {
    pub scope: String,
    pub iv: String,
    pub commit: String,
    pub data: String,
}

/// `vault.json` from vault version 2 on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct VaultFile {
    pub format: String,
    pub version: u32,
    pub lineage: String,
    pub password_slot: Envelope,
    pub biometric_slot: Option<BiometricSlot>,
    pub payload: SealedPayload,
}

/// What is on disk.
pub enum StoredVault {
    /// A bare envelope (scope `V1_SCOPE`): vault version 1, or the accounts branch's pre-container
    /// records payload. `migrations.rs` tells them apart by the payload inside.
    Envelope(Envelope),
    /// A DEK container, vault version 2 or later.
    Container(Box<VaultFile>),
}

fn invalid() -> Error {
    Error::storage("vault file is invalid")
}

/// Classifies `vault.json`: a top-level `format` key is a container (version 2 to
/// `current_version`), an envelope is a pre-container vault, anything else fails closed.
pub fn parse(text: &str, current_version: u32) -> Result<StoredVault, Error> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|_| invalid())?;
    let object = value.as_object().ok_or_else(invalid)?;
    if object.contains_key("format") {
        let file: VaultFile = serde_json::from_value(value).map_err(|_| invalid())?;
        file.validate(current_version)?;
        Ok(StoredVault::Container(Box::new(file)))
    } else if object.contains_key("cipher") && object.contains_key("kdf") {
        serde_json::from_value(value)
            .map(StoredVault::Envelope)
            .map_err(|_| invalid())
    } else {
        Err(invalid())
    }
}

/// Fresh randomness for sealing a new container, drawn at the boundary and passed in.
pub struct Randomness {
    pub dek_iv: [u8; 12],
    pub dek_commit: [u8; 16],
    pub payload_iv: [u8; 12],
    pub payload_commit: [u8; 16],
}

impl Randomness {
    pub fn draw() -> Result<Self, Error> {
        Ok(Self {
            dek_iv: random()?,
            dek_commit: random()?,
            payload_iv: random()?,
            payload_commit: random()?,
        })
    }
}

/// A fresh DEK from the OS CSPRNG.
pub fn new_dek() -> Result<Dek, Error> {
    Ok(Zeroizing::new(random::<32>()?))
}

/// Additional data for the biometric slot. It binds the stored enrolment `domain` hash as well as
/// the slot id, so an edit to `domain` (for example blanking it to skip the enrolment-change
/// check) makes the slot fail to open instead of silently weakening reconciliation.
fn biometric_aad(lineage: &str, id: &str, domain: &str) -> Vec<u8> {
    additional_data(BIOMETRIC_SCOPE, lineage, &format!("{id}\0{domain}"))
}

/// SHA-256 hex of a host biometric domain state, as stored in the slot.
pub fn domain_hash(state: &[u8]) -> String {
    Sha256::digest(state)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn seal_payload(
    dek: &[u8; 32],
    lineage: &str,
    plaintext: &[u8],
    iv: &[u8; 12],
    commit: &[u8; 16],
) -> Result<SealedPayload, Error> {
    let commit = BASE64.encode(commit);
    let aad = additional_data(PAYLOAD_SCOPE, lineage, &commit);
    Ok(SealedPayload {
        scope: PAYLOAD_SCOPE.into(),
        iv: BASE64.encode(iv),
        commit,
        data: BASE64.encode(aead_seal(dek, iv, &aad, plaintext)?),
    })
}

pub fn decode_dek(plaintext: &[u8]) -> Result<Dek, Error> {
    let record: DekRecord = serde_json::from_slice(plaintext).map_err(|_| invalid())?;
    let bytes = Zeroizing::new(BASE64.decode(&record.dek).map_err(|_| invalid())?);
    let mut dek = Zeroizing::new([0u8; 32]);
    if bytes.len() != 32 {
        return Err(invalid());
    }
    dek.copy_from_slice(&bytes);
    Ok(dek)
}

/// Builds a `version` file: the DEK sealed under `key` (the passphrase slot) and the serialised
/// `payload` under the DEK.
pub fn seal_new(
    key: &SealingKey,
    dek: &[u8; 32],
    version: u32,
    payload: &[u8],
    randomness: &Randomness,
) -> Result<VaultFile, Error> {
    let record = DekRecord {
        dek: BASE64.encode(dek),
    };
    let record_json =
        Zeroizing::new(serde_json::to_vec(&record).map_err(|_| Error::storage("serialise key"))?);
    let lineage = key.lineage().to_string();
    let password_slot = key.seal_with(
        &record_json,
        DEK_SCOPE,
        &randomness.dek_iv,
        &randomness.dek_commit,
    )?;
    let payload = seal_payload(
        dek,
        &lineage,
        payload,
        &randomness.payload_iv,
        &randomness.payload_commit,
    )?;
    Ok(VaultFile {
        format: FORMAT.into(),
        version,
        lineage,
        password_slot,
        biometric_slot: None,
        payload,
    })
}

/// Reads and classifies the vault stored under `key`; `None` when there is none.
pub fn read(
    storage: &dyn Storage,
    key: &str,
    current_version: u32,
) -> Result<Option<StoredVault>, Error> {
    let Some(bytes) = storage.read(key)? else {
        return Ok(None);
    };
    let text = std::str::from_utf8(&bytes).map_err(|_| invalid())?;
    parse(text, current_version).map(Some)
}

/// Writes `file` under `key`. The host's `Storage::write` is atomic, so a crash leaves the old
/// file or the new one, never a torn one.
pub fn write(storage: &dyn Storage, key: &str, file: &VaultFile) -> Result<(), Error> {
    let body = serde_json::to_vec(file).map_err(|_| Error::storage("serialise vault"))?;
    storage.write(key, &body)
}

impl VaultFile {
    fn validate(&self, current_version: u32) -> Result<(), Error> {
        let slot_ok = self.biometric_slot.as_ref().is_none_or(|slot| {
            is_lower_hex(&slot.id, 32)
                && decode::<12>(&slot.iv).is_some()
                && decode::<48>(&slot.data).is_some()
                && (slot.domain.is_empty() || is_lower_hex(&slot.domain, 64))
        });
        let ok = self.format == FORMAT
            && (FIRST_CONTAINER_VERSION..=current_version).contains(&self.version)
            && is_lower_hex(&self.lineage, 32)
            && self.password_slot.lineage == self.lineage
            && self.password_slot.scope == DEK_SCOPE
            && self.payload.scope == PAYLOAD_SCOPE
            && decode::<12>(&self.payload.iv).is_some()
            && decode::<16>(&self.payload.commit).is_some()
            && slot_ok;
        if ok {
            Ok(())
        } else {
            Err(invalid())
        }
    }

    /// Runs the KDF on the passphrase slot. A wrong passphrase is `WrongPassword`.
    pub fn open_dek(&self, password: &str) -> Result<(Dek, SealingKey), Error> {
        let (plaintext, key) = envelope::open(&self.password_slot, password, DEK_SCOPE)?;
        Ok((decode_dek(&plaintext)?, key))
    }

    /// Opens the passphrase slot with an already-derived key (no KDF).
    pub fn open_dek_with(&self, key: &SealingKey) -> Result<Dek, Error> {
        decode_dek(&key.open_sealed(&self.password_slot, DEK_SCOPE)?)
    }

    /// Decrypts the payload bytes. The DEK is already proven, so a failure is tampering. The
    /// caller checks the payload's version against `self.version`.
    pub fn open_payload(&self, dek: &[u8; 32]) -> Result<Zeroizing<Vec<u8>>, Error> {
        let (Some(iv), Ok(data)) = (
            decode::<12>(&self.payload.iv),
            BASE64.decode(&self.payload.data),
        ) else {
            return Err(invalid());
        };
        let aad = additional_data(PAYLOAD_SCOPE, &self.lineage, &self.payload.commit);
        aead_open(dek, &iv, &aad, &data).ok_or_else(invalid)
    }

    /// The same file with a `version` `payload` re-sealed under a fresh IV and commit; no KDF,
    /// same DEK, same slots. Re-sealing an older container at the current version is how it is
    /// upgraded in place.
    ///
    /// Fails closed unless `dek` opens the current payload. The slots are kept, so a DEK they
    /// do not wrap would leave a file the passphrase (and biometric) unlock can no longer read.
    pub fn with_payload(
        &self,
        dek: &[u8; 32],
        version: u32,
        payload: &[u8],
    ) -> Result<Self, Error> {
        self.require_dek(dek)?;
        Ok(Self {
            version,
            payload: seal_payload(dek, &self.lineage, payload, &random()?, &random()?)?,
            ..self.clone()
        })
    }

    /// Fails closed unless `dek` is this file's DEK, i.e. it opens the current payload.
    fn require_dek(&self, dek: &[u8; 32]) -> Result<(), Error> {
        self.open_payload(dek)
            .map(drop)
            .map_err(|_| Error::invalid("data key does not open this vault"))
    }

    /// A biometric slot wrapping `dek` under the host's 32-byte wrap secret. Fails closed unless
    /// `dek` opens the current payload: a slot wrapping any other key would unlock nothing.
    pub fn seal_biometric(
        &self,
        dek: &[u8; 32],
        wrap_secret: &[u8; 32],
        domain: String,
    ) -> Result<BiometricSlot, Error> {
        self.require_dek(dek)?;
        let id = random_hex::<16>()?;
        let iv = random::<12>()?;
        let aad = biometric_aad(&self.lineage, &id, &domain);
        Ok(BiometricSlot {
            data: BASE64.encode(aead_seal(wrap_secret, &iv, &aad, dek.as_slice())?),
            iv: BASE64.encode(iv),
            id,
            domain,
        })
    }

    /// Unwraps the DEK with the host's wrap secret. `BiometricUnavailable` without a slot;
    /// `BiometricInvalidated` when the secret does not open it (stale or tampered slot).
    pub fn open_biometric(&self, wrap_secret: &[u8; 32]) -> Result<Dek, Error> {
        let slot = self
            .biometric_slot
            .as_ref()
            .ok_or(Error::BiometricUnavailable)?;
        let (Some(iv), Some(data)) = (decode::<12>(&slot.iv), decode::<48>(&slot.data)) else {
            return Err(Error::BiometricInvalidated);
        };
        let aad = biometric_aad(&self.lineage, &slot.id, &slot.domain);
        let plaintext =
            aead_open(wrap_secret, &iv, &aad, &data).ok_or(Error::BiometricInvalidated)?;
        let mut dek = Zeroizing::new([0u8; 32]);
        if plaintext.len() != 32 {
            return Err(Error::BiometricInvalidated);
        }
        dek.copy_from_slice(&plaintext);
        Ok(dek)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_container_seals_the_current_version_and_opens_only_under_its_dek() {
        let key = SealingKey::from_parts("fixture passphrase", [1; 16], &"ef".repeat(16));
        let dek = [3u8; 32];
        let file = seal_new(&key, &dek, 5, b"{}", &Randomness::draw().unwrap()).unwrap();
        assert_eq!(file.version, 5);
        assert_eq!(file.open_payload(&dek).unwrap().as_slice(), b"{}");
        let (opened, reopened) = file.open_dek("fixture passphrase").unwrap();
        assert_eq!(*opened, dek);
        assert_eq!(*file.open_dek_with(&reopened).unwrap(), dek);
        assert!(matches!(
            file.open_dek("wrong passphrase"),
            Err(Error::WrongPassword)
        ));
        assert!(file.open_payload(&[4u8; 32]).is_err());
    }

    #[test]
    fn a_payload_is_resealed_only_under_the_dek_the_slots_wrap() {
        let key = SealingKey::from_parts("fixture passphrase", [1; 16], &"ef".repeat(16));
        let dek = [3u8; 32];
        let file = seal_new(&key, &dek, 4, b"{}", &Randomness::draw().unwrap()).unwrap();
        assert!(matches!(
            file.with_payload(&[4u8; 32], 5, b"{}"),
            Err(Error::InvalidInput { .. })
        ));
        assert!(matches!(
            file.seal_biometric(&[4u8; 32], &[9; 32], String::new()),
            Err(Error::InvalidInput { .. })
        ));
        let upgraded = file.with_payload(&dek, 5, b"[]").unwrap();
        let (opened, _) = upgraded.open_dek("fixture passphrase").unwrap();
        assert_eq!(upgraded.open_payload(&opened).unwrap().as_slice(), b"[]");
    }

    #[test]
    fn only_known_container_versions_parse() {
        let key = SealingKey::from_parts("fixture passphrase", [1; 16], &"ef".repeat(16));
        let file = seal_new(&key, &[3u8; 32], 5, b"{}", &Randomness::draw().unwrap()).unwrap();
        for (version, ok) in [
            (1, false),
            (2, true),
            (3, true),
            (4, true),
            (5, true),
            (6, false),
        ] {
            let text = serde_json::to_string(&VaultFile {
                version,
                ..file.clone()
            })
            .unwrap();
            assert_eq!(parse(&text, 5).is_ok(), ok, "container version {version}");
        }
    }

    #[test]
    fn the_biometric_slot_is_bound_to_its_lineage_and_id() {
        let key = SealingKey::from_parts("fixture passphrase", [1; 16], &"ef".repeat(16));
        let dek = [3u8; 32];
        let mut file = seal_new(&key, &dek, 5, b"{}", &Randomness::draw().unwrap()).unwrap();
        let slot = file.seal_biometric(&dek, &[9; 32], String::new()).unwrap();
        file.biometric_slot = Some(slot.clone());
        assert_eq!(*file.open_biometric(&[9; 32]).unwrap(), dek);
        let mut renamed = file.clone();
        renamed.biometric_slot = Some(BiometricSlot {
            id: "00".repeat(16),
            ..slot
        });
        assert!(matches!(
            renamed.open_biometric(&[9; 32]),
            Err(Error::BiometricInvalidated)
        ));
        let relined = VaultFile {
            lineage: "00".repeat(16),
            ..file
        };
        assert!(matches!(
            relined.open_biometric(&[9; 32]),
            Err(Error::BiometricInvalidated)
        ));
    }

    #[test]
    fn a_container_round_trips_through_storage() {
        let storage = crate::host::testing::Memory::default();
        assert!(read(&storage, "vault.json", 5).unwrap().is_none());
        let key = SealingKey::from_parts("fixture passphrase", [1; 16], &"ef".repeat(16));
        let file = seal_new(&key, &[3u8; 32], 5, b"{}", &Randomness::draw().unwrap()).unwrap();
        write(&storage, "vault.json", &file).unwrap();
        let Some(StoredVault::Container(stored)) = read(&storage, "vault.json", 5).unwrap() else {
            panic!("expected a container");
        };
        assert_eq!(*stored, file);
        assert!(read(&storage, "vault.json", 4).is_err());
    }
}
