//! The chain methods for `DappSession` (`dapp::chain`'s gates and results): add-chain, answered at
//! once; switch-chain, a card the person approves; and deployment data, answered after one chain
//! read the host makes (`DappReply::Read`, then `answer_read`).
//!
//! The extension's switch changes its active network; mobile's network is the active account's,
//! so approving selects the account the site's grant holds on the target network, else the
//! account `ensure_network_account` picks there. A site that lands outside its grant is shown a
//! connect card (`promptReconnectForStaleGrant`), which no page waits on.

use std::collections::hash_map::Entry;

use starknet_wallet_core::dapp::chain::{
    add_chain, deployment_data, deployment_data_gate, held_account_on, switch_chain,
};
use starknet_wallet_core::dapp::connect::CONNECT_DEDUPE;
use starknet_wallet_core::dapp::{Request, WalletRpcError};
use starknet_wallet_core::js::json::JsValue;
use starknet_wallet_core::send::sign::Deployment;

use crate::dapp::DAPP_QUEUE_LIMITS;
use crate::dapp_session::{
    error, result, settled, Card, DappReply, DappSession, DappSettled, Shape, VERSIONS,
};
use crate::networks::NetworkId;
use crate::rpc_request::{RpcRequest, RpcResponse};
use crate::send::{deployment_status_request, judge_deployment_status};
use crate::wallet::guard;
use crate::{felt, CoreError};

/// The JSON-RPC id of the one read a deployment-data request makes.
const READ_ID: u64 = 1;

/// A deployment-data request waiting on its chain read: the account the gates passed for and the
/// deployment this wallet builds for it.
pub(crate) struct PendingRead {
    origin: String,
    account_id: String,
    address: String,
    network: NetworkId,
    deployment: Option<Deployment>,
}

fn refused() -> DappReply {
    error(WalletRpcError::unknown_error())
}

impl DappSession {
    /// Answers `wallet_addStarknetChain`, `wallet_switchStarknetChain` or `wallet_deploymentData`.
    pub(crate) fn request_chain(
        &self,
        origin: &str,
        request: &Request,
        shown: NetworkId,
        request_id: u64,
    ) -> Result<DappReply, CoreError> {
        match request {
            Request::AddChain(chain) => self.with_site(origin, shown, false, |site, _| {
                Ok(match add_chain(site, &chain.chain_id) {
                    Ok(json) => result(&json),
                    Err(refusal) => error(refusal),
                })
            }),
            Request::SwitchChain { chain_id } => {
                let gated = self.with_site(origin, shown, false, |site, _| {
                    Ok(switch_chain(site, chain_id))
                })?;
                Ok(match gated {
                    Ok(network) => {
                        self.queue(origin, Card::SwitchChain(network), request_id, Shape::True)
                    }
                    Err(refusal) => error(refusal),
                })
            }
            Request::DeploymentData { api_version } => {
                self.request_deployment(origin, api_version.as_deref(), shown, request_id)
            }
            _ => Ok(refused()),
        }
    }

    fn request_deployment(
        &self,
        origin: &str,
        api_version: Option<&str>,
        shown: NetworkId,
        request_id: u64,
    ) -> Result<DappReply, CoreError> {
        let gated = self.with_site(origin, shown, false, |site, session| {
            let account = match deployment_data_gate(site, api_version, &VERSIONS) {
                Ok(account) => account,
                Err(refusal) => return Ok(Err(refusal)),
            };
            let Some(record) = session.and_then(|session| session.payload.record(&account.id))
            else {
                return Ok(Err(WalletRpcError::unknown_error()));
            };
            let deployment = felt::parse_non_zero(&record.public_key)
                .and_then(|key| crate::send::wallet_ext::deployment(record, &key));
            Ok(Ok(PendingRead {
                origin: origin.to_owned(),
                account_id: record.id.clone(),
                address: record.address.clone(),
                network: record.network(),
                deployment,
            }))
        })?;
        let read = match gated {
            Ok(read) => read,
            Err(refusal) => return Ok(error(refusal)),
        };
        let Some(body) = deployment_status_request(READ_ID, read.address.clone()) else {
            return Ok(refused());
        };
        let network = read.network;
        let mut state = guard(&self.state);
        if state.reads.len() >= DAPP_QUEUE_LIMITS.max_pending {
            return Ok(refused());
        }
        match state.reads.entry(request_id) {
            // A host reusing a request id it is still waiting on is not attributing replies.
            Entry::Occupied(_) => return Ok(refused()),
            Entry::Vacant(slot) => slot.insert(read),
        };
        Ok(DappReply::Read {
            network,
            requests: vec![RpcRequest::new(READ_ID, body)],
        })
    }

    /// The approved switch: the grant re-checked before switching (a revoked or locked site is
    /// refused), then the active account moved. `None` is a refusal.
    fn switch_for(&self, origin: &str, network: NetworkId) -> Option<()> {
        let held = self
            .with_site(origin, network, true, |site, session| {
                let (Some(session), Some(_)) = (session, site.granted()) else {
                    return Ok(None);
                };
                let payload = &session.payload;
                let accounts = payload
                    .accounts
                    .iter()
                    .map(|record| (record.id.as_str(), record.network()));
                let held = held_account_on(&payload.kept.origin_grants, accounts, origin, network);
                Ok(Some(held.map(str::to_owned)))
            })
            .ok()??;
        match held {
            Some(account_id) => self.wallet.select_account(account_id).ok()?,
            None => {
                self.wallet.ensure_network_account(network).ok()?;
            }
        }
        // Exposure only narrows: an account the site never saw is not shared by a switch.
        let landed = self
            .with_site(origin, network, false, |site, _| {
                Ok(site.granted().is_some())
            })
            .ok()?;
        if !landed {
            let mut state = guard(&self.state);
            let now = self.wallet.now().monotonic_ms;
            let reconnect = Some(CONNECT_DEDUPE.into());
            let _ = state
                .queue
                .enqueue(origin, reconnect, Card::Connect, now, None);
        }
        Some(())
    }
}

#[uniffi::export]
impl DappSession {
    /// The person approved `approval_id`'s switch card: the wallet moves to its network and every
    /// request waiting on it is answered `true`. Refused (UNKNOWN_ERROR) when the site is blocked,
    /// the wallet locked, or the site no longer connected; a card already settled or expired
    /// answers nothing. Renews. `now_ms` is the host's wall clock, for the site's first-seen record.
    pub fn approve_switch_chain(&self, approval_id: String, now_ms: f64) -> Vec<DappSettled> {
        let Some((origin, Card::SwitchChain(network), waiters)) =
            self.claim(&approval_id, |card| matches!(card, Card::SwitchChain(_)))
        else {
            return Vec::new();
        };
        self.record_seen(&origin, now_ms);
        match self.switch_for(&origin, network) {
            Some(()) => settled(waiters, |_| result(&JsValue::Bool(true))),
            None => settled(waiters, |_| refused()),
        }
    }

    /// The host's replies to a `Read` reply's requests, for `request_id`. A transport failure is
    /// answered with an empty body. The request's gates run again, so an account the site lost,
    /// or a lock, while the read was out refuses it (UNKNOWN_ERROR); so does an id with no read.
    pub fn answer_read(&self, request_id: u64, responses: Vec<RpcResponse>) -> DappReply {
        let Some(read) = guard(&self.state).reads.remove(&request_id) else {
            return refused();
        };
        let body = responses
            .into_iter()
            .find(|response| response.id == READ_ID)
            .map(|response| response.body)
            .unwrap_or_default();
        let deployed = judge_deployment_status(body, READ_ID);
        let current = self.with_site(&read.origin, read.network, false, |site, _| {
            Ok(site
                .granted()
                .is_some_and(|account| account.id == read.account_id))
        });
        if !matches!(current, Ok(true)) {
            return refused();
        }
        match deployment_data(&read.address, read.deployment.as_ref(), deployed) {
            Ok(json) => result(&json),
            Err(refusal) => error(refusal),
        }
    }
}
