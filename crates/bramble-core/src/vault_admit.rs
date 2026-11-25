//! Which of a vault's records mobile operates. Mobile opens every vault the extension opens
//! (`vault_rules`), and operates a record only when it re-derives from a key mobile can use; every
//! other record is kept unused (`vault_wire::Kept`), never refuses the vault.
//!
//! A phrase record is operated when its record, public key, y-coordinate and sealed key are what
//! the phrase derives at its coordinate. A Ready (`argentx-bip44`) record whose sealed key an
//! earlier Krusty derived (`ready-key-drift.ts`: Krusty 2.0 and earlier kept the ETH key's
//! leading zero bytes, about 1 phrase in 256) is operated with that sealed key, as the extension
//! keeps signing with it, when the record is what the sealed key derives. An imported record is
//! operated with its sealed key on the same condition, and a Ledger record, which holds none, when
//! its stored public key places it: it signs only on its device. Anything else (a record at a coordinate,
//! class or index mobile does not derive, one whose address or key does not match, or past
//! `MAX_ACCOUNT_RECORDS`) is kept: listed as not usable here, never signed with.

use wallet_core::SecretFelt;

use crate::account_catalog::AccountFamily;
use crate::account_store::{record_at, KeySource, StoredAccount};
use crate::account_validate::is_well_formed;
use crate::kms::Ladder;
use crate::vault_wire::SigningKey;
use crate::{felt, CoreError};

/// What `admit` makes of one record mobile could operate.
pub(crate) enum Admission {
    /// Operated; its key is derived from the phrase in each call.
    Derived,
    /// Operated with the key the vault sealed for it, which the session holds.
    Held(Box<SecretFelt>),
    /// Operated on its Ledger: the vault holds no key for it.
    Device,
    /// Kept unused.
    Kept,
}

/// Whether `record` is what `public_key` places under its family, class, salt and network.
fn placed_by(record: &StoredAccount, public_key: &str) -> bool {
    let Some(public_key) = felt::parse_non_zero(public_key) else {
        return false;
    };
    let family = record.family();
    let class_hash = (family != AccountFamily::OpenZeppelin).then(|| record.class_felt());
    record_at(
        family,
        record.key,
        record.account_index,
        class_hash.as_ref(),
        record.salt_felt().as_ref(),
        &public_key,
        record.network(),
    )
    .is_ok_and(|expected| expected == *record)
}

/// The sealed key, when it is canonical, a usable scalar, and places `record`.
fn held(record: &StoredAccount, sealed: &SigningKey) -> Result<Option<Box<SecretFelt>>, CoreError> {
    let Some(key) = felt::parse_sealed_private_key(&sealed.private_key) else {
        return Ok(None);
    };
    let public_key = wallet_core::stark_public_key(&key)?;
    Ok(placed_by(record, &public_key).then(|| Box::new(key)))
}

/// Decides one record: `mnemonic` is the vault's phrase (`None` in a Ledger-only vault), `keys`
/// its `signingKeys`, `public_key_y` the record's own.
pub(crate) fn admit(
    mnemonic: Option<&str>,
    record: &StoredAccount,
    keys: &[SigningKey],
    public_key_y: Option<&String>,
) -> Result<Admission, CoreError> {
    if !is_well_formed(record) {
        return Ok(Admission::Kept);
    }
    // A Ledger record holds no key (`vault_rules` already refuses one bound to it): operated when
    // its own stored key places it, so the device the plan asks for is the address's.
    if let KeySource::Ledger(_) = record.key {
        let unsealed = keys.iter().all(|key| key.account_id != record.id);
        return Ok(if unsealed && placed_by(record, &record.public_key) {
            Admission::Device
        } else {
            Admission::Kept
        });
    }
    let mut sealed = keys.iter().filter(|key| key.account_id == record.id);
    let (Some(sealed), None) = (sealed.next(), sealed.next()) else {
        return Ok(Admission::Kept);
    };
    let ladder = match record.key {
        KeySource::Phrase(ladder) => ladder,
        // An imported key is x-only; `vault_rules` already refuses a y-coordinate on its record.
        KeySource::Imported => {
            return Ok(held(record, sealed)?.map_or(Admission::Kept, Admission::Held));
        }
        KeySource::Ledger(_) => return Ok(Admission::Kept),
    };
    // A phrase record in a vault with no phrase derives from nothing here.
    let Some(mnemonic) = mnemonic else {
        return Ok(Admission::Kept);
    };
    let (pair, derived_y) =
        wallet_core::account_keypair(mnemonic, None, ladder, record.account_index)?;
    if placed_by(record, &pair.public_key_x)
        && *sealed.private_key == *pair.private_key.expose_secret_hex()
        && public_key_y == derived_y.as_ref()
    {
        return Ok(Admission::Derived);
    }
    // Only Ready's derivation changed under an earlier Krusty.
    if ladder == Ladder::ArgentX {
        if let Some(key) = held(record, sealed)? {
            return Ok(Admission::Held(key));
        }
    }
    Ok(Admission::Kept)
}
