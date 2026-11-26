//! `wallet_watchAsset` for `DappSession`: a site's ERC-20, read from the chain and tracked only
//! once the person approves its card. The extension's `handleWatchAsset` (`background/provider/
//! provider-ops.ts`), `trackWatchAsset` and `watchAssetToken` (`watch-asset.ts`) and
//! `applyWatchAssetApproval` (`approvals/approval-watch-asset.ts`).
//!
//! Gate order: the blocklist (no lock or grant: tracking a token touches no account), then queue
//! room, then the token's name, symbol and decimals read from the chain on the active network
//! (the host's read, `DappReply::Read`); a site's own decimals and symbol are never used, and its
//! name only when the chain's is unusable. A token the chain does not describe is refused as
//! INVALID_REQUEST_PAYLOAD (114). The card is queued whether or not the token is already tracked,
//! so a site learns nothing about what the wallet tracks before the person acts: an
//! already-tracked token's card is a notice whose approve and close both answer `true`.
//!
//! Approving stores the token as the site's (`TokenPreferences::add_from_dapp`, so no review
//! trusts it unless it is exactly a curated token), hidden if it looks like spam; an address
//! already tracked is left as it is. The host persists `TokenPreferences` afterwards, as after
//! any other add. Collectibles are refused (163): mobile does not show NFTs.

use starknet_wallet_core::dapp::{WalletRpcError, WatchAsset};
use starknet_wallet_core::js::felt::same_felt;
use starknet_wallet_core::js::json::JsValue;
use starknet_wallet_core::portfolio::token_meta::{
    token_metadata_requests, watch_asset_metadata_judge, TokenMetaOutcome,
};
use starknet_wallet_core::portfolio::wire::RpcReply;

use crate::dapp_chain::{refused, PendingRead};
use crate::dapp_session::{
    error, result, settled, Card, DappReply, DappSession, DappSettled, Shape,
};
use crate::networks::NetworkId;
use crate::portfolio::model::TokenConfig;
use crate::rpc_request::RpcResponse;
use crate::{felt, CoreError};

/// The first JSON-RPC id of the three metadata reads.
const FIRST_ID: u64 = 1;

/// A watch-asset request waiting on its metadata reads.
pub(crate) struct WatchRead {
    pub(crate) origin: String,
    network: NetworkId,
    address: String,
    site_name: Option<String>,
}

/// A watch-asset card: the token as the chain describes it, and whether the wallet already
/// tracked its address when the card was queued.
#[derive(Clone)]
pub(crate) struct WatchCard {
    pub(crate) token: TokenConfig,
    pub(crate) already_tracked: bool,
}

/// The dedupe key a watch card is queued under: the same site asking for the same token on the
/// same network joins its open card.
pub(crate) fn watch_dedupe(network: NetworkId, address: &str) -> String {
    format!("watch-asset:{}:{address}", network.catalog_id())
}

fn tracked_answer() -> DappReply {
    result(&JsValue::Bool(true))
}

impl DappSession {
    pub(crate) fn request_watch(
        &self,
        origin: &str,
        asset: &WatchAsset,
        shown: NetworkId,
        request_id: u64,
    ) -> Result<DappReply, CoreError> {
        let gated = self.with_site(origin, shown, false, |site, _| {
            Ok(if site.blocked {
                None
            } else {
                starknet_wallet_core::dapp::chain::network_for_chain_id(site.chain_id)
            })
        })?;
        let Some(network) = gated else {
            return Ok(refused());
        };
        let WatchAsset::Erc20 { address, name, .. } = asset else {
            return Ok(refused());
        };
        let address = felt::canonical(address);
        // A site asking again for a token whose card is open joins it, before any read or cap.
        if let Some(card) = self.open_watch_card(origin, network, &address) {
            return Ok(self.queue(origin, Card::WatchAsset(card), request_id, Shape::True));
        }
        if !self.can_admit(origin) {
            return Ok(refused());
        }
        let Ok(requests) = token_metadata_requests(FIRST_ID, address.clone()) else {
            return Ok(error(WalletRpcError::invalid_request_payload()));
        };
        let read = WatchRead {
            origin: origin.to_owned(),
            network,
            address,
            site_name: name.clone(),
        };
        Ok(self.hold_read(request_id, PendingRead::WatchAsset(read), network, requests))
    }

    /// `origin`'s open watch card for `address` on `network`, if any.
    fn open_watch_card(
        &self,
        origin: &str,
        network: NetworkId,
        address: &str,
    ) -> Option<WatchCard> {
        let mut state = crate::wallet::guard(&self.state);
        let now = self.wallet.now().monotonic_ms;
        state
            .queue
            .list(now)
            .into_iter()
            .find_map(|approval| match &approval.payload {
                Card::WatchAsset(card)
                    if approval.origin == origin
                        && card.token.network == network
                        && same_felt(&card.token.address, address) =>
                {
                    Some(card.clone())
                }
                _ => None,
            })
    }

    pub(crate) fn answer_watch(
        &self,
        read: WatchRead,
        request_id: u64,
        responses: Vec<RpcResponse>,
    ) -> DappReply {
        let outcome = watch_asset_metadata_judge(
            read.network,
            read.address.clone(),
            FIRST_ID,
            // `answer_read`'s contract: a failed POST is an empty body. The host's transport has
            // already refused a non-200 status.
            responses
                .into_iter()
                .map(|response| RpcReply {
                    id: response.id,
                    status: 200,
                    body: Some(response.body).filter(|body| !body.is_empty()),
                })
                .collect(),
            read.site_name.as_deref(),
        );
        let TokenMetaOutcome::Token { token } = outcome else {
            return error(WalletRpcError::invalid_request_payload());
        };
        if self.blocked(&read.origin) {
            return refused();
        }
        let card = WatchCard {
            already_tracked: self.tokens.is_tracked(token.network, &token.address),
            token,
        };
        self.queue(
            &read.origin,
            Card::WatchAsset(card),
            request_id,
            Shape::True,
        )
    }
}

#[uniffi::export]
impl DappSession {
    /// The person approved `approval_id`'s watch card: the token is tracked as the site's (an
    /// address already tracked is left as it is), and every request waiting on it is answered
    /// `true`. The host persists `TokenPreferences` afterwards. An already-tracked notice writes
    /// nothing; `deny` on it answers `true` too. A card already settled or expired answers
    /// nothing. `now_ms` is the host's wall clock, for the site's first-seen record.
    pub fn approve_watch_asset(&self, approval_id: String, now_ms: f64) -> Vec<DappSettled> {
        let Some((origin, Card::WatchAsset(card), waiters)) =
            self.claim(&approval_id, |card| matches!(card, Card::WatchAsset(_)))
        else {
            return Vec::new();
        };
        self.record_seen(&origin, now_ms);
        if card.already_tracked {
            return settled(waiters, |_| tracked_answer());
        }
        match self.tokens.add_from_dapp(card.token) {
            Ok(_) => settled(waiters, |_| tracked_answer()),
            Err(_) => settled(waiters, |_| refused()),
        }
    }
}
