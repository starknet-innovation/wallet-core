//! A typed-data approval's live account read (`executeSignApproval`'s
//! `requireLiveAuthorizableClass`): a Ready (Argent) or Braavos account signs only once the chain
//! shows it runs a class that verifies this wallet's signature, with this wallet's key as its
//! signer (and, for Braavos, no 2FA still active); OpenZeppelin signs at once. The reads and
//! verdicts are the send review's (`Kind::Authorize`), so an upgraded Ready account stored at
//! 0.3.x signs, and one still running 0.3.x does not.
//!
//! The host makes each read, one at a time (`DappSignStep::Read`, then `answer_sign_read`). After
//! the account passes, every check of `finish_sign` runs again under one session hold before
//! the key is used; a refused read answers the pages UNKNOWN_ERROR (163).

use starknet_wallet_core::send::plan::{Kind, Plan};
use starknet_wallet_core::send::plan_authorize::AuthorizeStep;
use starknet_wallet_core::send::types::{Authorization, SendStep, StepReply};

use crate::dapp_chain::refused;
use crate::dapp_session::{settled, Card, DappSession, DappSettled, Waiter};
use crate::dapp_sign::SignCard;
use crate::networks::NetworkId;
use crate::send::plan::SEND_POLICY;
use crate::send::wallet_ext::Use;
use crate::wallet::guard;

/// What a typed-data approval needs next.
#[derive(Debug, uniffi::Enum)]
pub enum DappSignStep {
    /// POST `body` to an endpoint for `network`, and pass its reply to `answer_sign_read` (an
    /// empty body when the POST failed).
    Read {
        network: NetworkId,
        id: u64,
        body: String,
    },
    /// The pages' answers: the signature, or the refusal.
    Settled { settled: Vec<DappSettled> },
}

/// An approved typed-data card waiting on its account's live read.
pub(crate) struct PendingSign {
    origin: String,
    card: SignCard,
    pub(crate) waiters: Vec<Waiter>,
    reviewed_account_id: String,
    plan: Plan,
}

fn done(settled: Vec<DappSettled>) -> DappSignStep {
    DappSignStep::Settled { settled }
}

impl DappSession {
    /// Takes one step of `sign`'s read: another read for the host, or the pages' answers.
    fn step_sign(
        &self,
        approval_id: String,
        mut sign: PendingSign,
        reply: StepReply,
    ) -> DappSignStep {
        match sign.plan.authorize(reply) {
            AuthorizeStep::Read(SendStep::Rpc { id, body, .. }) => {
                let network = sign.card.network;
                guard(&self.state).signing.insert(approval_id, sign);
                DappSignStep::Read { network, id, body }
            }
            AuthorizeStep::Authorized => done(self.finish_sign(
                &sign.origin,
                &sign.card,
                sign.waiters,
                &sign.reviewed_account_id,
                true,
            )),
            AuthorizeStep::Read(_) | AuthorizeStep::Refused(_) => {
                done(settled(sign.waiters, |_| refused()))
            }
        }
    }
}

#[uniffi::export]
impl DappSession {
    /// The person approved `approval_id`'s typed-data card, reviewed for `reviewed_account_id`
    /// (the card's `account_id`, echoed). An OpenZeppelin account signs at once; a Ready or
    /// Braavos account first has its live state read (`Read`, answered by `answer_sign_read`).
    /// Signs only while that account is still the active one granted to the site, on the network
    /// it was reviewed on, with the site not blocked and the wallet unlocked, and answers the
    /// request with the signature array; otherwise the request is refused. Either way the card is
    /// spent. A connect card, or a card already settled or expired, answers nothing. Renews.
    /// `now_ms` is the host's wall clock.
    pub fn approve_sign_typed_data(
        &self,
        approval_id: String,
        reviewed_account_id: String,
        now_ms: f64,
    ) -> DappSignStep {
        let Some((origin, Card::Sign(card), waiters)) =
            self.claim(&approval_id, |card| matches!(card, Card::Sign(_)))
        else {
            return done(Vec::new());
        };
        self.record_seen(&origin, now_ms);
        // A Ledger account is refused before any read: typed data is not wired to the device.
        let acting = match self.wallet.plan_account_for(&card.account_id, Use::Sign) {
            Ok(acting) if acting.device.is_none() => acting,
            _ => return done(settled(waiters, |_| refused())),
        };
        if matches!(acting.account.authorization, Authorization::StarkPair) {
            return done(self.finish_sign(&origin, &card, waiters, &reviewed_account_id, false));
        }
        let sign = PendingSign {
            origin,
            card,
            waiters,
            reviewed_account_id,
            plan: Plan::new(acting.account, Kind::Authorize, SEND_POLICY),
        };
        self.step_sign(approval_id, sign, StepReply::Start)
    }

    /// The reply to `approval_id`'s read `id` (`body` empty when the POST failed): the next read,
    /// or the pages' answers. An approval with no read outstanding answers nothing.
    pub fn answer_sign_read(&self, approval_id: String, id: u64, body: String) -> DappSignStep {
        let Some(sign) = guard(&self.state).signing.remove(&approval_id) else {
            return done(Vec::new());
        };
        let now_ms = self.wallet.now().monotonic_ms;
        let reply = if body.is_empty() {
            StepReply::TransportFailure { id, now_ms }
        } else {
            StepReply::Rpc { id, body, now_ms }
        };
        self.step_sign(approval_id, sign, reply)
    }
}
