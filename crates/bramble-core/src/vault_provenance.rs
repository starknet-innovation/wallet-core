//! Schema 14's one addition, `accountProvenance` (`schema/schema-provenance.ts` on the
//! extension's `release/v0.10.11`): an optional map from account id to how the account entered
//! the vault, `created` or `imported`. An account it does not name is unknown, and nothing ever
//! guesses one: the extension's 13-to-14 migration writes none, and neither does a restore from a
//! phrase. A v13 vault has no such key (`vault_rules::is_valid`).
//!
//! The extension keeps it outside the vault's JSON budget (`isVaultV14` sets it aside before
//! `isVaultV13`), so its size is bounded here instead: at most one entry per account record, and
//! the parse stops at mobile's record cap before it builds anything larger.

use std::collections::{BTreeMap, HashSet};
use std::fmt;

use serde::de::{Deserializer, Error, MapAccess, Visitor};
use serde::{Deserialize, Serialize};

use crate::account_store::{VaultPayload, MAX_ACCOUNT_RECORDS};
use crate::vault_v13::PROVENANCE_SCHEMA;
use crate::vault_wire::Account;

/// `AccountProvenance`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Origin {
    Created,
    Imported,
}

/// The vault's `accountProvenance`, by account id. Public data only.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub(crate) struct Provenance(BTreeMap<String, Origin>);

/// An object of `Origin`s, refused at its first repeated key (which `JSON.parse` would take, the
/// last one winning) and past `MAX_ACCOUNT_RECORDS` entries, which no vault mobile opens can name.
impl<'de> Deserialize<'de> for Provenance {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Entries;
        impl<'de> Visitor<'de> for Entries {
            type Value = Provenance;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("an object of account origins")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Provenance, A::Error> {
                let mut entries = BTreeMap::new();
                while let Some((id, origin)) = map.next_entry::<String, Origin>()? {
                    if entries.len() == MAX_ACCOUNT_RECORDS {
                        return Err(A::Error::custom("too many account origins"));
                    }
                    if entries.insert(id, origin).is_some() {
                        return Err(A::Error::custom("repeated account origin"));
                    }
                }
                Ok(Provenance(entries))
            }
        }
        deserializer.deserialize_map(Entries)
    }
}

/// `validAccountProvenance`: absent, or naming only accounts this vault holds (the values are
/// already `created` or `imported`, as typed). One pass over each list.
pub(crate) fn is_valid(provenance: Option<&Provenance>, accounts: &[Account]) -> bool {
    let Some(Provenance(entries)) = provenance else {
        return true;
    };
    let held: HashSet<&str> = accounts.iter().map(|account| account.id.as_str()).collect();
    entries.keys().all(|id| held.contains(id.as_str()))
}

impl VaultPayload {
    /// Records how `account_id`, just added, entered the vault, as the extension's service does
    /// at 14 (`created` for an account derived here, `imported` for a pasted key, the source's
    /// origin for a projection onto another network, nothing when that is unknown). A v13 vault
    /// records nothing.
    pub(crate) fn note_origin(&mut self, account_id: &str, origin: Option<Origin>) {
        let Some(origin) = origin else { return };
        if self.schema_version >= PROVENANCE_SCHEMA {
            self.account_provenance
                .get_or_insert_with(Provenance::default)
                .0
                .insert(account_id.into(), origin);
        }
    }

    /// The origin the vault records for `account_id`, if any.
    pub(crate) fn origin_of(&self, account_id: &str) -> Option<Origin> {
        self.account_provenance
            .as_ref()
            .and_then(|Provenance(entries)| entries.get(account_id).copied())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(count: usize) -> String {
        let body: Vec<String> = (1..=count)
            .map(|n| format!(r#""SN_MAIN:0x{n:x}":"created""#))
            .collect();
        format!("{{{}}}", body.join(","))
    }

    #[test]
    fn parses_only_an_object_of_known_origins() {
        let read: Provenance =
            serde_json::from_str(r#"{"SN_MAIN:0x1":"created","SN_MAIN:0x2":"imported"}"#).unwrap();
        assert_eq!(read.0.len(), 2);
        assert_eq!(
            serde_json::to_string(&read).unwrap(),
            r#"{"SN_MAIN:0x1":"created","SN_MAIN:0x2":"imported"}"#
        );
        assert_eq!(serde_json::from_str::<Provenance>("{}").unwrap().0.len(), 0);
        for refused in [
            "[]",
            "null",
            r#"{"SN_MAIN:0x1":"guessed"}"#,
            r#"{"SN_MAIN:0x1":"Created"}"#,
            r#"{"SN_MAIN:0x1":null}"#,
            r#"{"SN_MAIN:0x1":1}"#,
        ] {
            assert!(
                serde_json::from_str::<Provenance>(refused).is_err(),
                "{refused}"
            );
        }
    }

    /// `JSON.parse` keeps the last of two equal keys; mobile refuses the object, as serde refuses
    /// a repeated key everywhere else in the vault.
    #[test]
    fn a_repeated_key_is_refused() {
        let repeated = r#"{"SN_MAIN:0x1":"created","SN_MAIN:0x1":"created"}"#;
        assert!(serde_json::from_str::<Provenance>(repeated).is_err());
    }

    /// The parse stops at the record cap: no vault mobile opens holds more accounts to name.
    #[test]
    fn the_parse_stops_at_the_record_cap() {
        assert!(serde_json::from_str::<Provenance>(&entries(MAX_ACCOUNT_RECORDS)).is_ok());
        assert!(serde_json::from_str::<Provenance>(&entries(MAX_ACCOUNT_RECORDS + 1)).is_err());
    }
}
