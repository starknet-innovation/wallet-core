//! The relay journal's half of the activity log (`recordRequiredRelaySignature`,
//! `relay-record.ts`): a gasless execution is filed as its row before it is posted, with its
//! signed message's hash in the row's `hash` and the execution's nonce and deadline in `relay`,
//! so a lost answer still leaves a pending row the person sees and the wallet guards a second
//! signature with. The relay's answer settles it: the row takes the transaction hash and becomes
//! an ordinary pending transfer, observed like any other. A row the relay never answered for
//! stays pending (it may have run) and stops guarding once its deadline has passed.

use super::store::ActivityLog;
use crate::felt;
use crate::history::activity_rules::relay_execution;
use crate::Felt;

impl ActivityLog<'_> {
    /// Turns the journal row for `message_hash` into the submitted transaction `transaction`:
    /// one row, never two. False (nothing written) when the scope holds no such row.
    pub fn settle_relay(
        &mut self,
        account_id: &str,
        message_hash: &Felt,
        transaction: &Felt,
    ) -> bool {
        let mut log = self.load_log();
        let Some(entry) = log.scopes.get_mut(account_id).and_then(|entries| {
            entries
                .iter_mut()
                .find(|e| e.relay.is_some() && felt::parse(&e.hash).as_ref() == Some(message_hash))
        }) else {
            return false;
        };
        entry.hash = felt::canonical(transaction);
        entry.relay = None;
        self.save_log(&log)
    }

    /// Whether the scope holds a journal row whose execution can still run at `now_seconds`: a
    /// second gasless signature for the account waits until it lapses.
    pub fn relay_pending(&mut self, account_id: &str, now_seconds: u64) -> bool {
        self.list(account_id).iter().any(|entry| {
            entry
                .relay
                .as_ref()
                .and_then(relay_execution)
                .is_some_and(|relay| now_seconds < relay.execute_before)
        })
    }
}

#[cfg(test)]
#[path = "relay_tests.rs"]
mod tests;
