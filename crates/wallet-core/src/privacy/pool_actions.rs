//! What the core will sign for: the pool's one `compile_actions(user, key, Span<ClientAction>)`
//! calldata, decoded and bound to the job's reviewed actions before the `CallSet` is signed. The
//! signature authorises spending in the pool, so a compiler bug that slipped in a withdrawal, a
//! note to someone else, an external invoke or another key would otherwise be signed for.
//!
//! The calldata holds the viewing key and channel keys, so it is read as the secrets it is held
//! in, inside `scrubbed`; only public members (recipients, tokens, amounts) leave the decoder.
//!
//! The decoder is the extension's `pool-call-decode.ts` (its multisig co-signer review); the
//! layout is pinned there to the vendored pool ABI by `pool-client-actions-abi.test.ts`. The
//! binding is this core's: every value-moving action must be one the job asked for, each used
//! once, and every one the job asked for must be there. Beyond those, only three things may
//! appear: spending the account's notes (`UseNote`), channels to the account or a reviewed note
//! recipient, and notes back to the account itself (the SDK's surplus, or change). An empty
//! review, or a span that moves nothing, is refused.
//!
//! The extension's decoder refuses the invoke phase (`InvokeExternal`, `ComputeAndInvoke`); this
//! one decodes it (S6), and the binding holds it exactly: at most one, only a reviewed one, its
//! contract and every calldata felt equal to the review's with the placeholders resolved here,
//! independently of the compiler (the pool from the job, each open-note id recomputed from the
//! decoded `CreateOpenNote` and the viewing key).

use super::client_actions::{is_span, LAYOUT};
use super::compiler::Contracts;
use super::hashes::compute_note_id;
use super::proof::{InvokeArg, ProofLimits, ProofRefusal, ReviewedAction};
use super::shadow::{compute_additional_data, invoke_additional_data, OpenNoteRef};
use crate::scrub::scrubbed;
use crate::{compute_channel_key, Felt, SecretFelt};

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
    /// reviewed open transfer, never unreviewed, even to the account. `note_id` is recomputed
    /// here from its members and the viewing key, never read from the compiler, so an invoke that
    /// names it is bound to the note this span really creates.
    OpenNote {
        recipient: Felt,
        token: Felt,
        note_id: Felt,
    },
    Withdraw {
        recipient: Felt,
        token: Felt,
        amount: Felt,
    },
    /// `InvokeExternal`: the pool calls `contract` with `calldata`.
    Invoke {
        contract: Felt,
        calldata: Vec<Felt>,
    },
    /// `ComputeAndInvoke` through `contract` (the anonymizer, for a shadow account).
    ComputeAndInvoke {
        contract: Felt,
        compute: Vec<Felt>,
        invoke: Vec<Felt>,
    },
}

impl PoolAction {
    fn is_invoke_phase(&self) -> bool {
        matches!(self, Self::Invoke { .. } | Self::ComputeAndInvoke { .. })
    }
}

// Every one-felt member: `index` is a u32 and a note's `salt` a u128, so calldata Cairo could
// not deserialize is refused rather than signed.
fn fits_abi(variant: &str, member: &str, felt: &Felt) -> bool {
    match (variant, member) {
        (_, "index") => *felt < U32_LIMIT,
        ("CreateEncNote", "salt") => *felt < U128_LIMIT,
        _ => true,
    }
}

/// A variant's members by name: one felt, or a span's items.
type Members<'a> = [(&'a str, &'a [SecretFelt])];

fn field(members: &Members<'_>, name: &str) -> Option<Felt> {
    match members.iter().find(|(member, _)| *member == name)? {
        (member, [value]) if !is_span(member) => Some(*value.expose_secret()),
        _ => None,
    }
}

fn span(members: &Members<'_>, name: &str) -> Option<Vec<Felt>> {
    let (member, values) = members.iter().find(|(member, _)| *member == name)?;
    is_span(member).then(|| values.iter().map(|value| *value.expose_secret()).collect())
}

fn nonzero(value: Option<Felt>) -> Option<Felt> {
    value.filter(|value| *value != Felt::ZERO)
}

/// Reads only public members by name: a channel key is never copied out. An open note's id is
/// derived from its members with the viewing key (`compute_channel_key` then `compute_note_id`).
fn action_from(
    variant: &str,
    members: &Members<'_>,
    account: &Felt,
    viewing_key: &SecretFelt,
) -> Option<PoolAction> {
    match variant {
        "SetViewingKey" => return Some(PoolAction::Register),
        "UseNote" => return Some(PoolAction::UseNote),
        "OpenChannel" | "OpenSubchannel" => {
            return nonzero(field(members, "recipient_addr"))
                .map(|recipient| PoolAction::Channel { recipient });
        }
        "InvokeExternal" => {
            return Some(PoolAction::Invoke {
                contract: nonzero(field(members, "contract_address"))?,
                calldata: span(members, "calldata")?,
            });
        }
        "ComputeAndInvoke" => {
            return Some(PoolAction::ComputeAndInvoke {
                contract: nonzero(field(members, "contract_address"))?,
                compute: span(members, "compute_additional_data")?,
                invoke: span(members, "invoke_additional_data")?,
            });
        }
        _ => {}
    }
    let token = nonzero(field(members, "token"))?;
    if variant == "CreateOpenNote" {
        let recipient = nonzero(field(members, "recipient_addr"))?;
        let public_key = field(members, "recipient_public_key")?;
        let index = u32::try_from(field(members, "index")?).ok()?;
        let channel_key = compute_channel_key(account, viewing_key, &recipient, &public_key);
        return Some(PoolAction::OpenNote {
            recipient,
            token,
            note_id: compute_note_id(&channel_key, &token, index),
        });
    }
    let amount = field(members, "amount").filter(|amount| *amount < U128_LIMIT)?;
    if variant == "Deposit" {
        return Some(PoolAction::Deposit { token, amount });
    }
    let recipient =
        nonzero(field(members, "recipient_addr").or_else(|| field(members, "to_addr")))?;
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
/// `viewing_key`, every action is one the pool's ABI lists, and the span consumes the calldata
/// exactly.
pub(super) fn decode_compile_actions(
    calldata: &[SecretFelt],
    account: &Felt,
    viewing_key: &SecretFelt,
) -> Option<Vec<PoolAction>> {
    scrubbed(|| decode(calldata, account, viewing_key))
}

/// A felt as a count, `None` past `usize`.
fn count_of(felt: &SecretFelt) -> Option<usize> {
    usize::try_from(felt.expose_secret().to_biguint()).ok()
}

fn decode(
    calldata: &[SecretFelt],
    account: &Felt,
    viewing_key: &SecretFelt,
) -> Option<Vec<PoolAction>> {
    let [user, key, count, rest @ ..] = calldata else {
        return None;
    };
    if *user != *account || viewing_key != key || *count.expose_secret() > Felt::from(MAX_ACTIONS) {
        return None;
    }
    let count = count_of(count)?;
    let mut actions = Vec::with_capacity(count);
    let mut rest = rest;
    for _ in 0..count {
        let (tag, tail) = rest.split_first()?;
        let (variant, names) = count_of(tag).and_then(|at| LAYOUT.get(at))?;
        rest = tail;
        let mut members = Vec::with_capacity(names.len());
        for name in names.iter() {
            let len = if is_span(name) {
                let (len, tail) = rest.split_first()?;
                rest = tail;
                count_of(len)?
            } else {
                1
            };
            let (values, tail) = rest.split_at_checked(len)?;
            if !is_span(name) && !fits_abi(variant, name, values.first()?.expose_secret()) {
                return None;
            }
            members.push((*name, values));
            rest = tail;
        }
        actions.push(action_from(variant, &members, account, viewing_key)?);
    }
    rest.is_empty().then_some(actions)
}

/// What an invoke's or a shadow invoke's placeholders resolve to: the pool, and the batch's open
/// notes as the decoded span creates them, in order, with the ids recomputed in `decode`.
struct Resolve<'a> {
    contracts: &'a Contracts,
    open_notes: &'a [OpenNoteRef],
}

/// The value-moving client actions a reviewed action compiles to; `None` if it cannot compile
/// (an open note the batch does not create, a shadow invoke with no anonymizer).
fn expected(
    action: &ReviewedAction,
    account: &Felt,
    resolve: &Resolve<'_>,
) -> Option<Vec<PoolAction>> {
    Some(match action {
        ReviewedAction::Register => vec![PoolAction::Register],
        ReviewedAction::Deposit {
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
        ReviewedAction::Transfer {
            token,
            recipient,
            amount,
        } => vec![PoolAction::Note {
            recipient: *recipient,
            token: *token,
            amount: *amount,
        }],
        // Matched on recipient and token (`fulfils`): the id is the span's own.
        ReviewedAction::OpenTransfer { token, recipient } => vec![PoolAction::OpenNote {
            recipient: *recipient,
            token: *token,
            note_id: Felt::ZERO,
        }],
        ReviewedAction::Withdraw {
            token,
            recipient,
            amount,
        } => vec![PoolAction::Withdraw {
            recipient: *recipient,
            token: *token,
            amount: *amount,
        }],
        ReviewedAction::Invoke { contract, calldata } => vec![PoolAction::Invoke {
            contract: *contract,
            calldata: calldata
                .iter()
                .map(|arg| match arg {
                    InvokeArg::Felt(felt) => Some(*felt),
                    InvokeArg::PoolAddress => Some(resolve.contracts.pool),
                    InvokeArg::OpenNoteId(at) => resolve.open_notes.get(*at).map(|n| n.note_id),
                })
                .collect::<Option<Vec<Felt>>>()?,
        }],
        ReviewedAction::ShadowInvoke {
            dapp_name,
            nonce,
            calls,
            collect,
        } => vec![PoolAction::ComputeAndInvoke {
            contract: resolve.contracts.anonymizer?,
            compute: compute_additional_data(dapp_name, nonce),
            invoke: invoke_additional_data(calls, resolve.open_notes, *collect),
        }],
    })
}

/// Whether `action` is the `wanted` one; an open note by recipient and token.
fn fulfils(wanted: &PoolAction, action: &PoolAction) -> bool {
    match (wanted, action) {
        (
            PoolAction::OpenNote {
                recipient, token, ..
            },
            PoolAction::OpenNote {
                recipient: got,
                token: got_token,
                ..
            },
        ) => recipient == got && token == got_token,
        _ => wanted == action,
    }
}

/// Whether `decoded` moves exactly what `reviewed` asked for: each expected action once, and
/// nothing else but spent notes, channels to the account or a reviewed note recipient, and notes
/// back to `account`. At most one invoke-phase action, and only a reviewed one, with every
/// placeholder resolved here against `contracts` and the span's own open notes. An empty review,
/// or a span with nothing but bookkeeping, never matches (the extension's review returns no card
/// for either).
///
/// An invoke runs arbitrary code, but it is pinned felt for felt to the review, and it changes
/// nothing about the unreviewed allowances: the pool still nets the batch to zero, and the only
/// actions that take value out of it (withdrawals, encrypted notes, open notes) are still pinned
/// to the review or to the account, so what an unreviewed `UseNote` frees still returns to it.
pub(super) fn matches_review(
    decoded: &[PoolAction],
    reviewed: &[ReviewedAction],
    account: &Felt,
    contracts: &Contracts,
) -> bool {
    let open_notes: Vec<OpenNoteRef> = decoded
        .iter()
        .filter_map(|action| match action {
            PoolAction::OpenNote { token, note_id, .. } => Some(OpenNoteRef {
                note_id: *note_id,
                token: *token,
            }),
            _ => None,
        })
        .collect();
    let resolve = Resolve {
        contracts,
        open_notes: &open_notes,
    };
    let Some(mut want) = reviewed
        .iter()
        .map(|action| expected(action, account, &resolve))
        .collect::<Option<Vec<_>>>()
        .map(|want| want.concat())
    else {
        return false;
    };
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
    let invokes = decoded.iter().filter(|action| action.is_invoke_phase());
    if want.is_empty() || !decoded.iter().any(moves) || invokes.count() > 1 {
        return false;
    }
    for action in decoded {
        if let Some(at) = want.iter().position(|wanted| fulfils(wanted, action)) {
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

/// Refuses `compile_actions` calldata unless it is within the calldata bound, decodes for
/// `account` and `viewing_key`, and moves exactly `reviewed` (its invoke resolved against
/// `contracts`).
pub(super) fn bound_sign_request(
    calldata: &[SecretFelt],
    account: &Felt,
    viewing_key: &SecretFelt,
    reviewed: &[ReviewedAction],
    contracts: &Contracts,
    limits: &ProofLimits,
) -> Result<(), ProofRefusal> {
    let bound = calldata.len() <= limits.invoke_calldata_felts
        && decode_compile_actions(calldata, account, viewing_key)
            .is_some_and(|decoded| matches_review(&decoded, reviewed, account, contracts));
    if bound {
        Ok(())
    } else {
        Err(ProofRefusal::UnboundSignRequest)
    }
}

#[cfg(test)]
#[path = "pool_actions_tests.rs"]
mod tests;
