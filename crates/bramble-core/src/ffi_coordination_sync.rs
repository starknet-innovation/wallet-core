//! A coordination sync past its relays (`ffi_coordination.rs`): opening the wraps and importing
//! every trusted package, as the extension's `recordOpenedWraps` and `importSharedIntent`
//! (`multisig-coordination-ops.ts`) do, one fresh `MultisigStateRead` per import.

use std::collections::VecDeque;
use std::sync::Arc;

use wallet_core::nostr::sync_window::{is_wrap_past_retry, retryable_synced_at};
use wallet_core::send::multisig_import::ImportError;
use wallet_core::send::multisig_intent::{IntentPackage, IntentState, SigningIntent};
use wallet_core::send::multisig_review::CallReviewError;

use crate::coordination::{self, Checkpoint};
use crate::ffi_coordination::{
    not_found, CoordinationSyncOutcome, CoordinationSyncStep, NostrSyncPlan, Stage,
};
use crate::multisig_rpc::MultisigStateRead;
use crate::multisig_store::Record;
use crate::multisig_tx_import::ImportRefusal;
use crate::networks::NetworkId;
use crate::nostr::NostrApplicationData;
use crate::portfolio::tokens::TokenPreferences;
use crate::{contacts, multisig_constants, multisig_intents, nostr, CoreError, WalletCore};

/// A trusted package waiting for its read.
struct Queued {
    event_id: String,
    created_at: u64,
    package: IntentPackage,
}

/// `recordOpenedWraps`' state, bound to the session it was opened in.
pub(crate) struct Importing {
    generation: u64,
    record: Record,
    network: NetworkId,
    tokens: Arc<TokenPreferences>,
    queue: VecDeque<Queued>,
    pending: Option<(Queued, Arc<MultisigStateRead>)>,
    receipted: Vec<String>,
    intent_ids: Vec<String>,
    oldest_retryable: Option<u64>,
    now: u64,
}

impl Importing {
    /// One wrap settled. A retryable wrap older than the first-sync lookback is final too
    /// (`isWrapPastRetry`); the rest hold `syncedAt` back to the oldest of them.
    fn note(&mut self, event_id: &str, created_at: u64, is_final: bool) {
        let lookbacks = coordination::sync_lookbacks();
        if is_final || is_wrap_past_retry(created_at, self.now as f64, lookbacks) {
            if !self.receipted.iter().any(|id| id == event_id) {
                self.receipted.push(event_id.to_string());
            }
        } else {
            let oldest = self.oldest_retryable.unwrap_or(created_at).min(created_at);
            self.oldest_retryable = Some(oldest);
        }
    }
}

/// What opening one wrap decided before any chain read.
enum Triage {
    Settled { is_final: bool },
    Import(Box<IntentPackage>),
}

/// `importSharedIntent` up to its import: the STRK20 key and authorization first, then
/// `trustedSharedIntent`. Multisig STRK20 is not ported (P15), so nothing here can accept a key
/// or an authorization: the extension leaves such a wrap for a later sync when it cannot (`!accept`,
/// `!store`), and finishes one from anyone but a saved contact on this network. The wrap's content
/// is never read, so no viewing key is parsed.
fn triage(opened: &NostrApplicationData, saved: &[contacts::Entry], record: &Record) -> Triage {
    let sender = opened.sender_public_key.as_str();
    if [
        coordination::MULTISIG_STRK20_KEY_IDENTIFIER,
        coordination::MULTISIG_STRK20_AUTHORIZATION_IDENTIFIER,
    ]
    .contains(&opened.identifier.as_str())
    {
        let later = nostr::is_public_key(sender)
            && coordination::is_saved_nostr_contact(saved, sender, &record.network_id);
        return Triage::Settled { is_final: !later };
    }
    trusted_package(opened, saved, record).map_or(Triage::Settled { is_final: true }, |package| {
        Triage::Import(Box::new(package))
    })
}

/// `trustedSharedIntent`: a valid package for this record from a saved contact on its network
/// whose Stark key is in the package's frozen signer set. Whether that set is the chain's is the
/// import's check.
fn trusted_package(
    opened: &NostrApplicationData,
    saved: &[contacts::Entry],
    record: &Record,
) -> Option<IntentPackage> {
    let sender = opened.sender_public_key.as_str();
    if opened.identifier != multisig_constants::get().coordination.identifier
        || !nostr::is_public_key(sender)
    {
        return None;
    }
    let package = IntentPackage::parse(
        &opened.content,
        multisig_constants::argent_multisig_class(),
        multisig_constants::get().limits.summary_length,
    )?;
    let intent = &package.intent;
    (intent.account_id == record.id
        && coordination::is_saved_signer(
            saved,
            sender,
            &record.network_id,
            &intent.config_snapshot.signer_guids,
        ))
    .then_some(package)
}

/// `sharedIntentHasUpdate`: the package is new here, or adds a signer to a collecting or ready
/// intent.
fn has_update(current: Option<&SigningIntent>, incoming: &SigningIntent) -> bool {
    let Some(current) = current else {
        return true;
    };
    matches!(current.state, IntentState::Collecting | IntentState::Ready)
        && incoming.signatures.iter().any(|signature| {
            !current
                .signatures
                .iter()
                .any(|stored| stored.public_key == signature.public_key)
        })
}

/// `FINAL_IMPORT_REFUSALS`, plus the call-safety refusals the extension throws
/// (`requireSafeArgentMultisigCalls`, the signer-change review), which it settles as final too.
/// Anything else (a read that did not finish, a call this wallet may accept once it trusts more,
/// a full store, storage) is opened again by a later sync.
fn is_final(refusal: &ImportRefusal) -> bool {
    match refusal {
        ImportRefusal::Import(
            ImportError::AuthorityUnavailable
            | ImportError::ReadForAnotherAccount
            | ImportError::Review(
                CallReviewError::MissingCurrentConfig
                | CallReviewError::PoisonedRecipient
                | CallReviewError::PoisonedSpender,
            ),
        ) => false,
        ImportRefusal::Import(_) => true,
        ImportRefusal::Wallet(CoreError::InvalidInput { reason }) => [
            "Argent multisig intent invalid",
            "Argent multisig intent conflict",
        ]
        .contains(&reason.as_str()),
        ImportRefusal::Wallet(_) => false,
    }
}

#[uniffi::export]
impl WalletCore {
    /// Opens the unseen candidates the relays returned and starts importing the trusted ones
    /// over the person's tracked `tokens` (the review's, as `import_multisig_intent`). A wrap
    /// that does not open is left for a later sync. Returns the first read to drive, or `Done`.
    /// A lock, or a new session, before `Done` stops the sync with `Locked` and records nothing.
    pub fn coordination_sync_open(
        &self,
        plan: Arc<NostrSyncPlan>,
        tokens: Arc<TokenPreferences>,
    ) -> Result<CoordinationSyncStep, CoreError> {
        let mut stage = plan.stage();
        if !matches!(*stage, Stage::Relays) {
            return Err(sync_refused("this sync was already opened"));
        }
        let candidates = plan.judge.candidates()?;
        let generation = self.session_generation().ok_or(CoreError::Locked)?;
        let record = self
            .multisig_record(&plan.record_id)
            .map_err(|_| not_found())?;
        let network = NetworkId::from_catalog_id(&record.network_id).ok_or_else(not_found)?;
        let saved = contacts::load(self.data_dir())?;
        let mut importing = Importing {
            generation,
            record,
            network,
            tokens,
            queue: VecDeque::new(),
            pending: None,
            receipted: Vec::new(),
            intent_ids: Vec::new(),
            oldest_retryable: None,
            now: plan.now,
        };
        for event in candidates
            .iter()
            .filter(|event| !plan.seen.contains(&event.id))
        {
            let Some(opened) = self.with_phrase(|mnemonic| Ok(nostr::open(mnemonic, event)))?
            else {
                continue;
            };
            match triage(&opened, &saved, &importing.record) {
                Triage::Settled { is_final } => {
                    importing.note(&event.id, event.created_at, is_final);
                }
                Triage::Import(package) => importing.queue.push_back(Queued {
                    event_id: event.id.clone(),
                    created_at: event.created_at,
                    package: *package,
                }),
            }
        }
        *stage = Stage::Importing(Box::new(importing));
        self.advance_sync(&plan, &mut stage)
    }

    /// Imports the package `read` was handed out for (`read` must be the one the last step
    /// returned, driven to its end) and returns the next step. One refused package never stops
    /// the sync: a final refusal is receipted, a passing one tried again by the next sync.
    pub fn coordination_sync_import(
        &self,
        plan: Arc<NostrSyncPlan>,
        read: Arc<MultisigStateRead>,
    ) -> Result<CoordinationSyncStep, CoreError> {
        let mut stage = plan.stage();
        let Stage::Importing(importing) = &mut *stage else {
            return Err(sync_refused("this sync is not importing"));
        };
        let Some((next, pending)) = importing.pending.take() else {
            return Err(sync_refused("this sync is not waiting for a read"));
        };
        if !Arc::ptr_eq(&pending, &read) {
            importing.pending = Some((next, pending));
            return Err(sync_refused("not the read this sync asked for"));
        }
        if let Err(error) = self.require_session(importing.generation) {
            *stage = Stage::Over;
            return Err(error);
        }
        // Removed while the host read: kept out for good, as at the queue.
        // ponytail: a removal landing between this check and `import_package`'s upsert still
        // stores the intent again; take one lock over both stores if that window matters.
        let intent = &next.package.intent;
        let removed = multisig_intents::get(self.data_dir(), &intent.id)
            .ok()
            .and_then(|current| self.kept_out(intent, current.as_ref()));
        match removed {
            Some(true) => importing.note(&next.event_id, next.created_at, true),
            None => importing.note(&next.event_id, next.created_at, false),
            Some(false) => match self.import_package(&next.package, &read, &importing.tokens) {
                Ok(intent) => {
                    if !importing.intent_ids.contains(&intent.id) {
                        importing.intent_ids.push(intent.id);
                    }
                    importing.note(&next.event_id, next.created_at, true);
                }
                Err(ImportRefusal::Wallet(CoreError::Locked)) => {
                    *stage = Stage::Over;
                    return Err(CoreError::Locked);
                }
                Err(refusal) => importing.note(&next.event_id, next.created_at, is_final(&refusal)),
            },
        }
        self.advance_sync(&plan, &mut stage)
    }
}

fn sync_refused(reason: &str) -> CoreError {
    CoreError::InvalidInput {
        reason: reason.into(),
    }
}

impl WalletCore {
    /// `isRemovedIntent` for an intent not stored (`current`): a relay can serve a removed
    /// intent again under any event id, so the removal itself keeps it out, for good. `None`
    /// when the coordination file is unreadable, which the next sync tries again.
    fn kept_out(&self, intent: &SigningIntent, current: Option<&SigningIntent>) -> Option<bool> {
        if current.is_some() {
            return Some(false);
        }
        let file = coordination::load(self.data_dir()).ok()?;
        Some(coordination::is_removed_intent(
            &file,
            &intent.account_id,
            &intent.id,
        ))
    }

    fn require_session(&self, generation: u64) -> Result<(), CoreError> {
        if self.session_generation() == Some(generation) {
            Ok(())
        } else {
            Err(CoreError::Locked)
        }
    }

    /// Settles queued packages that need no import, hands out the next read, or records the
    /// checkpoint and ends the sync.
    fn advance_sync(
        &self,
        plan: &NostrSyncPlan,
        stage: &mut Stage,
    ) -> Result<CoordinationSyncStep, CoreError> {
        let Stage::Importing(importing) = stage else {
            return Err(sync_refused("this sync is not importing"));
        };
        if let Err(error) = self.require_session(importing.generation) {
            *stage = Stage::Over;
            return Err(error);
        }
        while let Some(next) = importing.queue.pop_front() {
            let intent = &next.package.intent;
            let Ok(current) = multisig_intents::get(self.data_dir(), &intent.id) else {
                importing.note(&next.event_id, next.created_at, false);
                continue;
            };
            let Some(removed) = self.kept_out(intent, current.as_ref()) else {
                importing.note(&next.event_id, next.created_at, false);
                continue;
            };
            if removed || !has_update(current.as_ref(), intent) {
                importing.note(&next.event_id, next.created_at, true);
                continue;
            }
            let read = MultisigStateRead::new(importing.network, importing.record.address.clone())?;
            importing.pending = Some((next, read.clone()));
            return Ok(CoordinationSyncStep::Read { read });
        }
        let Stage::Importing(importing) = std::mem::replace(stage, Stage::Over) else {
            return Err(sync_refused("this sync is not importing"));
        };
        let Importing {
            record,
            receipted,
            intent_ids,
            oldest_retryable,
            now,
            ..
        } = *importing;
        let outcome = CoordinationSyncOutcome {
            received: receipted.len() as u32,
            imported: intent_ids.len() as u32,
            intent_ids,
        };
        // As the extension, a checkpoint write failure does not fail the sync.
        let synced_at = retryable_synced_at(
            i64::try_from(now).unwrap_or(i64::MAX),
            oldest_retryable.and_then(|at| i64::try_from(at).ok()),
            coordination::sync_lookbacks(),
        );
        let checkpoint = Checkpoint {
            account_id: record.id,
            event_ids: receipted,
            relay_urls: plan.judge.relay_urls(),
            synced_at: u64::try_from(synced_at).unwrap_or(0),
        };
        let _ = coordination::update(self.data_dir(), |file| {
            coordination::record_sync(file, checkpoint);
            file.held.clear();
        });
        Ok(CoordinationSyncStep::Done { outcome })
    }
}
