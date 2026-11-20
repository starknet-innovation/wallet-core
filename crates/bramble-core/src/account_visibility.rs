//! Hidden accounts, as the extension keeps them: a preference (`hiddenAccountIds`, in
//! `public_store`'s preferences file) that leaves an account out of the host's lists and changes
//! nothing in the vault, so a hidden account keeps its key and can still be selected and sign.
//!
//! The extension's background refuses the two states its lists cannot render
//! (`account-visibility-ops.ts`): hiding the last account a network still shows, and hiding the
//! active account without moving to another one on that network first.

use crate::account_store::{StoredAccount, VaultPayload};
use crate::networks::NetworkId;
use crate::wallet::WalletCore;
use crate::CoreError;

/// The first account on `network` that is not hidden and is not `except`.
pub(crate) fn first_visible<'a>(
    payload: &'a VaultPayload,
    network: NetworkId,
    except: Option<&str>,
) -> Option<&'a StoredAccount> {
    payload.accounts.iter().find(|record| {
        record.network() == network
            && Some(record.id.as_str()) != except
            && !payload.hidden_account_ids.contains(&record.id)
    })
}

fn invalid(reason: &str) -> CoreError {
    CoreError::InvalidInput {
        reason: reason.into(),
    }
}

#[uniffi::export]
impl WalletCore {
    /// Hides or reveals one account. Hiding refuses an unknown id and the last account its
    /// network still shows; hiding the active account first makes the network's first visible
    /// account active (a vault write), then records the hide (a preferences write), so a failure
    /// between the two leaves the wallet on a visible account. Revealing an id that is not hidden
    /// is a no-op. Passive: never renews the idle window.
    pub fn set_account_hidden(&self, account_id: String, hidden: bool) -> Result<(), CoreError> {
        self.with_session(false, |session| {
            if !hidden {
                return self.commit(session, |payload| {
                    payload.hidden_account_ids.retain(|id| *id != account_id);
                    Ok(())
                });
            }
            let payload = &session.payload;
            let target = payload
                .record(&account_id)
                .ok_or_else(|| invalid("unknown account"))?;
            let successor = first_visible(payload, target.network(), Some(&account_id))
                .ok_or_else(|| invalid("can't hide the last account on this network"))?
                .id
                .clone();
            if payload.active_account_id.as_deref() == Some(account_id.as_str()) {
                self.commit(session, |payload| {
                    payload.active_account_id = Some(successor);
                    Ok(())
                })?;
            }
            self.commit(session, |payload| {
                if !payload.hidden_account_ids.contains(&account_id) {
                    payload.hidden_account_ids.push(account_id);
                }
                Ok(())
            })
        })
    }
}
