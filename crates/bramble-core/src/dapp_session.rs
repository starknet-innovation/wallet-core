//! `DappSession`: the in-app browser's half of P16 that the core owns, exported for the apps'
//! WebView bridge. This first slice connects a site and answers the passive reads around it
//! (`wallet_requestAccounts`, `getPermissions`, `requestChainId`, the discovery methods, and
//! Bramble's `mc_accounts` and `mc_requestAccounts`); every other method is refused until its
//! review lands. Nothing here signs.
//!
//! The host establishes the asking origin (the main frame's committed `https` origin) and passes
//! each request with an id of its own. A request that needs the person comes back `Pending`; the
//! host holds the page's reply and shows `pending_approvals`, and `approve_connect`,
//! `deny_connect` and `settle_expired` hand back each held request's reply by that id.
//!
//! Approvals live in memory: they die with the process, as the extension's die with its service
//! worker. A connect card survives a lock, as the extension's does: a site asking while the
//! wallet is locked is shown its card after unlock, and approving it needs an unlocked wallet.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use starknet_wallet_core::dapp::connect::{
    self, accounts_view, addresses, approve_connect, deny_connect, request_accounts, Connection,
    CONNECT_DEDUPE,
};
use starknet_wallet_core::dapp::queue::Refusal;
use starknet_wallet_core::dapp::{
    Answer, ApprovalId, ApprovalQueue, DappAccount, Request, Site, Unlocked, Versions,
    WalletRpcError,
};
use starknet_wallet_core::js::json::JsValue;

use crate::dapp::{
    read_page_request, PageRequest, DAPP_QUEUE_LIMITS, SUPPORTED_SPECS, SUPPORTED_WALLET_API,
};
use crate::networks::NetworkId;
use crate::vault_rules::is_wallet_origin;
use crate::wallet::{Session, WalletCore};
use crate::{felt, CoreError};

const VERSIONS: Versions = Versions {
    specs: SUPPORTED_SPECS,
    wallet_api: SUPPORTED_WALLET_API,
};

/// What a page is answered: a result as JSON, a wallet API error, or nothing yet.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum DappReply {
    Result {
        json: String,
    },
    Error {
        code: i64,
        message: String,
        data: Option<String>,
    },
    /// Held for the person; settled later under the host's request id.
    Pending {
        approval_id: String,
    },
}

/// A held request's reply, by the host's id for it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DappSettled {
    pub request_id: u64,
    pub reply: DappReply,
}

/// A connect card: the site asking. The account it would see is the active one, which the host
/// reads beside it and echoes back on approval.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DappApproval {
    pub approval_id: String,
    pub origin: String,
}

/// Which result shape a held connect request wants.
#[derive(Debug, Clone, Copy)]
enum Shape {
    /// `wallet_requestAccounts`: the addresses.
    Addresses,
    /// `mc_requestAccounts`: `{ accounts: [{ address, networkId }], ok: true }`.
    View,
}

enum Outcome {
    Reply(DappReply),
    Ask(Shape),
}

/// `wallet_requestAccounts` in `shape`: the granted account, nothing for a silent request, or
/// a card to queue.
fn ask_accounts(site: &Site<'_>, silent: bool, shape: Shape) -> Outcome {
    match request_accounts(site, silent) {
        Ok(Connection::Granted(account)) => Outcome::Reply(shaped(shape, Some(account))),
        Ok(Connection::Silent) => Outcome::Reply(shaped(shape, None)),
        Ok(Connection::Ask) => Outcome::Ask(shape),
        Err(refusal) => Outcome::Reply(error(refusal)),
    }
}

struct Waiter {
    request_id: u64,
    shape: Shape,
}

struct State {
    queue: ApprovalQueue<()>,
    waiters: HashMap<String, Vec<Waiter>>,
}

/// The dapp requests of one wallet. Thread-safe; every method takes `&self`.
#[derive(uniffi::Object)]
pub struct DappSession {
    wallet: Arc<WalletCore>,
    state: Mutex<State>,
}

fn error(error: WalletRpcError) -> DappReply {
    DappReply::Error {
        code: error.code,
        message: error.message.to_owned(),
        data: error.data,
    }
}

fn result(json: &JsValue) -> DappReply {
    DappReply::Result {
        json: json.to_json(),
    }
}

fn shaped(shape: Shape, account: Option<&DappAccount>) -> DappReply {
    let accounts: Vec<&DappAccount> = account.into_iter().collect();
    result(&match shape {
        Shape::Addresses => addresses(&accounts),
        Shape::View => accounts_view(&accounts),
    })
}

/// The active account a site on `network` may be shown, from an unlocked session.
fn active_on(session: &Session, network: NetworkId) -> Option<DappAccount> {
    let payload = &session.payload;
    let record = payload
        .active_account_id
        .as_deref()
        .and_then(|id| payload.record(id))
        .filter(|record| record.network() == network)?;
    Some(DappAccount {
        id: record.id.clone(),
        address: record.address.clone(),
        network_id: record.network.clone(),
    })
}

impl DappSession {
    /// Runs `f` over the wallet's state for `origin` on `network`: its grants and active account
    /// while unlocked, nothing while locked (or with no wallet). Only the person's decisions pass
    /// `renew`; a page's requests never extend the idle window.
    fn with_site<T>(
        &self,
        origin: &str,
        network: NetworkId,
        renew: bool,
        f: impl FnOnce(&Site<'_>, Option<&mut Session>) -> Result<T, CoreError>,
    ) -> Result<T, CoreError> {
        let chain_id = felt::canonical(&network.chain_id());
        // ponytail: no blocklist yet; its PR makes this a lookup. Bramble must not ship the
        // browser before it lands.
        let blocked = false;
        let mut f = Some(f);
        let unlocked = self.wallet.with_session(renew, |session| {
            let active = active_on(session, network);
            let grants = session.payload.kept.origin_grants.clone();
            let site = Site {
                origin,
                blocked,
                unlocked: Some(Unlocked {
                    grants: &grants,
                    active: active.as_ref(),
                }),
                chain_id: &chain_id,
            };
            match f.take() {
                Some(f) => f(&site, Some(session)),
                None => Err(CoreError::Locked),
            }
        });
        match (unlocked, f) {
            (Err(CoreError::Locked | CoreError::NoWallet), Some(f)) => {
                let site = Site {
                    origin,
                    blocked,
                    unlocked: None,
                    chain_id: &chain_id,
                };
                f(&site, None)
            }
            (outcome, _) => outcome,
        }
    }

    fn queue_connect(&self, origin: &str, request_id: u64, shape: Shape) -> DappReply {
        let mut state = crate::wallet::guard(&self.state);
        let now = self.wallet.now().monotonic_ms;
        let queued = match state
            .queue
            .enqueue(origin, Some(CONNECT_DEDUPE.into()), (), now, None)
        {
            Ok(Ok(queued)) => queued,
            Ok(Err(Refusal::SiteFull | Refusal::QueueFull)) | Err(_) => {
                return error(WalletRpcError::unknown_error())
            }
        };
        let id = queued.id.as_str().to_owned();
        state
            .waiters
            .entry(id.clone())
            .or_default()
            .push(Waiter { request_id, shape });
        DappReply::Pending { approval_id: id }
    }

    /// Takes `approval_id`'s card out of the queue, with the requests waiting on it.
    fn claim(&self, approval_id: &str) -> Option<(String, Vec<Waiter>)> {
        let mut state = crate::wallet::guard(&self.state);
        let now = self.wallet.now().monotonic_ms;
        let id = ApprovalId::from_echo(approval_id);
        let approval = state.queue.claim(&id, now)?;
        let waiters = state.waiters.remove(approval_id).unwrap_or_default();
        Some((approval.origin, waiters))
    }
}

fn settled(waiters: Vec<Waiter>, reply: impl Fn(Shape) -> DappReply) -> Vec<DappSettled> {
    waiters
        .into_iter()
        .map(|waiter| DappSettled {
            request_id: waiter.request_id,
            reply: reply(waiter.shape),
        })
        .collect()
}

#[uniffi::export]
impl DappSession {
    #[uniffi::constructor]
    pub fn new(wallet: Arc<WalletCore>) -> Arc<Self> {
        Arc::new(Self {
            wallet,
            state: Mutex::new(State {
                queue: ApprovalQueue::new(DAPP_QUEUE_LIMITS),
                waiters: HashMap::new(),
            }),
        })
    }

    /// Answers `method` from `origin`, with `params` as the page's JSON text, on the network the
    /// browser shows. `request_id` is the host's, echoed when a `Pending` reply settles.
    pub fn request(
        &self,
        origin: String,
        method: String,
        params: Option<String>,
        network: NetworkId,
        request_id: u64,
    ) -> Result<DappReply, CoreError> {
        // A request from an origin the vault could not store is one the host failed to attribute.
        if !is_wallet_origin(&origin) {
            return Ok(error(WalletRpcError::invalid_request_payload()));
        }
        let page = match read_page_request(&method, params.as_deref()) {
            Ok(page) => page,
            Err(refusal) => return Ok(error(refusal)),
        };
        let outcome = self.with_site(&origin, network, false, |site, _| {
            Ok(match &page {
                PageRequest::GrantedAccounts => Outcome::Reply(shaped(Shape::View, site.granted())),
                PageRequest::RequestAccounts => ask_accounts(site, false, Shape::View),
                PageRequest::Spec(Request::RequestAccounts { silent }) => {
                    ask_accounts(site, *silent, Shape::Addresses)
                }
                PageRequest::Spec(request) => {
                    Outcome::Reply(match connect::answer(request, site, &VERSIONS) {
                        Some(Ok(Answer::Result(json))) => result(&json),
                        Some(Err(refusal)) => error(refusal),
                        // Connect is only asked above; the rest wait for their review.
                        Some(Ok(Answer::Connect)) | None => error(WalletRpcError::unknown_error()),
                    })
                }
            })
        })?;
        Ok(match outcome {
            Outcome::Reply(reply) => reply,
            Outcome::Ask(shape) => self.queue_connect(&origin, request_id, shape),
        })
    }

    /// The cards waiting for the person, oldest first.
    pub fn pending_approvals(&self) -> Vec<DappApproval> {
        let mut state = crate::wallet::guard(&self.state);
        let now = self.wallet.now().monotonic_ms;
        state
            .queue
            .list(now)
            .into_iter()
            .map(|approval| DappApproval {
                approval_id: approval.id.as_str().to_owned(),
                origin: approval.origin.clone(),
            })
            .collect()
    }

    /// The person approved `approval_id`'s card, reviewed with `reviewed_account_id` active on
    /// `network`. Grants the site that account, written to the vault, and answers every request
    /// waiting on the card. A card already settled or expired answers nothing. Renews.
    pub fn approve_connect(
        &self,
        approval_id: String,
        reviewed_account_id: String,
        network: NetworkId,
    ) -> Vec<DappSettled> {
        let Some((origin, waiters)) = self.claim(&approval_id) else {
            return Vec::new();
        };
        let granted = self.with_site(&origin, network, true, |site, session| {
            let (Ok((grants, account)), Some(session)) =
                (approve_connect(site, Some(&reviewed_account_id)), session)
            else {
                return Ok(None);
            };
            let account = account.clone();
            self.wallet.commit(session, |payload| {
                payload.kept.origin_grants = grants;
                Ok(())
            })?;
            Ok(Some(account))
        });
        match granted {
            Ok(Some(account)) => settled(waiters, |shape| shaped(shape, Some(&account))),
            _ => settled(waiters, |_| error(WalletRpcError::unknown_error())),
        }
    }

    /// The person declined `approval_id`'s card: every request waiting on it is told so.
    pub fn deny_connect(&self, approval_id: String) -> Vec<DappSettled> {
        let Some((_, waiters)) = self.claim(&approval_id) else {
            return Vec::new();
        };
        settled(waiters, |_| error(deny_connect()))
    }

    /// The requests whose cards expired or were evicted since the last call, each refused. Call it
    /// on a timer while any request is held, and after every other call.
    pub fn settle_expired(&self) -> Vec<DappSettled> {
        let mut state = crate::wallet::guard(&self.state);
        let now = self.wallet.now().monotonic_ms;
        state.queue.list(now);
        let dropped = state.queue.take_dropped();
        dropped
            .into_iter()
            .flat_map(|id| state.waiters.remove(id.as_str()).unwrap_or_default())
            .map(|waiter| DappSettled {
                request_id: waiter.request_id,
                reply: error(WalletRpcError::unknown_error()),
            })
            .collect()
    }
}
