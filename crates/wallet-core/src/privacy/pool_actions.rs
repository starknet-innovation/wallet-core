//! What the core will sign for: the SDK's one `compile_actions(user, key, Span<ClientAction>)`
//! call, decoded and bound to the job's reviewed actions before the `CallSet` is signed. The
//! signature authorises spending in the pool, so an engine that slipped in a withdrawal, a note
//! to someone else, an external invoke or another key would otherwise be signed for.
//!
//! The decoder is the extension's `pool-call-decode.ts` (its multisig co-signer review); the
//! layout is pinned there to the vendored pool ABI by `pool-client-actions-abi.test.ts`. The
//! binding is this core's: every value-moving action must be one the job asked for, each used
//! once, and every one the job asked for must be there. Beyond those, only three things may
//! appear: spending the account's notes (`UseNote`), channels to the account or a reviewed note
//! recipient, and notes back to the account itself (the SDK's surplus, or change). An empty
//! review, or a span that moves nothing, is refused.

use super::call_set::COMPILE_ACTIONS;
use super::client_actions::LAYOUT;
use super::engine::{EngineAction, ProofLimits, ProofRefusal};
use crate::send::calls::Call;
use crate::{Felt, SecretFelt};

/// The most client actions one call may carry.
const MAX_ACTIONS: usize = 64;

const U32_LIMIT: Felt = Felt::from_hex_unchecked("0x100000000");
const U128_LIMIT: Felt = Felt::from_hex_unchecked("0x100000000000000000000000000000000");

/// One decoded client action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PoolAction {
    Register,
    /// `UseNote`: spends one of the account's notes, an inflow to the batch. Safe to allow
    /// unreviewed only because the pool nets every batch to zero and every outflow that does not
    /// return to the account is pinned to a reviewed action, so what it frees can only come back
    /// to the account.
    UseNote,
    /// `OpenChannel` or `OpenSubchannel` to `recipient`. Moves no value, but writes an on-chain
    /// link to the recipient and uses up a channel index, so it is held to the account and the
    /// reviewed note recipients.
    Channel {
        recipient: Felt,
    },
    Deposit {
        token: Felt,
        amount: Felt,
    },
    Note {
        recipient: Felt,
        token: Felt,
        amount: Felt,
    },
    /// `CreateOpenNote`: a note for `recipient` whose amount is filled later. Allowed only as a
    /// reviewed open transfer, never unreviewed, even to the account.
    OpenNote {
        recipient: Felt,
        token: Felt,
    },
    Withdraw {
        recipient: Felt,
        token: Felt,
        amount: Felt,
    },
}

// Every member is one felt; `index` is a u32 and a note's `salt` a u128, so calldata Cairo could
// not deserialize is refused rather than signed.
fn fits_abi(variant: &str, member: &str, felt: &Felt) -> bool {
    match (variant, member) {
        (_, "index") => *felt < U32_LIMIT,
        ("CreateEncNote", "salt") => *felt < U128_LIMIT,
        _ => true,
    }
}

fn field(members: &[&str], felts: &[Felt], name: &str) -> Option<Felt> {
    let at = members.iter().position(|member| *member == name)?;
    felts.get(at).copied()
}

fn action_from(variant: &str, members: &[&str], felts: &[Felt]) -> Option<PoolAction> {
    match variant {
        "SetViewingKey" => return Some(PoolAction::Register),
        "UseNote" => return Some(PoolAction::UseNote),
        "OpenChannel" | "OpenSubchannel" => {
            return field(members, felts, "recipient_addr")
                .filter(|recipient| *recipient != Felt::ZERO)
                .map(|recipient| PoolAction::Channel { recipient });
        }
        _ => {}
    }
    let token = field(members, felts, "token").filter(|token| *token != Felt::ZERO)?;
    if variant == "CreateOpenNote" {
        return field(members, felts, "recipient_addr")
            .filter(|recipient| *recipient != Felt::ZERO)
            .map(|recipient| PoolAction::OpenNote { recipient, token });
    }
    let amount = field(members, felts, "amount").filter(|amount| *amount < U128_LIMIT)?;
    if variant == "Deposit" {
        return Some(PoolAction::Deposit { token, amount });
    }
    let recipient = field(members, felts, "recipient_addr")
        .or_else(|| field(members, felts, "to_addr"))
        .filter(|recipient| *recipient != Felt::ZERO)?;
    match variant {
        "CreateEncNote" => Some(PoolAction::Note {
            recipient,
            token,
            amount,
        }),
        "Withdraw" => Some(PoolAction::Withdraw {
            recipient,
            token,
            amount,
        }),
        _ => None,
    }
}

/// `decodeMultisigStrk20CompileActions`: `None` unless the user is `account`, the key slot holds
/// `viewing_key`, every action is one the wallet's flows produce, and the span consumes the
/// calldata exactly.
pub(super) fn decode_compile_actions(
    calldata: &[Felt],
    account: &Felt,
    viewing_key: &SecretFelt,
) -> Option<Vec<PoolAction>> {
    let [user, key, count, rest @ ..] = calldata else {
        return None;
    };
    if user != account || viewing_key != key || *count > Felt::from(MAX_ACTIONS) {
        return None;
    }
    let count = usize::try_from(count.to_biguint()).ok()?;
    let mut actions = Vec::with_capacity(count);
    let mut rest = rest;
    for _ in 0..count {
        let (tag, tail) = rest.split_first()?;
        let (variant, members) = usize::try_from(tag.to_biguint())
            .ok()
            .and_then(|at| LAYOUT.get(at))?;
        let (felts, tail) = tail.split_at_checked(members.len())?;
        if !members
            .iter()
            .zip(felts)
            .all(|(member, felt)| fits_abi(variant, member, felt))
        {
            return None;
        }
        actions.push(action_from(variant, members, felts)?);
        rest = tail;
    }
    rest.is_empty().then_some(actions)
}

/// The value-moving client actions a reviewed action compiles to.
fn expected(action: &EngineAction, account: &Felt) -> Vec<PoolAction> {
    match action {
        EngineAction::Register => vec![PoolAction::Register],
        EngineAction::Deposit {
            token,
            amount,
            recipient,
        } => {
            let deposit = PoolAction::Deposit {
                token: *token,
                amount: *amount,
            };
            match recipient {
                // A deposit for someone else is the deposit plus their note (the SDK adds it).
                Some(recipient) if recipient != account => vec![
                    deposit,
                    PoolAction::Note {
                        recipient: *recipient,
                        token: *token,
                        amount: *amount,
                    },
                ],
                _ => vec![deposit],
            }
        }
        EngineAction::Transfer {
            token,
            recipient,
            amount,
        } => vec![PoolAction::Note {
            recipient: *recipient,
            token: *token,
            amount: *amount,
        }],
        EngineAction::OpenTransfer { token, recipient } => vec![PoolAction::OpenNote {
            recipient: *recipient,
            token: *token,
        }],
        EngineAction::Withdraw {
            token,
            recipient,
            amount,
        } => vec![PoolAction::Withdraw {
            recipient: *recipient,
            token: *token,
            amount: *amount,
        }],
    }
}

/// Whether `decoded` moves exactly what `reviewed` asked for: each expected action once, and
/// nothing else but spent notes, channels to the account or a reviewed note recipient, and notes
/// back to `account`. An empty review, or a span with nothing but bookkeeping, never matches
/// (the extension's review returns no card for either).
pub(super) fn matches_review(
    decoded: &[PoolAction],
    reviewed: &[EngineAction],
    account: &Felt,
) -> bool {
    let mut want: Vec<PoolAction> = reviewed
        .iter()
        .flat_map(|action| expected(action, account))
        .collect();
    let mut channel_to: Vec<Felt> = want
        .iter()
        .filter_map(|action| match action {
            PoolAction::Note { recipient, .. } | PoolAction::OpenNote { recipient, .. } => {
                Some(*recipient)
            }
            _ => None,
        })
        .collect();
    channel_to.push(*account);
    let moves =
        |action: &PoolAction| !matches!(action, PoolAction::UseNote | PoolAction::Channel { .. });
    if want.is_empty() || !decoded.iter().any(moves) {
        return false;
    }
    for action in decoded {
        if let Some(at) = want.iter().position(|wanted| wanted == action) {
            want.swap_remove(at);
            continue;
        }
        let allowed = match action {
            PoolAction::Note { recipient, .. } => recipient == account,
            PoolAction::Channel { recipient } => channel_to.contains(recipient),
            PoolAction::UseNote => true,
            _ => false,
        };
        if !allowed {
            return false;
        }
    }
    want.is_empty()
}

/// Refuses calls to sign unless they are exactly one `compile_actions` on `pool`, within the
/// calldata bound, that decodes for `account` and `viewing_key` and moves exactly `reviewed`.
pub(super) fn bound_sign_request(
    calls: &[Call],
    pool: &Felt,
    account: &Felt,
    viewing_key: &SecretFelt,
    reviewed: &[EngineAction],
    limits: &ProofLimits,
) -> Result<(), ProofRefusal> {
    match calls {
        [call]
            if call.contract_address == *pool
                && call.entrypoint == COMPILE_ACTIONS
                && call.calldata.len() <= limits.invoke_calldata_felts
                && decode_compile_actions(&call.calldata, account, viewing_key)
                    .is_some_and(|decoded| matches_review(&decoded, reviewed, account)) =>
        {
            Ok(())
        }
        _ => Err(ProofRefusal::UnboundSignRequest),
    }
}

#[cfg(test)]
#[path = "pool_actions_tests.rs"]
mod tests;
