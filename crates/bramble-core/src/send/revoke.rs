//! The allowance revoke (`handleRevokeAllowance` in `background/portfolio/allowance-ops.ts`):
//! the first, and so far only, export that builds a call batch. The core builds the one call
//! (`approve(spender, 0)`, `allowances::revoke::RevokeTarget`) from an allowance id the log or the
//! chain named for the account, reviews it (`Invoke::reviewed`) and runs it through the plan
//! like a transfer; the host names an id, never a call.
//!
//! An ERC-20 revoke on an ERC-721 approves item #0 instead, so a contract the person does not
//! trust as a token is sent a revoke only once the chain has answered `allowance(owner,
//! spender)` for that pair: the `AllowanceRevoke` reads it, sans-IO, before it will plan, and
//! the reading never leaves the core.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};

use wallet_core::allowances::revoke::{RevokeRefusal, RevokeTarget};
use wallet_core::allowances::tracked::TrackedAllowanceRead;
use wallet_core::history::events::ScanStep;
use wallet_core::send::call_decode::Known;
use wallet_core::send::plan::Kind;
use wallet_core::send::review_flags::ReviewContext;
use wallet_core::send::u256::U256;

use super::plan::TransferPlan;
use super::plan::SEND_POLICY;
use super::types::SendError;
use super::wallet_ext::{known_token_addresses, PlanAccount, Use};
use crate::allowances::{ranking, token_infos, trusted_tokens, with_allowance_log};
use crate::felt;
use crate::portfolio::model::TokenConfig;
use crate::portfolio::tokens::TokenPreferences;
use crate::portfolio::wire::{RpcReply, RpcRequest};
use crate::wallet::WalletCore;

/// Why a revoke was refused before a plan exists. Each is a fixed sentence (the extension's,
/// where it has one); none carries an id, an address or an amount.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum RevokeError {
    #[error("wallet is locked")]
    Locked,
    #[error("unknown account")]
    UnknownAccount,
    /// The allowance log, contacts or multisig file could not be read.
    #[error("a wallet file could not be read")]
    Storage,
    #[error("invalid revoke request")]
    InvalidRequest,
    /// The id names another account's grant (or a multisig's, which needs its own intent).
    #[error("switch to the account that granted this allowance")]
    OtherAccount,
    #[error("allowance not found")]
    NotFound,
    #[error("this contract is an NFT collection, not a token; revoke it from the dapp")]
    Collection,
    /// The contract is not a trusted token and the chain did not answer `allowance` for it.
    #[error(
        "this wallet could not confirm that contract is a token, so it will not send a revoke \
         that could approve an NFT instead; revoke it from the dapp"
    )]
    UnconfirmedToken,
    /// The batch review refused the call; `reason` is its fixed sentence.
    #[error("{reason}")]
    Refused { reason: String },
}

impl From<RevokeRefusal> for RevokeError {
    fn from(refusal: RevokeRefusal) -> Self {
        match refusal {
            RevokeRefusal::InvalidRequest => Self::InvalidRequest,
            RevokeRefusal::OtherAccount => Self::OtherAccount,
            RevokeRefusal::NotFound => Self::NotFound,
            RevokeRefusal::Collection => Self::Collection,
            RevokeRefusal::UnconfirmedToken => Self::UnconfirmedToken,
            RevokeRefusal::Invoke(refusal) => Self::Refused {
                reason: refusal.to_string(),
            },
        }
    }
}

impl From<SendError> for RevokeError {
    fn from(error: SendError) -> Self {
        match error {
            SendError::Locked => Self::Locked,
            SendError::UnknownAccount => Self::UnknownAccount,
            // `trusted_recipients` fails closed on an unreadable contacts or multisig file.
            _ => Self::Storage,
        }
    }
}

impl From<wallet_core::Error> for RevokeError {
    fn from(_: wallet_core::Error) -> Self {
        Self::Storage
    }
}

/// One revoke, bound to the account, network and session it was prepared in. Ask `next_calls`
/// until it is empty (the confirmation read; none when the token is trusted), then `plan`.
#[derive(uniffi::Object)]
pub struct AllowanceRevoke {
    core: Weak<WalletCore>,
    dir: PathBuf,
    acting: PlanAccount,
    target: RevokeTarget,
    tokens: Vec<TokenConfig>,
    read: Mutex<TrackedAllowanceRead>,
}

impl AllowanceRevoke {
    fn read(&self) -> MutexGuard<'_, TrackedAllowanceRead> {
        self.read.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[uniffi::export]
impl AllowanceRevoke {
    /// The confirmation read to send now to an endpoint of the account's network; empty while
    /// waiting for its reply, when done, or when the token is trusted and no read is needed.
    pub fn next_calls(&self) -> Vec<RpcRequest> {
        self.read().next_calls()
    }

    /// Judges the read's reply. `Transport`: `restart` on the next endpoint; with none left,
    /// `plan` refuses an untrusted contract (`UnconfirmedToken`).
    pub fn accept(&self, reply: RpcReply) -> ScanStep {
        self.read().accept(reply)
    }

    /// Discards the read so it replays on another endpoint.
    pub fn restart(&self) {
        self.read().restart();
    }

    /// The revoke's plan: `approve(spender, 0)`, reviewed as the extension's `verifyCalls` reviews
    /// it, with fee affordability over `fee_balance` (STRK base units the host last read for the
    /// account; `None` is `Unknown`). Its card is `TransferPlan::current_review`,
    /// `reviewed_calls` and `risk_flags`. A lock since `prepare_revoke` voids the plan.
    pub fn plan(&self, fee_balance: Option<String>) -> Result<Arc<TransferPlan>, RevokeError> {
        let core = self.core.upgrade().ok_or(RevokeError::Locked)?;
        let network = self.acting.network;
        // An unfinished read is no reading: an untrusted contract then refuses.
        let readings = self.read().finish().unwrap_or_default();
        let infos = token_infos(&self.tokens);
        let known = Known {
            tokens: &infos,
            nft_contracts: &[],
        };
        let trusted = core.trusted_recipients(network)?;
        let token_contracts: Vec<_> = known_token_addresses(network)
            .into_iter()
            .chain(self.tokens.iter().filter_map(|t| felt::parse(&t.address)))
            .collect();
        let standing = with_allowance_log(&self.dir, |log| log.list())?;
        let context = ReviewContext {
            known,
            trusted: &trusted,
            known_contracts: &[],
            token_contracts: &token_contracts,
            standing: &standing,
        };
        let mut invoke =
            self.target
                .invoke(&self.acting.account, &context, &readings, &SEND_POLICY)?;
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
    /// Starts revoking the allowance `allowance_id` (an `AllowanceView::id`) from `account_id`
    /// (a vault record id that passes the signing gate), over the person's trusted tokens on its
    /// network. Refuses an id this account does not own on that network, an unknown one, and a
    /// malformed one, before any read. Requires an unlocked wallet; the session is recorded, so
    /// a lock voids the revoke and its plan.
    pub fn prepare_revoke(
        self: Arc<Self>,
        account_id: String,
        allowance_id: String,
        tokens: Arc<TokenPreferences>,
    ) -> Result<Arc<AllowanceRevoke>, RevokeError> {
        let acting = self.plan_account_for(&account_id, Use::Sign)?;
        let tokens = trusted_tokens(&tokens, acting.network);
        let infos = token_infos(&tokens);
        let known = Known {
            tokens: &infos,
            nft_contracts: &[],
        };
        let stored = with_allowance_log(&self.data_dir, |log| log.list())?;
        let target = RevokeTarget::resolve(&allowance_id, &acting.account, &stored, &known)?;
        let pairs: Vec<_> = target.confirmation_pair(&known).into_iter().collect();
        Ok(Arc::new(AllowanceRevoke {
            core: Arc::downgrade(&self),
            dir: self.data_dir.clone(),
            read: Mutex::new(TrackedAllowanceRead::new(&pairs, crate::ALLOWANCE_POLICY)),
            acting,
            target,
            tokens,
        }))
    }
}
