//! The extension's pre-sign annotation flags (`verifyCalls` in
//! `packages/features/transactions/src/routing/verify.ts`): what a review card warns about in a
//! batch. They warn; none refuses. The one flag the extension enforces, `address-poisoning`, is
//! `multisig_review::refuse_poisoned`, which refuses the batch before any flag is read, so it never
//! appears here.
//!
//! Each flag is attached to the call it is about. The approval rules are judged across the whole
//! batch: an allowance split into several increases is flagged on the call whose running total
//! crosses the unlimited threshold, seeded from what the wallet's own allowance log says already
//! stands for that account on that network.

use std::collections::{HashMap, HashSet};
use std::fmt;

use starknet_types_core::felt::Felt;

use super::call_decode::{
    recipient_for_flags, uint256_at, CallKind, DecodedCall, Known, APPROVE_SET,
    AVNU_MULTI_ROUTE_SWAP, DECREASE_ALLOWANCE,
};
use super::calls::{short_label, Call};
use super::types::Account;
use super::u256::U256;
use crate::allowances::calls::{standing_allowance_totals, AllowanceOwner};
use crate::allowances::record::{is_unlimited, AllowanceRecord};
use crate::felt;

/// `RiskFlag`: one warning, with the extension's `code` and `message`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RiskFlag {
    /// The call's contract is not a trusted token, a watched collection's item approval, or a
    /// contract the caller pinned.
    UnknownContract { contract: Felt },
    /// The call completes an allowance at or past `UNLIMITED_ALLOWANCE_THRESHOLD`.
    UnlimitedApproval,
    /// The call grants an operator over a whole collection.
    OperatorApproval,
    /// The call pays the account itself.
    SelfTransfer,
    /// The call pays a token contract the wallet knows.
    TokenContractRecipient,
    /// A swap pays its proceeds to an address other than the account.
    ForeignBeneficiary { beneficiary: Felt },
    /// A swap whose route the pinned decoder could not read.
    UndecodedSwap,
}

impl RiskFlag {
    /// `RiskFlagCode`.
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnknownContract { .. } => "unknown-contract",
            Self::UnlimitedApproval => "unlimited-approval",
            Self::OperatorApproval => "operator-approval",
            Self::SelfTransfer => "self-transfer",
            Self::TokenContractRecipient => "token-contract-recipient",
            Self::ForeignBeneficiary { .. } => "foreign-beneficiary",
            Self::UndecodedSwap => "undecoded-swap",
        }
    }

    /// The extension's sentence.
    pub fn message(&self) -> String {
        match self {
            Self::UnknownContract { contract } => format!(
                "{} is not a token or contact this wallet knows. Verify the contract before continuing.",
                short(contract)
            ),
            Self::UnlimitedApproval => "This grants an effectively unlimited spending allowance. Approve only the amount you intend to spend.".into(),
            Self::OperatorApproval => "This lets one address transfer EVERY item you own in this collection, including ones you acquire later, until you revoke it. Grant it only to a marketplace you trust.".into(),
            Self::SelfTransfer => "The recipient is this wallet's own address.".into(),
            Self::TokenContractRecipient => {
                "This is a token contract, not a wallet. Tokens sent here are usually lost.".into()
            }
            Self::ForeignBeneficiary { beneficiary } => format!(
                "This swap pays its proceeds to {}, not to this account. Reject it unless you meant to send them there.",
                short(beneficiary)
            ),
            Self::UndecodedSwap => "The wallet could not read this swap's payee or minimum output. Reject it unless you trust this site.".into(),
        }
    }
}

impl fmt::Display for RiskFlag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message())
    }
}

/// What the wallet knows on the batch's network, for the review (`VerificationContext` and
/// `verifyCalls`'s `standing`). Everything is the caller's; the core bundles none of it.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReviewContext<'a> {
    /// Trusted tokens and watched collections, as for the summaries.
    pub known: Known<'a>,
    /// Every other address the person recognises, for the poisoned-payee refusal.
    pub trusted: &'a [Felt],
    /// `knownContracts`: contracts the caller pinned (a swap's router, a staking pool), which
    /// raise no `unknown-contract`.
    pub known_contracts: &'a [Felt],
    /// The token contracts the wallet knows on the account's network beyond `known.tokens` (STRK
    /// and the curated registry, as `Transfer::new`'s `tokens`), for `token-contract-recipient`.
    pub token_contracts: &'a [Felt],
    /// The wallet's allowance log. Only records of the acting account on its network, for a
    /// trusted fungible token, seed the running totals (`standingAllowanceTotals`).
    pub standing: &'a [AllowanceRecord],
}

/// `shortAddress(canonical(value))`.
fn short(value: &Felt) -> String {
    short_label(&felt::canonical(value))
}

/// A pair's running total, `None` once it has passed `UINT256_MAX`. It stays there through a
/// decrease until an `approve` replaces it: the extension's `bigint` would know by how much, and
/// a cap that forgot the excess would read a later grant from too low and miss its flag. Wrong, if
/// at all, by over-warning, the direction this annotation may err in.
type Total = Option<U256>;

/// `standingAllowanceTotals` under `allowanceOwner(actor)`, each amount a `Total`.
fn standing_totals(actor: &Account, context: &ReviewContext) -> HashMap<(Felt, Felt), Total> {
    let Some(owner) = AllowanceOwner::of(actor.network, &actor.address) else {
        return HashMap::new();
    };
    standing_allowance_totals(context.standing, &owner, &context.known)
        .into_iter()
        .map(|(pair, amount)| (pair, Some(amount)))
        .collect()
}

/// `unlimitedApprovalCallIndexes`: whether each call is a grant whose running total for its
/// `(token, spender)` pair reaches the threshold. `approve` replaces the total, an increase adds to
/// it, a decrease lowers it (clamped at zero) and grants nothing.
fn unlimited_approvals(
    calls: &[Call],
    decoded: &[DecodedCall],
    actor: &Account,
    context: &ReviewContext,
) -> Vec<bool> {
    let mut totals = standing_totals(actor, context);
    calls
        .iter()
        .zip(decoded)
        .map(|(call, decoded)| {
            let (Some(spender), Some(amount)) =
                (call.calldata.first(), uint256_at(&call.calldata, 1))
            else {
                return false;
            };
            let key = (call.contract_address, *spender);
            let here = totals.get(&key).copied().unwrap_or(Some(U256::ZERO));
            let (grants, total) = if DECREASE_ALLOWANCE.contains(&call.entrypoint.as_str()) {
                (
                    false,
                    here.map(|here| here.checked_sub(amount).unwrap_or_default()),
                )
            } else if decoded.kind != CallKind::Approve {
                return false;
            } else if call.entrypoint == APPROVE_SET {
                (true, Some(amount))
            } else {
                (true, here.and_then(|here| here.checked_add(amount)))
            };
            totals.insert(key, total);
            grants && total.is_none_or(is_unlimited)
        })
        .collect()
}

/// Every contract that counts as a token for `token-contract-recipient`: the caller's, then the
/// trusted ones.
fn token_contracts(context: &ReviewContext) -> Vec<Felt> {
    let trusted = context
        .known
        .tokens
        .iter()
        .filter_map(|token| felt::parse(&token.address));
    context
        .token_contracts
        .iter()
        .copied()
        .chain(trusted)
        .collect()
}

/// `verifyCalls(...).flags`, kept per call: `flags[i]` is what call `i` raises, in the extension's
/// order. `decoded` is each call's `decode_call`.
pub(crate) fn call_flags(
    calls: &[Call],
    decoded: &[DecodedCall],
    actor: &Account,
    context: &ReviewContext,
) -> Vec<Vec<RiskFlag>> {
    let unlimited = unlimited_approvals(calls, decoded, actor, context);
    let tokens = token_contracts(context);
    calls
        .iter()
        .zip(decoded)
        .zip(unlimited)
        .map(|((call, decoded), unlimited)| {
            let contract = call.contract_address;
            let known = context.known.token(&contract).is_some()
                || decoded.kind == CallKind::NftApproval
                || context.known_contracts.contains(&contract);
            // `approved` is the second of exactly two felts, which the decode already required.
            let operator = decoded.kind == CallKind::OperatorApproval
                && call
                    .calldata
                    .get(1)
                    .is_some_and(|approved| *approved != Felt::ZERO);
            let recipient = recipient_for_flags(&call.entrypoint, &call.calldata);
            let mut flags = Vec::new();
            if !known {
                flags.push(RiskFlag::UnknownContract { contract });
            }
            if unlimited {
                flags.push(RiskFlag::UnlimitedApproval);
            }
            if operator {
                flags.push(RiskFlag::OperatorApproval);
            }
            if call.entrypoint == AVNU_MULTI_ROUTE_SWAP {
                if decoded.kind != CallKind::Swap {
                    flags.push(RiskFlag::UndecodedSwap);
                }
                if let Some(beneficiary) = recipient.filter(|to| *to != actor.address) {
                    flags.push(RiskFlag::ForeignBeneficiary { beneficiary });
                }
            } else if let Some(recipient) = recipient {
                if recipient == actor.address {
                    flags.push(RiskFlag::SelfTransfer);
                }
                if tokens.contains(&recipient) {
                    flags.push(RiskFlag::TokenContractRecipient);
                }
            }
            flags
        })
        .collect()
}

/// `dedupeFlags` over per-call flags: the batch's list as the extension's card shows it, each
/// `(code, message)` once, first occurrence kept.
pub(crate) fn dedupe<'a>(flags: impl Iterator<Item = &'a RiskFlag>) -> Vec<&'a RiskFlag> {
    let mut seen = HashSet::new();
    flags
        .filter(|flag| seen.insert((flag.code(), flag.message())))
        .collect()
}

#[cfg(test)]
#[path = "review_flags_tests.rs"]
mod tests;
