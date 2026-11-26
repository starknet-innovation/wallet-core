//! A typed-data approval's live account read (`executeSignApproval`'s
//! `requireLiveAuthorizableClass`): a Ready (Argent) or Braavos account signs only once the chain
//! shows it runs a class that verifies this wallet's signature, with this wallet's key as its
//! signer (and, for Braavos, no 2FA still active); OpenZeppelin signs at once. The reads and
//! verdicts are the send review's (`Kind::Authorize`), so an upgraded Ready account stored at
//! 0.3.x signs, and one still running 0.3.x does not.
//!
//! The host makes each read, one at a time (`DappSignStep::Read`, then `answer_sign_read`). After
//! the account passes, every check of `finish_sign` runs again under one session hold before
//! the key is used; a refused read answers the pages UNKNOWN_ERROR (163). A Ledger account's
//! hash then goes to its device (`Device`, `dapp_sign_device.rs`).

use starknet_wallet_core::ledger::HashSigning;
use starknet_wallet_core::send::plan::{Kind, Plan};
use starknet_wallet_core::send::plan_authorize::AuthorizeStep;
use starknet_wallet_core::send::types::{Authorization, SendStep, StepReply};
use starknet_wallet_core::Felt;

use crate::dapp_chain::refused;
use crate::dapp_session::{settled, Card, DappSession, DappSettled};
use crate::dapp_sign::Approved;
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
    /// Send `apdu` to the Ledger account's device; pass its answer to `answer_sign_device`, or
    /// what the transport reported to `sign_device_transport_failure`.
    Device { apdu: Vec<u8> },
    /// The device refused (`reason` is the extension's rejection code: `device-locked`,
    /// `wrong-device`, ...). Nothing was signed. Call `retry_sign_device` once the person has
    /// fixed it, or `cancel_sign_device` to refuse the pages; the card stays claimed until one
    /// of them, or until its page goes (`withdraw`).
    DeviceRetry { reason: String },
    /// The pages' answers: the signature, or the refusal.
    Settled { settled: Vec<DappSettled> },
}

/// An approved typed-data card waiting on the host: its account's live read, or its device.
pub(crate) struct PendingSign {
    pub(crate) approved: Approved,
    pub(crate) stage: SignStage,
}

pub(crate) enum SignStage {
    Read(Box<Plan>),
    /// The hash on a Ledger, under the session `generation` that computed it, for the account
    /// whose stored key is `public_key`.
    Device {
        signing: Box<HashSigning>,
        generation: u64,
        public_key: Felt,
    },
}

fn done(settled: Vec<DappSettled>) -> DappSignStep {
    DappSignStep::Settled { settled }
}

impl DappSession {
    /// Takes `approval_id`'s pending signature if it waits on the device (`device`) or on a
    /// read; one waiting on the other stays.
    pub(crate) fn take_sign(&self, approval_id: &str, device: bool) -> Option<PendingSign> {
        let mut state = guard(&self.state);
        let sign = state.signing.remove(approval_id)?;
        if matches!(sign.stage, SignStage::Device { .. }) == device {
            return Some(sign);
        }
        state.signing.insert(approval_id.to_owned(), sign);
        None
    }

    /// Takes one step of `sign`'s read: another read for the host, or the pages' answers.
    fn step_sign(
        &self,
        approval_id: String,
        approved: Approved,
        mut plan: Plan,
        reply: StepReply,
    ) -> DappSignStep {
        match plan.authorize(reply) {
            AuthorizeStep::Read(SendStep::Rpc { id, body, .. }) => {
                let network = approved.card.network;
                let stage = SignStage::Read(Box::new(plan));
                guard(&self.state)
                    .signing
                    .insert(approval_id, PendingSign { approved, stage });
                DappSignStep::Read { network, id, body }
            }
            AuthorizeStep::Authorized => self.finish_sign(approval_id, approved, true),
            AuthorizeStep::Read(_) | AuthorizeStep::Refused(_) => {
                done(settled(approved.waiters, |_| refused()))
            }
        }
    }
}

#[uniffi::export]
impl DappSession {
    /// The person approved `approval_id`'s typed-data card, reviewed for `reviewed_account_id`
    /// (the card's `account_id`, echoed). An OpenZeppelin account signs at once; a Ready or
    /// Braavos account first has its live state read (`Read`, answered by `answer_sign_read`). A
    /// Ledger account then signs on its device (`Device`, answered by `answer_sign_device`).
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
        let Ok(acting) = self.wallet.plan_account_for(&card.account_id, Use::Sign) else {
            return done(settled(waiters, |_| refused()));
        };
        let approved = Approved {
            origin,
            card,
            waiters,
            reviewed_account_id,
        };
        if matches!(acting.account.authorization, Authorization::StarkPair) {
            return self.finish_sign(approval_id, approved, false);
        }
        let plan = Plan::new(acting.account, Kind::Authorize, SEND_POLICY);
        self.step_sign(approval_id, approved, plan, StepReply::Start)
    }

    /// The reply to `approval_id`'s read `id` (`body` empty when the POST failed): the next read,
    /// or the pages' answers. An approval with no read outstanding answers nothing.
    pub fn answer_sign_read(&self, approval_id: String, id: u64, body: String) -> DappSignStep {
        let Some(PendingSign {
            approved,
            stage: SignStage::Read(plan),
        }) = self.take_sign(&approval_id, false)
        else {
            return done(Vec::new());
        };
        let now_ms = self.wallet.now().monotonic_ms;
        let reply = if body.is_empty() {
            StepReply::TransportFailure { id, now_ms }
        } else {
            StepReply::Rpc { id, body, now_ms }
        };
        self.step_sign(approval_id, approved, *plan, reply)
    }
}
