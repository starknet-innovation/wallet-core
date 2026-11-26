//! `wallet_addInvokeTransaction` for `DappSession`: a site's call batch, reviewed as the wallet's
//! own batches are and submitted only through a `TransferPlan` the person confirms. The
//! extension's `handleAddInvokeTransaction` and `queueInvokeApproval` (`background/provider/
//! provider-ops.ts`), and its execution at approval (`approvals/approval-invoke-execution.ts`,
//! `provider/held-invoke-submit.ts`).
//!
//! Gate order at request: the blocklist and queue room, then the lock and the granted active
//! account (a site that holds a grant elsewhere is also shown a connect card, as
//! `promptReconnectForStaleGrant` does), an account this wallet signs with, the batch's bounds
//! and the self-call refusal (`invoke::check`), then the unrefused review (`verify_calls`: the
//! calls' summaries and flags). A poisoned payee or spender is flagged on the card, not refused:
//! an instant refusal would tell the site which addresses the person has paid (Sherlock 140).
//!
//! Approving a card (`approve_invoke`) re-checks the site, the lock and the account, reviews the
//! batch again with `Invoke::reviewed` (which now refuses a poisoned payee) and hands the host the
//! plan: fee review, the person's confirm, signature (a Ledger account's on its device) and
//! submission, as every other batch runs. The page is answered by `settle_invoke` once the plan
//! has moved on: `{ transaction_hash }` when the node accepted the invoke, USER_REFUSED_OP when the
//! plan closed before anything was signed, UNKNOWN_ERROR when it closed after. Every refusal is
//! UNKNOWN_ERROR (163), as the extension's strings map.

use std::sync::Arc;

use starknet_wallet_core::dapp::invoke::invoke_result;
use starknet_wallet_core::dapp::{Site, WalletRpcError};
use starknet_wallet_core::send::calls::Call;
use starknet_wallet_core::send::invoke::{batch_flags, check, verify_calls, Invoke};
use starknet_wallet_core::send::plan::Kind;
use starknet_wallet_core::send::u256::U256;

use crate::allowances::{ranking, trusted_tokens};
use crate::dapp_chain::refused;
use crate::dapp_session::{
    error, result, settled, Card, DappCard, DappReply, DappSession, DappSettled, Shape, Waiter,
};
use crate::networks::NetworkId;
use crate::send::plan::{TransferPlan, SEND_POLICY};
use crate::send::types::RiskFlag;
use crate::send::wallet_ext::{PlanAccount, Use};
use crate::wallet::guard;
use crate::{felt, CoreError};

/// An invoke card: the account it was reviewed for, the calls as the page sent them, and their
/// review.
pub(crate) struct InvokeCard {
    account_id: String,
    address: String,
    network: NetworkId,
    calls: Vec<Call>,
    summaries: Vec<String>,
    flags: Vec<RiskFlag>,
}

impl InvokeCard {
    pub(crate) fn card(&self) -> DappCard {
        DappCard::Invoke {
            account_id: self.account_id.clone(),
            address: self.address.clone(),
            network: self.network,
            call_summaries: self.summaries.clone(),
            flags: self.flags.clone(),
        }
    }
}

/// An approved invoke card whose plan the host is running, with the requests waiting on it.
pub(crate) struct InFlight {
    pub(crate) waiters: Vec<Waiter>,
    plan: Arc<TransferPlan>,
}

impl InFlight {
    /// Closes the plan, for a page that has gone: nothing more is signed for it.
    pub(crate) fn close(&self) {
        self.plan.close();
    }
}

/// What approving an invoke card gives the host: the plan to run, or the pages' answers when the
/// approval was refused.
#[derive(uniffi::Enum)]
pub enum DappInvokeStart {
    Plan { plan: Arc<TransferPlan> },
    Settled { settled: Vec<DappSettled> },
}

/// The granted active account's id, or the refusal; a site that holds a grant on another
/// account is shown a connect card (`granted_elsewhere`).
fn granted_account(site: &Site<'_>) -> Result<String, bool> {
    if let Some(account) = site.granted() {
        return Ok(account.id.clone());
    }
    let elsewhere = !site.blocked
        && site.unlocked.is_some_and(|unlocked| {
            unlocked
                .grants
                .iter()
                .any(|grant| grant.origin == site.origin && !grant.account_ids.is_empty())
        });
    Err(elsewhere)
}

impl DappSession {
    /// `handleAddInvokeTransaction`: a card for the granted account, or the refusal.
    pub(crate) fn request_invoke(
        &self,
        origin: &str,
        calls: &[Call],
        request_id: u64,
    ) -> Result<DappReply, CoreError> {
        if self.blocked(origin) || !self.can_admit(origin) {
            return Ok(refused());
        }
        // The shown network answers nothing here: only an unlocked session queues an invoke.
        let granted = self.with_site(origin, NetworkId::Mainnet, false, |site, _| {
            Ok(granted_account(site))
        })?;
        let account_id = match granted {
            Ok(account_id) => account_id,
            Err(elsewhere) => {
                if elsewhere {
                    self.queue_reconnect(origin);
                }
                return Ok(refused());
            }
        };
        let Ok(acting) = self.wallet.plan_account_for(&account_id, Use::Sign) else {
            return Ok(refused());
        };
        let Some(card) = self.review_card(&acting, calls) else {
            return Ok(refused());
        };
        Ok(self.queue(origin, Card::Invoke(card), request_id, Shape::Invoke))
    }

    /// `queueInvokeApproval`'s review: the bounds and self-call refusal, then the summaries and
    /// flags with nothing refused.
    fn review_card(&self, acting: &PlanAccount, calls: &[Call]) -> Option<InvokeCard> {
        let account = &acting.account;
        check(&account.address, calls, &SEND_POLICY).ok()?;
        let inputs = self.review_inputs_for(acting.network)?;
        let reviewed = verify_calls(acting.network, &account.address, calls, &inputs.context());
        Some(InvokeCard {
            account_id: acting.account_id.clone(),
            address: felt::canonical(&account.address),
            network: acting.network,
            calls: calls.to_vec(),
            flags: batch_flags(&reviewed)
                .into_iter()
                .map(RiskFlag::from)
                .collect(),
            summaries: reviewed.into_iter().map(|call| call.summary).collect(),
        })
    }

    /// The approved card's plan: the site, lock and account checked again, and the batch
    /// reviewed again with every refusal.
    fn invoke_plan(
        &self,
        origin: &str,
        card: InvokeCard,
        fee_balance: Option<&str>,
    ) -> Option<Arc<TransferPlan>> {
        let current = self.with_site(origin, card.network, true, |site, _| {
            Ok(granted_account(site).ok())
        });
        if current.ok()?.as_deref() != Some(card.account_id.as_str()) {
            return None;
        }
        let acting = self
            .wallet
            .plan_account_for(&card.account_id, Use::Sign)
            .ok()?;
        let inputs = self.review_inputs_for(acting.network)?;
        let mut invoke =
            Invoke::reviewed(&acting.account, card.calls, &SEND_POLICY, &inputs.context())
                .ok()?
                .with_dapp_origin(origin);
        if let Some(balance) = fee_balance.and_then(U256::from_decimal) {
            invoke = invoke.with_fee_balance(balance);
        }
        let ranked = ranking(&trusted_tokens(&self.tokens, acting.network));
        Some(TransferPlan::new(
            &self.wallet,
            acting,
            Kind::Invoke(Box::new(invoke)),
            ranked,
        ))
    }
}

#[uniffi::export]
impl DappSession {
    /// The person approved `approval_id`'s invoke card: the plan to run (`advance` from `Start`,
    /// show its review, `Confirm`), with `fee_balance` the account's STRK in base units as the
    /// host last read it (`None` is unknown). Once the plan has moved (after each step the host
    /// handles), `settle_invoke` answers the page. Refused, the pages are answered at once
    /// (UNKNOWN_ERROR). A card already settled or expired answers nothing. Renews. `now_ms` is
    /// the host's wall clock, for the site's first-seen record.
    pub fn approve_invoke(
        &self,
        approval_id: String,
        fee_balance: Option<String>,
        now_ms: f64,
    ) -> DappInvokeStart {
        let Some((origin, Card::Invoke(card), waiters)) =
            self.claim(&approval_id, |card| matches!(card, Card::Invoke(_)))
        else {
            return DappInvokeStart::Settled {
                settled: Vec::new(),
            };
        };
        self.record_seen(&origin, now_ms);
        let plan = (!self.blocked(&origin))
            .then(|| self.invoke_plan(&origin, card, fee_balance.as_deref()))
            .flatten();
        let Some(plan) = plan else {
            return DappInvokeStart::Settled {
                settled: settled(waiters, |_| refused()),
            };
        };
        guard(&self.state).in_flight.insert(
            approval_id,
            InFlight {
                waiters,
                plan: plan.clone(),
            },
        );
        DappInvokeStart::Plan { plan }
    }

    /// Answers the pages waiting on `approval_id`'s plan once it has moved on: the transaction
    /// hash once the node accepted it, USER_REFUSED_OP if the plan closed with nothing signed
    /// (the person backed out of the review, or a lock voided it), UNKNOWN_ERROR if it closed
    /// after signing. Nothing while the plan is still running, or for an id with no plan.
    pub fn settle_invoke(&self, approval_id: String) -> Vec<DappSettled> {
        let mut state = guard(&self.state);
        let Some(flight) = state.in_flight.get(&approval_id) else {
            return Vec::new();
        };
        let plan = &flight.plan;
        let reply = match plan.accepted_invoke() {
            Some(hash) => result(&invoke_result(&hash)),
            None if !plan.is_closed() => return Vec::new(),
            None if plan.signatures_made() == 0 => error(WalletRpcError::user_refused_op()),
            None => refused(),
        };
        let Some(flight) = state.in_flight.remove(&approval_id) else {
            return Vec::new();
        };
        settled(flight.waiters, |_| reply.clone())
    }
}
