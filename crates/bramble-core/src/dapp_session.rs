//! `DappSession`: the in-app browser's half of P16 that the core owns, exported for the apps'
//! WebView bridge. This first slice connects a site and answers the passive reads around it
//! (`wallet_requestAccounts`, `getPermissions`, `requestChainId`, the discovery methods, and
//! Bramble's `mc_accounts` and `mc_requestAccounts`), signs a site's SNIP-12 typed data once
//! the person approves its card (`dapp_sign.rs`), and answers the chain methods (add-chain,
//! switch-chain and deployment data, `dapp_chain.rs`); every other method is refused until its
//! review lands. Nothing signs without a queued card the person approved.
//!
//! The host establishes the asking origin (the main frame's committed `https` origin) and passes
//! each request with an id of its own. A request that needs the person comes back `Pending`; the
//! host holds the page's reply and shows `pending_approvals`, and `approve_connect`,
//! `approve_sign_typed_data`, `deny` and `settle_expired` hand back each held request's reply by
//! that id.
//!
//! Approvals live in memory: they die with the process, as the extension's die with its service
//! worker. A card survives a lock, as the extension's do: a site asking to connect while the
//! wallet is locked is shown its card after unlock, and approving any card needs an unlocked
//! wallet (a typed-data card approved while locked is refused and spent).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use starknet_wallet_core::dapp::chain::switch_dedupe;
use starknet_wallet_core::dapp::connect::{
    self, accounts_view, addresses, approve_connect, deny_connect, request_accounts, Connection,
    CONNECT_DEDUPE,
};
use starknet_wallet_core::dapp::queue::Refusal;
use starknet_wallet_core::dapp::{
    Answer, ApprovalId, ApprovalQueue, DappAccount, PhishingFeed, Request, Site, SiteWarning,
    Unlocked, Versions, WalletRpcError,
};
use starknet_wallet_core::js::json::JsValue;

use crate::dapp::{
    read_page_request, PageRequest, DAPP_QUEUE_LIMITS, PHISHING_FEED, SUPPORTED_SPECS,
    SUPPORTED_WALLET_API,
};
use crate::dapp_chain::PendingRead;
use crate::dapp_sign::SignCard;
use crate::networks::NetworkId;
use crate::portfolio::tokens::TokenPreferences;
use crate::rpc_request::RpcRequest;
use crate::send::types::RiskFlag;
use crate::vault_rules::is_wallet_origin;
use crate::wallet::{Session, WalletCore};
use crate::{felt, CoreError};

pub(crate) const VERSIONS: Versions = Versions {
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
    /// The answer needs the chain: the host POSTs `requests` to an endpoint for `network` and
    /// hands the replies to `answer_read` under this request's id.
    Read {
        network: NetworkId,
        requests: Vec<RpcRequest>,
    },
}

/// A held request's reply, by the host's id for it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DappSettled {
    pub request_id: u64,
    pub reply: DappReply,
}

/// A card: the site asking, what the wallet warns about it, and what it asks for. A blocked
/// site's card can only be denied.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DappApproval {
    pub approval_id: String,
    pub origin: String,
    pub blocked: bool,
    pub warnings: Vec<SiteWarning>,
    pub card: DappCard,
}

/// What a card asks the person.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum DappCard {
    /// Connect: the account it would see is the active one, which the host reads beside it and
    /// echoes back to `approve_connect`.
    Connect,
    /// Sign SNIP-12 typed data as `account_id` (echoed back to `approve_sign_typed_data`) on
    /// `network`. `summary` is `"<primaryType> · <domain.name>"`; `typed_data_json` is the
    /// document the signature commits to, for the app to render. A SNIP-9 outside execution
    /// also carries its calls' summaries and the review's flags, as an invoke card does (both
    /// empty for any other document). One request, one card.
    SignTypedData {
        account_id: String,
        address: String,
        network: NetworkId,
        summary: String,
        typed_data_json: String,
        call_summaries: Vec<String>,
        flags: Vec<RiskFlag>,
    },
    /// Switch the wallet to `network` ("Switch network to …"). Approving lands on the account
    /// the site's grant holds there, or the wallet's own pick, which the site does not see until
    /// it connects again.
    SwitchChain { network: NetworkId },
}

/// A queued card's payload.
pub(crate) enum Card {
    Connect,
    Sign(SignCard),
    SwitchChain(NetworkId),
}

/// Which result a held request wants.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Shape {
    /// `wallet_requestAccounts`: the addresses.
    Addresses,
    /// `mc_requestAccounts`: `{ accounts: [{ address, networkId }], ok: true }`.
    View,
    /// `wallet_signTypedData`: the signature array.
    Signature,
    /// `wallet_switchStarknetChain`: `true`.
    True,
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

pub(crate) struct Waiter {
    pub(crate) request_id: u64,
    shape: Shape,
}

pub(crate) struct State {
    pub(crate) queue: ApprovalQueue<Card>,
    pub(crate) waiters: HashMap<String, Vec<Waiter>>,
    /// Requests waiting on the host's chain read, by its request id.
    pub(crate) reads: HashMap<u64, PendingRead>,
}

/// The dapp requests of one wallet. Thread-safe; every method takes `&self`.
#[derive(uniffi::Object)]
pub struct DappSession {
    pub(crate) wallet: Arc<WalletCore>,
    /// The app's tracked tokens: an outside execution's review trusts them as the invoke route's
    /// does (`trustedTokens`).
    pub(crate) tokens: Arc<TokenPreferences>,
    pub(crate) state: Mutex<State>,
    /// Never held while taking `state` or the wallet's session.
    pub(crate) feed: Mutex<PhishingFeed>,
}

pub(crate) fn error(error: WalletRpcError) -> DappReply {
    DappReply::Error {
        code: error.code,
        message: error.message.to_owned(),
        data: error.data,
    }
}

pub(crate) fn result(json: &JsValue) -> DappReply {
    DappReply::Result {
        json: json.to_json(),
    }
}

fn shaped(shape: Shape, account: Option<&DappAccount>) -> DappReply {
    let accounts: Vec<&DappAccount> = account.into_iter().collect();
    result(&match shape {
        Shape::Addresses => addresses(&accounts),
        Shape::View => accounts_view(&accounts),
        // Never held on a connect card.
        Shape::Signature | Shape::True => return error(WalletRpcError::unknown_error()),
    })
}

/// The active account and its network, the wallet's active network (the extension's
/// `activeNetworkId` and active account move together), from an unlocked session.
fn active(session: &Session) -> Option<(DappAccount, NetworkId)> {
    let payload = &session.payload;
    let record = payload
        .active_account_id
        .as_deref()
        .and_then(|id| payload.record(id))?;
    let account = DappAccount {
        id: record.id.clone(),
        address: record.address.clone(),
        network_id: record.network.clone(),
    };
    Some((account, record.network()))
}

impl DappSession {
    /// Runs `f` over the wallet's state for `origin`: its grants, active account and that
    /// account's network while unlocked; while locked (or with no wallet, or no active account),
    /// nothing but `shown`, the network the browser shows. Only the person's decisions pass
    /// `renew`; a page's requests never extend the idle window.
    pub(crate) fn with_site<T>(
        &self,
        origin: &str,
        shown: NetworkId,
        renew: bool,
        f: impl FnOnce(&Site<'_>, Option<&mut Session>) -> Result<T, CoreError>,
    ) -> Result<T, CoreError> {
        let chain_id = |network: NetworkId| felt::canonical(&network.chain_id());
        let blocked = self.blocked(origin);
        let mut f = Some(f);
        let unlocked = self.wallet.with_session(renew, |session| {
            let active = active(session);
            let network = active.as_ref().map_or(shown, |(_, network)| *network);
            let chain_id = chain_id(network);
            let grants = session.payload.kept.origin_grants.clone();
            let site = Site {
                origin,
                blocked,
                unlocked: Some(Unlocked {
                    grants: &grants,
                    active: active.as_ref().map(|(account, _)| account),
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
                let chain_id = chain_id(shown);
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

    /// `canAdmit`: whether `origin` has room for another card, checked before any costly work.
    pub(crate) fn can_admit(&self, origin: &str) -> bool {
        let mut state = crate::wallet::guard(&self.state);
        let now = self.wallet.now().monotonic_ms;
        state.queue.can_admit(origin, now).is_ok()
    }

    /// Queues `card` (a connect card joins its site's open one, a switch card its site's open one
    /// for the same network) and holds `request_id` on it.
    pub(crate) fn queue(
        &self,
        origin: &str,
        card: Card,
        request_id: u64,
        shape: Shape,
    ) -> DappReply {
        let mut state = crate::wallet::guard(&self.state);
        let now = self.wallet.now().monotonic_ms;
        let dedupe = match &card {
            Card::Connect => Some(CONNECT_DEDUPE.into()),
            Card::SwitchChain(network) => Some(switch_dedupe(*network)),
            Card::Sign(_) => None,
        };
        let queued = match state.queue.enqueue(origin, dedupe, card, now, None) {
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

    /// Takes `approval_id`'s card out of the queue, with the requests waiting on it, when `kind`
    /// accepts it; a card of another kind stays queued, so no decision lands on a card it was
    /// not made for.
    pub(crate) fn claim(
        &self,
        approval_id: &str,
        kind: impl FnOnce(&Card) -> bool,
    ) -> Option<(String, Card, Vec<Waiter>)> {
        let mut state = crate::wallet::guard(&self.state);
        let now = self.wallet.now().monotonic_ms;
        let id = ApprovalId::from_echo(approval_id);
        if !kind(&state.queue.get(&id, now)?.payload) {
            return None;
        }
        let approval = state.queue.claim(&id, now)?;
        let waiters = state.waiters.remove(approval_id).unwrap_or_default();
        Some((approval.origin, approval.payload, waiters))
    }
}

pub(crate) fn settled(
    waiters: Vec<Waiter>,
    reply: impl Fn(Shape) -> DappReply,
) -> Vec<DappSettled> {
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
    pub fn new(wallet: Arc<WalletCore>, tokens: Arc<TokenPreferences>) -> Arc<Self> {
        Arc::new(Self {
            wallet,
            tokens,
            state: Mutex::new(State {
                queue: ApprovalQueue::new(DAPP_QUEUE_LIMITS),
                waiters: HashMap::new(),
                reads: HashMap::new(),
            }),
            feed: Mutex::new(PhishingFeed::new(PHISHING_FEED)),
        })
    }

    /// Answers `method` from `origin`, with `params` as the page's JSON text. The network is the
    /// active account's; `shown`, the network the browser shows, answers `requestChainId` only
    /// while locked. `request_id` is the host's, echoed when a `Pending` reply settles.
    pub fn request(
        &self,
        origin: String,
        method: String,
        params: Option<String>,
        shown: NetworkId,
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
        match &page {
            PageRequest::Spec(Request::SignTypedData { typed_data }) => {
                return Ok(self.request_sign(&origin, typed_data, request_id));
            }
            PageRequest::Spec(
                request @ (Request::AddChain(_)
                | Request::SwitchChain { .. }
                | Request::DeploymentData { .. }),
            ) => return self.request_chain(&origin, request, shown, request_id),
            _ => {}
        }
        let outcome = self.with_site(&origin, shown, false, |site, _| {
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
            Outcome::Ask(shape) => self.queue(&origin, Card::Connect, request_id, shape),
        })
    }

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

    /// The person approved `approval_id`'s connect card, reviewed with `reviewed_account_id` active.
    /// Grants the site that account, written to the vault, and answers every request waiting on
    /// the card. A card already settled or expired answers nothing; a site blocked since it asked
    /// is refused. Renews. `now_ms` is the host's wall clock, for the site's first-seen record.
    pub fn approve_connect(
        &self,
        approval_id: String,
        reviewed_account_id: String,
        now_ms: f64,
    ) -> Vec<DappSettled> {
        let Some((origin, _, waiters)) =
            self.claim(&approval_id, |card| matches!(card, Card::Connect))
        else {
            return Vec::new();
        };
        self.record_seen(&origin, now_ms);
        // Approving needs an unlocked wallet, whose network is the active account's: the shown
        // network only answers a locked `requestChainId`, so any value serves here.
        let granted = self.with_site(&origin, NetworkId::Mainnet, true, |site, session| {
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

    /// The person declined `approval_id`'s card, of any kind: every request waiting on it is
    /// told so (USER_REFUSED_OP).
    pub fn deny(&self, approval_id: String, now_ms: f64) -> Vec<DappSettled> {
        let Some((origin, _, waiters)) = self.claim(&approval_id, |_| true) else {
            return Vec::new();
        };
        self.record_seen(&origin, now_ms);
        settled(waiters, |_| error(deny_connect()))
    }
}
