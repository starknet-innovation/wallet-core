//! Cards that end without the person's decision: expired or evicted ones, answered as a deny
//! (the extension's `DROPPED_APPROVAL_RESPONSE`), and ones whose page has gone, withdrawn
//! unanswered.

use starknet_wallet_core::dapp::{ApprovalId, WalletRpcError};

use crate::dapp_session::{error, DappSession, DappSettled};
use crate::wallet::guard;

#[uniffi::export]
impl DappSession {
    /// The requests whose cards expired or were evicted since the last call, each answered as a
    /// deny is (USER_REFUSED_OP, `DROPPED_APPROVAL_RESPONSE`): consent was never given. Call it
    /// on a timer while any request is held, and after every other call.
    pub fn settle_expired(&self) -> Vec<DappSettled> {
        let mut state = guard(&self.state);
        let now = self.wallet.now().monotonic_ms;
        state.queue.list(now);
        let dropped = state.queue.take_dropped();
        dropped
            .into_iter()
            .flat_map(|id| state.waiters.remove(id.as_str()).unwrap_or_default())
            .map(|waiter| DappSettled {
                request_id: waiter.request_id,
                reply: error(WalletRpcError::user_refused_op()),
            })
            .collect()
    }

    /// The page that made `request_ids` has gone (navigated away, reloaded or closed): those
    /// requests are dropped unanswered, and a card left with no request waiting on it is
    /// withdrawn, so a late approval can neither grant nor sign for a page that no longer asks.
    /// A card another page of the same site still waits on stays. Request ids must be unique
    /// across the host's pages.
    ///
    /// A mobile divergence: the extension keeps such a card until a decision or its expiry, and a
    /// late approval there still writes the grant.
    pub fn withdraw(&self, request_ids: Vec<u64>) {
        let mut state = guard(&self.state);
        let now = self.wallet.now().monotonic_ms;
        let mut emptied = Vec::new();
        for (id, waiters) in state.waiters.iter_mut() {
            waiters.retain(|waiter| !request_ids.contains(&waiter.request_id));
            if waiters.is_empty() {
                emptied.push(id.clone());
            }
        }
        for id in emptied {
            state.waiters.remove(&id);
            state.queue.claim(&ApprovalId::from_echo(&id), now);
        }
    }
}
