//! Connecting a site, and the passive reads a page makes before and after: the extension's
//! handlers for `wallet_requestAccounts`, `wallet_getPermissions`, `wallet_requestChainId` and
//! the version discovery methods (`apps/extension/src/background/routing/messages.ts`,
//! `provider/provider-ops.ts`, `transactions/chain-ops.ts`, `portfolio/discovery-ops.ts`), the
//! connect approval's decision (`approvals/approval-decisions.ts`), and the results the page
//! sees (`wallet-protocol/src/provider/results.ts`).
//!
//! Gate order, as the extension runs it once the origin is established: the blocklist, then the
//! lock, then the grant. A blocked site learns nothing, not even the active network or the
//! versions. A locked wallet answers every passive read as an unlocked one answers a site it has
//! not connected, so a page polling these cannot tell when the wallet locks.
//!
//! Sans-IO: the host reads the vault's grants, the active account and network into a [`Site`]
//! just before each call, queues [`Answer::Connect`] in its `ApprovalQueue` under
//! [`CONNECT_DEDUPE`], and writes back the grants [`approve_connect`] returns.

use super::error::WalletRpcError;
use super::grants::{granted_account, with_origin_grant, OriginGrant};
use super::request::Request;
use crate::js::json::JsValue;

/// The dedupe key a connect approval is queued under: a site asking again while its card is
/// open joins that card (`approvals.request`).
pub const CONNECT_DEDUPE: &str = "connect";

/// What a site may be told about an account: its vault id, address and network.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DappAccount {
    /// `network:address`, the vault's account id. Never shown to the page.
    pub id: String,
    pub address: String,
    /// The extension's network id (`SN_MAIN`, `SN_SEPOLIA`).
    pub network_id: String,
}

/// The wallet's state when a site asks, read by the host just before the call.
#[derive(Debug, Clone, Copy)]
pub struct Site<'a> {
    /// The asking origin, already established by the host and checked against the vault's
    /// origin grammar.
    pub origin: &'a str,
    /// Whether the blocklist refuses this origin.
    pub blocked: bool,
    /// `None` while locked.
    pub unlocked: Option<Unlocked<'a>>,
    /// The active network's chain id, `0x`-hex (`STARKNET_NETWORKS[..].chainId`).
    pub chain_id: &'a str,
}

/// What only an unlocked wallet can read.
#[derive(Debug, Clone, Copy)]
pub struct Unlocked<'a> {
    pub grants: &'a [OriginGrant],
    pub active: Option<&'a DappAccount>,
}

impl<'a> Site<'a> {
    /// The account this site may see now (`grantedDappAccounts`): the active one, if its grant
    /// covers it; nothing while locked or blocked.
    pub fn granted(&self) -> Option<&'a DappAccount> {
        if self.blocked {
            return None;
        }
        let unlocked = self.unlocked?;
        let active = unlocked.active?;
        granted_account(unlocked.grants, Some(&active.id), self.origin).map(|_| active)
    }
}

/// The versions a wallet advertises (`SUPPORTED_SPECS`, `SUPPORTED_WALLET_API`).
#[derive(Debug, Clone, Copy)]
pub struct Versions {
    pub specs: &'static [&'static str],
    pub wallet_api: &'static [&'static str],
}

/// How the wallet answers a request it can answer at once, or that it must ask the person about.
#[derive(Debug, Clone, PartialEq)]
pub enum Answer {
    /// The page's result, as JSON.
    Result(JsValue),
    /// Queue a connect approval under [`CONNECT_DEDUPE`] and hold the page's reply until
    /// [`approve_connect`] or [`deny_connect`] settles it.
    Connect,
}

/// `wallet_requestAccounts` before its result is shaped: the account already granted, nothing
/// (a silent request with no grant), or the person must be asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connection<'a> {
    Granted(&'a DappAccount),
    Silent,
    Ask,
}

/// `handleRequestAccounts`'s gates. A blocked site is refused; a locked wallet asks, as an
/// unconnected one does, unless the request is silent.
pub fn request_accounts<'a>(
    site: &Site<'a>,
    silent: bool,
) -> Result<Connection<'a>, WalletRpcError> {
    if site.blocked {
        return Err(WalletRpcError::unknown_error());
    }
    Ok(match site.granted() {
        Some(account) => Connection::Granted(account),
        None if silent => Connection::Silent,
        None => Connection::Ask,
    })
}

/// Answers the connect and discovery methods. `None` for every other request, which this slice
/// does not answer yet.
pub fn answer(
    request: &Request,
    site: &Site<'_>,
    versions: &Versions,
) -> Option<Result<Answer, WalletRpcError>> {
    let blocked = || Err(WalletRpcError::unknown_error());
    Some(match request {
        Request::RequestAccounts { silent } => {
            request_accounts(site, *silent).map(|connection| match connection {
                Connection::Granted(account) => Answer::Result(addresses(&[account])),
                Connection::Silent => Answer::Result(addresses(&[])),
                Connection::Ask => Answer::Connect,
            })
        }
        // A blocked or locked site holds no permission; never an error, which would tell a
        // polling page when the wallet locked.
        Request::GetPermissions => {
            let granted = site.granted().is_some();
            Ok(Answer::Result(strings(if granted {
                &["accounts"]
            } else {
                &[]
            })))
        }
        Request::RequestChainId if site.blocked => blocked(),
        Request::RequestChainId => Ok(Answer::Result(JsValue::String(site.chain_id.to_owned()))),
        Request::SupportedSpecs { api_version } | Request::SupportedWalletApi { api_version } => {
            if site.blocked {
                return Some(blocked());
            }
            // `apiVersionSupported`: both methods check against the wallet API versions.
            if api_version
                .as_deref()
                .is_some_and(|asked| !versions.wallet_api.contains(&asked))
            {
                return Some(Err(WalletRpcError::api_version_not_supported()));
            }
            let listed = match request {
                Request::SupportedSpecs { .. } => versions.specs,
                _ => versions.wallet_api,
            };
            Ok(Answer::Result(strings(listed)))
        }
        _ => return None,
    })
}

/// The approve decision on a connect card (`applyConnect`): the grants to write back, with the
/// active account added for this site, and the account the waiting pages now see.
///
/// The grant goes to the account the card named, never to whichever is active when the click
/// lands: `reviewed_account_id` is the card's echo, and an active account that moved since is
/// refused. Refused too: a site blocked since it asked, a locked wallet, no active account.
pub fn approve_connect<'a>(
    site: &Site<'a>,
    reviewed_account_id: Option<&str>,
) -> Result<(Vec<OriginGrant>, &'a DappAccount), WalletRpcError> {
    let refused = WalletRpcError::unknown_error;
    if site.blocked {
        return Err(refused());
    }
    let unlocked = site.unlocked.ok_or_else(refused)?;
    let active = unlocked.active.ok_or_else(refused)?;
    if reviewed_account_id.is_some_and(|reviewed| reviewed != active.id) {
        return Err(refused());
    }
    Ok((
        with_origin_grant(unlocked.grants, site.origin, &active.id),
        active,
    ))
}

/// The deny decision on a connect card: every page waiting on it is told the person refused.
pub fn deny_connect() -> WalletRpcError {
    WalletRpcError::user_refused_op()
}

/// `wallet_requestAccounts`'s result: the granted accounts' addresses.
pub fn addresses(accounts: &[&DappAccount]) -> JsValue {
    JsValue::Array(
        accounts
            .iter()
            .map(|account| JsValue::String(account.address.clone()))
            .collect(),
    )
}

/// The `{ accounts: [{ address, networkId }], ok: true }` view Bramble's own methods return
/// (`accountsViewPayload`): no id, key or derivation.
pub fn accounts_view(accounts: &[&DappAccount]) -> JsValue {
    let list = accounts
        .iter()
        .map(|account| {
            JsValue::Object(vec![
                ("address".into(), JsValue::String(account.address.clone())),
                (
                    "networkId".into(),
                    JsValue::String(account.network_id.clone()),
                ),
            ])
        })
        .collect();
    JsValue::Object(vec![
        ("accounts".into(), JsValue::Array(list)),
        ("ok".into(), JsValue::Bool(true)),
    ])
}

fn strings(values: &[&str]) -> JsValue {
    JsValue::Array(
        values
            .iter()
            .map(|value| JsValue::String((*value).to_owned()))
            .collect(),
    )
}

#[cfg(test)]
#[path = "connect_tests.rs"]
mod tests;
