//! The frozen preparation (`swap-preparations.ts`, `swapPrepareResponse`'s review lines and
//! `swapExecuteResponse`'s binding and activity row): the reviewed batch, which names its own
//! activity row, the lines and flags the review shows, and the minimum the route enforces.

use super::{PendingBuild, SwapRefusal};
use crate::felt;
use crate::js::felt::address_label;
use crate::send::avnu_route::AvnuRoute;
use crate::send::call_decode::AVNU_MULTI_ROUTE_SWAP;
use crate::send::calls::format_token_amount;
use crate::send::invoke::{ActivityAsset, Invoke};
use crate::send::review_flags::RiskFlag;
use crate::send::types::Account;
use crate::send::u256::U256;
use crate::{Felt, NetworkId};

/// A reviewed swap, ready for `send::plan::Plan`: built only by `PendingBuild::judge`, released
/// only to the account and network it was prepared for, and consumed when released, so one
/// preparation submits at most once (`claim`). Deliberately not `Clone`: a copy would be a
/// second claim on the same reviewed batch.
#[derive(Debug, PartialEq, Eq)]
pub struct SwapPreparation {
    network: NetworkId,
    account: Felt,
    invoke: Invoke,
    summaries: Vec<String>,
    min_buy_amount: U256,
    min_buy_text: String,
}

impl SwapPreparation {
    /// The batch names the swap's activity row (`recordSubmittedActivity`, kind `invoke`): the
    /// summary, the amount sold as typed, and the sold token by symbol and address
    /// (`activityToken`), so a row never names the token by a symbol another contract may share.
    pub(super) fn new(build: PendingBuild, invoke: Invoke, route: &AvnuRoute) -> Self {
        let request = &build.quote.request;
        let (sell, buy) = (&request.sell.token, &request.buy.token);
        let min_buy_text = format_token_amount(&route.min_buy_amount, buy.decimals);
        // The route's line is wallet-authored from the decoded calldata, stating the two facts
        // the policy bound (the floor and the payee); the approval keeps the verifier's line.
        let route_line = format!(
            "Swap {} {} for at least {min_buy_text} {}, paid to {}",
            request.sell_text,
            sell.symbol,
            buy.symbol,
            address_label(&felt::canonical(&build.account.address)),
        );
        let summaries = invoke
            .calls
            .iter()
            .zip(invoke.calls())
            .map(|(call, reviewed)| {
                if call.entrypoint == AVNU_MULTI_ROUTE_SWAP {
                    route_line.clone()
                } else {
                    reviewed.summary.clone()
                }
            })
            .collect();
        let invoke = invoke
            .with_activity_summary(&format!(
                "Swap {} {} for ~{} {} via avnu",
                request.sell_text,
                sell.symbol,
                build.quote.buy_amount_text(),
                buy.symbol
            ))
            .with_activity_asset(ActivityAsset {
                amount: Some(request.sell_text.clone()),
                symbol: sell.symbol.clone(),
                token_address: request.sell.address,
            });
        Self {
            network: build.account.network,
            account: build.account.address,
            invoke,
            summaries,
            min_buy_amount: route.min_buy_amount,
            min_buy_text,
        }
    }

    /// One line per call, in call order: the approval as the verifier reads it, the route as
    /// what it pays and to whom.
    pub fn summaries(&self) -> &[String] {
        &self.summaries
    }

    /// The minimum output the frozen route carries, in base units and in the buy token's
    /// decimals: what the transaction enforces, not a figure computed beside it.
    pub fn min_buy_amount(&self) -> U256 {
        self.min_buy_amount
    }

    pub fn min_buy_amount_text(&self) -> &str {
        &self.min_buy_text
    }

    /// The review's flags (`report.flags`), as the batch review raised them.
    pub fn flags(&self) -> Vec<&RiskFlag> {
        self.invoke.flags()
    }

    /// Releases the batch to `account`, the one it was prepared for on the same network
    /// ("swap preparation changed" otherwise). It carries its own activity row.
    pub fn into_invoke(self, account: &Account) -> Result<Invoke, SwapRefusal> {
        if account.network != self.network || account.address != self.account {
            return Err(SwapRefusal::PreparationChanged);
        }
        Ok(self.invoke)
    }
}
