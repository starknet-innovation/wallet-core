//! Sending one NFT on mobile (`wallet.reviewNftTransfer` and `wallet.transferNft`,
//! `nft-transfer-submit.ts`): the host names the token and the recipient, never a call. The core
//! re-proves ownership on chain first (the standard it finds picks ERC-721 `transfer_from` or
//! ERC-1155 `safe_transfer_from` of one edition), builds the call
//! (`wallet_core::nft::transfer::prepare_transfer`), and runs it through the plan as every batch
//! runs (`Invoke::reviewed`: the poisoned-recipient refusal, the flags, the fee review, one
//! confirm, the Activity row naming the recipient). Like the revoke (`send::revoke`), the plan is
//! bound to the account, network and session it was prepared in, so a lock voids it.

use std::path::PathBuf;
use std::sync::{Arc, Weak};

use wallet_core::history::events::ScanStep;
use wallet_core::nft::transfer::{prepare_transfer, NftTransferRequest};
use wallet_core::send::call_decode::Known;
use wallet_core::send::plan::Kind;
use wallet_core::send::review_flags::ReviewContext;
use wallet_core::send::u256::U256;

use crate::allowances::{ranking, token_infos, trusted_tokens, with_allowance_log};
use crate::felt;
use crate::ffi_nft::NftError;
use crate::ffi_nft_read::NftOwnershipRead;
use crate::ffi_nft_watched::with_watched;
use crate::portfolio::model::TokenConfig;
use crate::portfolio::tokens::TokenPreferences;
use crate::portfolio::wire::{RpcReply, RpcRequest};
use crate::send::plan::{TransferPlan, SEND_POLICY};
use crate::send::wallet_ext::{known_token_addresses, PlanAccount, Use};
use crate::wallet::WalletCore;

/// One NFT send, bound to the account, network and session it was prepared in. Ask
/// `next_calls` until it is empty (the ownership read), then `plan`.
#[derive(uniffi::Object)]
pub struct NftTransfer {
    core: Weak<WalletCore>,
    dir: PathBuf,
    acting: PlanAccount,
    request: NftTransferRequest,
    /// The trusted tokens the review reads and a write evicts by.
    tokens: Vec<TokenConfig>,
    ownership: Arc<NftOwnershipRead>,
}

#[uniffi::export]
impl NftTransfer {
    /// The ownership read to send now to an endpoint of the account's network; empty while
    /// waiting for its reply or when done.
    pub fn next_calls(&self) -> Vec<RpcRequest> {
        self.ownership.next_calls()
    }

    /// Judges the read's reply. `Transport`: `restart` on the next endpoint; with none left,
    /// `plan` refuses ("this account does not own that NFT").
    pub fn accept(&self, reply: RpcReply) -> ScanStep {
        self.ownership.accept(reply)
    }

    /// Discards the read so it replays on another endpoint.
    pub fn restart(&self) {
        self.ownership.restart();
    }

    /// The send's plan: the one transfer call, reviewed as the extension's `verifyCalls`
    /// reviews it with the collection the one pinned contract, with fee affordability over
    /// `fee_balance` (STRK base units the host last read; `None` is `Unknown`). Its card is
    /// `TransferPlan::current_review`, `reviewed_calls` and `risk_flags`. Refused with the
    /// extension's sentence when the read is unfinished or found no balance, or the review
    /// refuses the recipient. A lock since `prepare_nft_transfer` voids it.
    pub fn plan(&self, fee_balance: Option<String>) -> Result<Arc<TransferPlan>, NftError> {
        let core = self.core.upgrade().ok_or(NftError::Locked)?;
        let network = self.acting.network;
        let infos = token_infos(&self.tokens);
        let collections = with_watched(&self.dir, |nfts| nfts.collections(network))?;
        let trusted = core.trusted_recipients(network)?;
        let token_contracts: Vec<_> = known_token_addresses(network)
            .into_iter()
            .chain(self.tokens.iter().filter_map(|t| felt::parse(&t.address)))
            .collect();
        let standing = with_allowance_log(&self.dir, |log| log.list())?;
        let review = ReviewContext {
            known: Known {
                tokens: &infos,
                nft_contracts: &collections,
            },
            trusted: &trusted,
            known_contracts: &[],
            token_contracts: &token_contracts,
            standing: &standing,
        };
        let mut invoke = prepare_transfer(
            &self.acting.account,
            &self.request,
            self.ownership.owned(),
            &SEND_POLICY,
            &review,
        )?;
        if let Some(balance) = fee_balance.as_deref().and_then(U256::from_decimal) {
            invoke = invoke.with_fee_balance(balance);
        }
        Ok(TransferPlan::new(
            &core,
            self.acting.clone(),
            Kind::Invoke(Box::new(invoke)),
            ranking(&self.tokens),
        ))
    }
}

#[uniffi::export]
impl WalletCore {
    /// Starts sending `token_id` (base 10) of `contract` (a non-zero lowercase felt) from
    /// `account_id` (a vault record id that passes the signing gate) on its network to
    /// `recipient` (a non-zero lowercase felt, as typed), over the person's trusted tokens there.
    /// Refuses malformed input before any read. Requires an unlocked wallet; the session is
    /// recorded, so a lock voids the send and its plan.
    pub fn prepare_nft_transfer(
        self: Arc<Self>,
        account_id: String,
        contract: String,
        token_id: String,
        recipient: String,
        tokens: Arc<TokenPreferences>,
    ) -> Result<Arc<NftTransfer>, NftError> {
        let acting = self.plan_account_for(&account_id, Use::Sign)?;
        if felt::parse_non_zero(&recipient).is_none() {
            return Err(NftError::refused("invalid transfer nft request"));
        }
        let ownership = self.nft_ownership_read(account_id, contract, token_id)?;
        let request = NftTransferRequest {
            network: acting.network,
            contract: ownership.target.contract,
            token_id: ownership.target.token_id,
            recipient,
        };
        Ok(Arc::new(NftTransfer {
            core: Arc::downgrade(&self),
            dir: self.data_dir.clone(),
            tokens: trusted_tokens(&tokens, acting.network),
            acting,
            request,
            ownership,
        }))
    }
}
