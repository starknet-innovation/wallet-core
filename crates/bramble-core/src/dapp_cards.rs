//! The cards `DappSession` holds for the person: listing them with their sites' warnings, and
//! declining one of any kind.

use starknet_wallet_core::dapp::connect::deny_connect;
use starknet_wallet_core::js::json::JsValue;

use crate::dapp_session::{
    error, result, settled, Card, DappApproval, DappCard, DappSession, DappSettled,
};
use crate::dapp_watch::WatchCard;

#[uniffi::export]
impl DappSession {
    /// The cards waiting for the person, oldest first, each with its site's warnings as of
    /// `now_ms` (the host's wall clock).
    pub fn pending_approvals(&self, now_ms: f64) -> Vec<DappApproval> {
        let listed: Vec<(String, String, DappCard)> = {
            let mut state = crate::wallet::guard(&self.state);
            let now = self.wallet.now().monotonic_ms;
            state
                .queue
                .list(now)
                .into_iter()
                .map(|approval| {
                    let card = match &approval.payload {
                        Card::Connect => DappCard::Connect,
                        Card::Sign(sign) => sign.card(),
                        Card::SwitchChain(network) => DappCard::SwitchChain { network: *network },
                        Card::WatchAsset(card) => DappCard::WatchAsset {
                            token: card.token.clone(),
                            already_tracked: card.already_tracked,
                        },
                    };
                    (
                        approval.id.as_str().to_owned(),
                        approval.origin.clone(),
                        card,
                    )
                })
                .collect()
        };
        listed
            .into_iter()
            .map(|(approval_id, origin, card)| {
                let insight = self.insight(&origin, now_ms);
                DappApproval {
                    approval_id,
                    origin,
                    blocked: insight.blocked,
                    warnings: insight.warnings,
                    card,
                }
            })
            .collect()
    }

    /// The person declined `approval_id`'s card, of any kind: every request waiting on it is
    /// told so (USER_REFUSED_OP).
    pub fn deny(&self, approval_id: String, now_ms: f64) -> Vec<DappSettled> {
        let Some((origin, card, waiters)) = self.claim(&approval_id, |_| true) else {
            return Vec::new();
        };
        self.record_seen(&origin, now_ms);
        // Closing an already-tracked notice answers as tracked, so a site cannot tell a notice
        // from a card the person approved (audit M1).
        if let Card::WatchAsset(WatchCard {
            already_tracked: true,
            ..
        }) = card
        {
            return settled(waiters, |_| result(&JsValue::Bool(true)));
        }
        settled(waiters, |_| error(deny_connect()))
    }
}
