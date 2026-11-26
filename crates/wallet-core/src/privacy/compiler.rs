//! The SDK's `ActionCompiler` (`internal/compiler.ts`) on Bramble's one option path
//! (`official-sdk-builder.ts`: `autoDiscover: {channels: "refresh", notes: "refresh"}`,
//! `autoRegister: false`, `autoSelectNotes: "naive"`, `autoSetup: true`, `registryConst: true`,
//! no registry carried between jobs): the reviewed actions become the pool's `ClientAction`s, in
//! the SDK's order, with its values and its random draws in its order.
//!
//! Discovery sits between two pure steps. `walk_inputs` says what the walks must fetch (the
//! recipients of `getRecipientsNeeded`, the tokens of `resolveNotes`' `tokensToDiscover`);
//! `compile` takes what they found and runs the rest of `resolveNotes` (naive selection, the
//! insufficient-balance refusal, change), `createPool` and `transformToClientActions` through the
//! simulator (`simulator.rs`).
//!
//! Stricter than the SDK, by refusing rather than producing anything different: amounts are
//! nonzero `u128`s (the SDK asserts only that they are positive), a channel is never opened
//! without the walk's `total` (its "total-only" fallback is not ported), every selected note's id
//! must be its witness's (`compute_note_id(channel_key, token, nonce)`), and every index is a
//! checked `u32`. Refusals are fixed variants that carry no value.

use std::cmp::Reverse;

use super::client_actions::ClientAction;
use super::discovery::{AddressMap, Note};
use super::entropy::Entropy;
use super::hashes::compute_note_id;
use super::outgoing::OutgoingChannels;
use super::proof::ReviewedAction;
use super::simulator::{transform, Simulator};
use crate::send::u256::U256;
use crate::{Felt, SecretFelt};

/// Why a job could not be compiled. No variant carries an amount, address or key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompileRefusal {
    /// An amount is zero or above `u128` (the SDK's "amount must be positive" asserts), or a
    /// balance does not fit.
    InvalidAmount,
    /// The account's notes do not cover a token's outflow ("Insufficient balance for token").
    InsufficientBalance,
    /// A token ends with a surplus and nowhere to send it ("Surplus of … but no surplus action
    /// found"). Not reachable from Bramble's builder calls.
    NoSurplusAction,
    /// Registering an account that already has its own channel ("Channel already exists for").
    AlreadyRegistered,
    /// A recipient has no channel to open ("Missing channel context for recipient", "Channel not
    /// found for recipient"): it has not registered.
    RecipientNotRegistered,
    /// A channel must be opened and the walk gave no outgoing-channel count. Stricter: the SDK
    /// would run a "total-only" walk.
    MissingChannelTotal,
    /// A subchannel must be opened on a channel without a key. The SDK would write `undefined`.
    MissingChannelKey,
    /// A selected note's id is not the one its witness proves. Stricter: the SDK trusts both.
    WitnessMismatch,
    /// An index would pass `u32`.
    IndexOverflow,
    /// The viewing key has no public key.
    InvalidViewingKey,
    /// The randomness source failed.
    Entropy,
}

/// What the discovery walks must fetch: the outgoing walk's recipients and the incoming walk's
/// tokens, in the SDK's order. No tokens means the incoming walk is skipped, as the SDK skips
/// `discoverNotes`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkInputs {
    pub recipients: Vec<Felt>,
    pub tokens: Vec<Felt>,
}

/// The pool's client actions, in order, and whether the SDK would warn `USER_LINKAGE` (more than
/// one `OpenChannel`), which the wallet refuses.
#[derive(Debug)]
pub struct Compiled {
    pub actions: Vec<ClientAction>,
    pub user_linkage: bool,
}

/// `CreateNoteAction`; `amount` `None` is the SDK's `Open`.
pub(super) struct CreateNote {
    pub(super) token: Felt,
    pub(super) recipient: Felt,
    pub(super) amount: Option<u128>,
}

/// One token's builder (`TokenOperationsBuilderImpl`).
#[derive(Default)]
struct TokenOps {
    deposits: Vec<u128>,
    create_notes: Vec<(Felt, Option<u128>)>,
    withdraws: Vec<(Felt, u128)>,
}

/// The SDK's `Actions` as `collectActionsAndOptions` builds them from Bramble's builder calls.
/// Every surplus goes to the account as a note (`surplusTo(account, false)`), so a surplus is
/// just its token. No `openChannels`, `openTokenChannels` or `useNotes` are ever given.
pub(super) struct Actions {
    pub(super) register: bool,
    pub(super) deposits: Vec<(Felt, u128)>,
    pub(super) create_notes: Vec<CreateNote>,
    pub(super) withdraws: Vec<(Felt, Felt, u128)>,
    surpluses: Vec<Felt>,
}

fn amount(value: &Felt) -> Result<u128, CompileRefusal> {
    u128::try_from(*value)
        .ok()
        .filter(|amount| *amount != 0)
        .ok_or(CompileRefusal::InvalidAmount)
}

/// `strk20BuilderFor` then `collectActionsAndOptions`: each action is added through
/// `with(token)`, so actions are grouped by token in the order tokens first appear; `register()`
/// sets the viewing key; `surplusTo(account, false)` applies to every token iff any action is a
/// transfer or a withdrawal.
fn collect(reviewed: &[ReviewedAction]) -> Result<Actions, CompileRefusal> {
    let mut register = false;
    let mut builders: AddressMap<TokenOps> = AddressMap::default();
    for action in reviewed {
        match action {
            ReviewedAction::Register => register = true,
            ReviewedAction::Deposit {
                token,
                amount: value,
                recipient,
            } => {
                let value = amount(value)?;
                let ops = builders.get_or_insert_with(*token, TokenOps::default);
                ops.deposits.push(value);
                if let Some(recipient) = recipient {
                    ops.create_notes.push((*recipient, Some(value)));
                }
            }
            ReviewedAction::Transfer {
                token,
                recipient,
                amount: value,
            } => {
                let value = amount(value)?;
                let ops = builders.get_or_insert_with(*token, TokenOps::default);
                ops.create_notes.push((*recipient, Some(value)));
            }
            ReviewedAction::OpenTransfer { token, recipient } => {
                let ops = builders.get_or_insert_with(*token, TokenOps::default);
                ops.create_notes.push((*recipient, None));
            }
            ReviewedAction::Withdraw {
                token,
                recipient,
                amount: value,
            } => {
                let value = amount(value)?;
                let ops = builders.get_or_insert_with(*token, TokenOps::default);
                ops.withdraws.push((*recipient, value));
            }
        }
    }
    let spends = reviewed.iter().any(|action| {
        matches!(
            action,
            ReviewedAction::Transfer { .. }
                | ReviewedAction::OpenTransfer { .. }
                | ReviewedAction::Withdraw { .. }
        )
    });
    let mut actions = Actions {
        register,
        deposits: Vec::new(),
        create_notes: Vec::new(),
        withdraws: Vec::new(),
        surpluses: Vec::new(),
    };
    for (token, ops) in builders.iter() {
        actions
            .deposits
            .extend(ops.deposits.iter().map(|value| (*token, *value)));
        actions.create_notes.extend(
            ops.create_notes
                .iter()
                .map(|(recipient, value)| CreateNote {
                    token: *token,
                    recipient: *recipient,
                    amount: *value,
                }),
        );
        actions.withdraws.extend(
            ops.withdraws
                .iter()
                .map(|(recipient, value)| (*token, *recipient, *value)),
        );
        if spends {
            actions.surpluses.push(*token);
        }
    }
    Ok(actions)
}

/// A token's balance as what flows in and what flows out, so it is exact without signed
/// arithmetic (the SDK's `bigint` balance is `inflow - outflow`).
#[derive(Debug, Default, Clone, Copy)]
struct Balance {
    inflow: U256,
    outflow: U256,
}

fn add(sum: &mut U256, value: u128) -> Result<(), CompileRefusal> {
    *sum = sum
        .checked_add(U256::from_u128(value))
        .ok_or(CompileRefusal::InvalidAmount)?;
    Ok(())
}

/// `resolveNotes`' `balances`, in its insertion order: deposits, then withdrawals, then created
/// notes with an amount (an `Open` note touches no balance).
fn balances(actions: &Actions) -> Result<AddressMap<Balance>, CompileRefusal> {
    let mut balances: AddressMap<Balance> = AddressMap::default();
    for (token, value) in &actions.deposits {
        add(
            &mut balances.get_or_insert_with(*token, Balance::default).inflow,
            *value,
        )?;
    }
    for (token, _, value) in &actions.withdraws {
        add(
            &mut balances
                .get_or_insert_with(*token, Balance::default)
                .outflow,
            *value,
        )?;
    }
    for note in &actions.create_notes {
        if let Some(value) = note.amount {
            add(
                &mut balances
                    .get_or_insert_with(note.token, Balance::default)
                    .outflow,
                value,
            )?;
        }
    }
    Ok(balances)
}

/// `getRecipientsNeeded`: the account, then each created note's recipient, then each surplus's
/// (the account), each once, in first-seen order.
fn recipients_needed(account: &Felt, actions: &Actions) -> Vec<Felt> {
    let mut needed = vec![*account];
    for recipient in actions.create_notes.iter().map(|note| note.recipient) {
        if !needed.contains(&recipient) {
            needed.push(recipient);
        }
    }
    // Every surplus goes to the account, which is already first.
    needed
}

/// Before discovery: what the walks must fetch for `reviewed`.
pub fn walk_inputs(
    account: &Felt,
    reviewed: &[ReviewedAction],
) -> Result<WalkInputs, CompileRefusal> {
    let actions = collect(reviewed)?;
    let tokens = balances(&actions)?
        .iter()
        .filter(|(_, balance)| balance.inflow < balance.outflow)
        .map(|(token, _)| *token)
        .collect();
    Ok(WalkInputs {
        recipients: recipients_needed(account, &actions),
        tokens,
    })
}

/// After discovery: the client actions for `reviewed`, from the outgoing walk's `channels` and
/// the incoming walk's `notes` (empty when `walk_inputs` asked for no tokens). Draws from
/// `entropy` in the SDK's order; nothing is drawn if the job is refused before the first action.
pub fn compile(
    account: &Felt,
    viewing_key: &SecretFelt,
    reviewed: &[ReviewedAction],
    channels: &OutgoingChannels,
    notes: &AddressMap<Vec<Note>>,
    entropy: &mut impl Entropy,
) -> Result<Compiled, CompileRefusal> {
    let mut actions = collect(reviewed)?;
    let needed = recipients_needed(account, &actions);
    let use_notes = resolve_notes(account, &mut actions, notes)?;

    // `createPool`: the discovered channels; the job has no registry.
    let mut pool = Simulator::new(*account, viewing_key.clone(), channels.total);
    for (recipient, channel) in channels.channels.iter() {
        pool.setup_channel(*recipient, channel);
    }
    transform(account, &actions, &use_notes, &needed, &mut pool, entropy)
}

/// The rest of `resolveNotes`: per token in `balances` order, a deficit takes the token's notes
/// largest first (a stable sort, as JavaScript's; the comparator's `Number(b.amount - a.amount)`
/// keeps the sign of the exact difference, so the order is the exact one) until it is covered;
/// one still short is refused; a surplus becomes a note to the surplus recipient, or to the
/// account when the token was deposited and has no surplus action.
fn resolve_notes<'a>(
    account: &Felt,
    actions: &mut Actions,
    notes: &'a AddressMap<Vec<Note>>,
) -> Result<Vec<(Felt, &'a Note)>, CompileRefusal> {
    let mut use_notes = Vec::new();
    for (token, balance) in balances(actions)?.iter() {
        let (token, mut balance) = (*token, *balance);
        if balance.inflow < balance.outflow {
            let mut available: Vec<&Note> = notes.get(&token).into_iter().flatten().collect();
            available.sort_by_key(|note| Reverse(note.amount));
            for note in available {
                if note.id != compute_note_id(&note.witness.channel_key, &token, note.witness.nonce)
                {
                    return Err(CompileRefusal::WitnessMismatch);
                }
                use_notes.push((token, note));
                add(&mut balance.inflow, note.amount)?;
                if balance.inflow >= balance.outflow {
                    break;
                }
            }
        }
        if balance.inflow < balance.outflow {
            return Err(CompileRefusal::InsufficientBalance);
        }
        if balance.inflow > balance.outflow {
            let surplus = balance
                .inflow
                .checked_sub(balance.outflow)
                .filter(|surplus| surplus.high == 0)
                .ok_or(CompileRefusal::InvalidAmount)?;
            let has_surplus = actions.surpluses.contains(&token)
                || actions
                    .deposits
                    .iter()
                    .any(|(deposited, _)| *deposited == token);
            if !has_surplus {
                return Err(CompileRefusal::NoSurplusAction);
            }
            actions.create_notes.push(CreateNote {
                token,
                recipient: *account,
                amount: Some(surplus.low),
            });
        }
    }
    Ok(use_notes)
}

#[cfg(test)]
#[path = "compiler_tests.rs"]
mod tests;
