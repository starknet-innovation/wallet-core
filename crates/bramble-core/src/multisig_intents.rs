//! The signing-intent store (`multisig-intents.json`): the extension's multisig repository's
//! `intents` (`packages/features/multisig/src/repository.ts`, `intent-capacity.ts`). Every intent
//! is one `isArgentMultisigSigningIntent` accepts, a write replaces a stored one only as
//! `canReplaceArgentMultisigSigningIntent` allows (`upsertIntentIn`), and a new one is placed as
//! `intentPlacement` places it: at most `MAX_INTENTS` in all and `MAX_INTENTS_PER_MULTISIG` for
//! one account, evicting only a settled intent. Public data: hashes, calls and signatures.

use std::path::Path;
use std::sync::{Mutex, PoisonError};

use serde::{Deserialize, Serialize};
use wallet_core::send::multisig_intent::{IntentState, SigningIntent};

use crate::{multisig_constants, public_store, CoreError};

/// `MAX_INTENTS`.
pub const MAX_INTENTS: usize = 100;
/// `MAX_INTENTS_PER_MULTISIG`.
pub const MAX_INTENTS_PER_MULTISIG: usize = 25;

/// Serialises every read-modify-write of the file, so two operations on one intent cannot both
/// pass `can_replace` against the same stored version.
static WRITE: Mutex<()> = Mutex::new(());

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    version: u32,
    intents: Vec<SigningIntent>,
}

fn unreadable() -> CoreError {
    CoreError::Storage {
        reason: format!("{} unreadable", public_store::MULTISIG_INTENTS),
    }
}

fn refused(outcome: &str) -> CoreError {
    CoreError::InvalidInput {
        reason: format!("Argent multisig intent {outcome}"),
    }
}

/// `isArgentMultisigSigningIntent` on Bramble's pinned class and summary length.
pub fn is_valid(intent: &SigningIntent) -> bool {
    intent.is_valid(
        multisig_constants::argent_multisig_class(),
        multisig_constants::get().limits.summary_length,
    )
}

/// Every stored intent, oldest first; a missing file is empty, an invalid one fails closed.
pub fn load(dir: &Path) -> Result<Vec<SigningIntent>, CoreError> {
    let Some(file) = public_store::read::<File>(dir, public_store::MULTISIG_INTENTS)? else {
        return Ok(Vec::new());
    };
    let intents = file.intents;
    let unique = intents
        .iter()
        .enumerate()
        .all(|(index, intent)| intents[..index].iter().all(|other| other.id != intent.id));
    if file.version != 1 || intents.len() > MAX_INTENTS || !unique || !intents.iter().all(is_valid)
    {
        return Err(unreadable());
    }
    Ok(intents)
}

/// The stored intent `id`.
pub fn get(dir: &Path, id: &str) -> Result<Option<SigningIntent>, CoreError> {
    Ok(load(dir)?.into_iter().find(|intent| intent.id == id))
}

/// `isTerminalIntent`: settled, so it may be evicted to make room.
fn is_terminal(intent: &SigningIntent) -> bool {
    matches!(
        intent.state,
        IntentState::Accepted | IntentState::Rejected | IntentState::Stale
    )
}

/// `intentPlacement`: `Ok(None)` when it fits, `Ok(Some(index))` once that settled intent goes.
fn placement(intents: &[SigningIntent], account_id: &str) -> Result<Option<usize>, CoreError> {
    let same = |intent: &SigningIntent| intent.account_id == account_id;
    let evict = |keep: &dyn Fn(&SigningIntent) -> bool| {
        intents
            .iter()
            .position(|intent| keep(intent) && is_terminal(intent))
            .map(Some)
            .ok_or_else(|| refused("store full"))
    };
    if intents.iter().filter(|intent| same(intent)).count() >= MAX_INTENTS_PER_MULTISIG {
        return evict(&same);
    }
    if intents.len() >= MAX_INTENTS {
        return evict(&|_| true);
    }
    Ok(None)
}

/// `upsertIntentIn`: stores `next` if it is valid and, when an intent with its id is stored,
/// may replace it; else refuses ("invalid", "conflict", "store full").
pub fn upsert(dir: &Path, next: &SigningIntent) -> Result<(), CoreError> {
    let _write = WRITE.lock().unwrap_or_else(PoisonError::into_inner);
    if !is_valid(next) {
        return Err(refused("invalid"));
    }
    let mut intents = load(dir)?;
    match intents.iter().position(|intent| intent.id == next.id) {
        Some(index) => {
            let replaceable = intents[index].can_replace(
                next,
                multisig_constants::argent_multisig_class(),
                multisig_constants::get().limits.summary_length,
            );
            if !replaceable {
                return Err(refused("conflict"));
            }
            intents[index] = next.clone();
        }
        None => {
            if let Some(index) = placement(&intents, &next.account_id)? {
                intents.remove(index);
            }
            intents.push(next.clone());
        }
    }
    write(dir, intents)
}

/// Drops every intent of a removed multisig (`removeMultisig`).
pub fn remove_account(dir: &Path, account_id: &str) -> Result<(), CoreError> {
    let _write = WRITE.lock().unwrap_or_else(PoisonError::into_inner);
    let mut intents = load(dir)?;
    intents.retain(|intent| intent.account_id != account_id);
    write(dir, intents)
}

fn write(dir: &Path, intents: Vec<SigningIntent>) -> Result<(), CoreError> {
    public_store::write(
        dir,
        public_store::MULTISIG_INTENTS,
        &File {
            version: 1,
            intents,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Sepolia proposal's stored intents from G5: collecting with one signature, and ready.
    fn stages() -> (SigningIntent, SigningIntent) {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/extension-multisig-transactions.json"
        ))
        .unwrap();
        let stages = &fixture["networks"][0]["proposal"]["stages"];
        let intent = |stage: &serde_json::Value| {
            serde_json::from_str(stage["intent"].as_str().unwrap()).unwrap()
        };
        (intent(&stages[1]), intent(&stages[2]))
    }

    #[test]
    fn an_invalid_intent_is_never_stored() {
        let dir = tempfile::tempdir().unwrap();
        let (_, ready) = stages();
        let lying = SigningIntent {
            summary: " padded ".into(),
            ..ready
        };
        assert!(upsert(dir.path(), &lying).is_err());
        assert!(load(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn a_stored_intent_is_replaced_only_as_the_lifecycle_allows() {
        let dir = tempfile::tempdir().unwrap();
        let (collecting, ready) = stages();
        upsert(dir.path(), &collecting).unwrap();
        upsert(dir.path(), &ready).unwrap();
        // Dropping a signature (ready back to collecting) is a conflict.
        assert!(upsert(dir.path(), &collecting).is_err());
        assert_eq!(load(dir.path()).unwrap(), [ready]);
    }

    #[test]
    fn a_full_multisig_evicts_only_its_own_settled_intents() {
        let (open, _) = stages();
        let numbered = |n: usize, account: &str, state: IntentState| SigningIntent {
            id: format!("{account}:{n}"),
            account_id: account.into(),
            state,
            ..open.clone()
        };
        let mut intents: Vec<_> = (0..MAX_INTENTS_PER_MULTISIG)
            .map(|n| numbered(n, "a", IntentState::Collecting))
            .collect();
        assert!(placement(&intents, "a").is_err());
        assert_eq!(placement(&intents, "b").unwrap(), None);
        intents[3].state = IntentState::Stale;
        assert_eq!(placement(&intents, "a").unwrap(), Some(3));
        let mut all: Vec<_> = (0..MAX_INTENTS)
            .map(|n| numbered(n, &format!("m{}", n % 5), IntentState::Ready))
            .collect();
        assert!(placement(&all, "new").is_err());
        all[7].state = IntentState::Rejected;
        assert_eq!(placement(&all, "new").unwrap(), Some(7));
    }
}
