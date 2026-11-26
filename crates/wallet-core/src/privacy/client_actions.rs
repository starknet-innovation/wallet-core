//! The privacy pool's `ClientAction` (`internal/client-actions.ts`) and its Cairo encoding: the
//! `compile_actions(user, viewing_key, Span<ClientAction>)` calldata the SDK builds with
//! `CallData.compile` (`internal/proof-invocation-factory.ts`, over `serialization.ts`).
//!
//! An enum is its variant index followed by its members in ABI order. A member is one felt, or a
//! `Span<felt252>` (its length, then its felts) for the invoke phase's calldata (`is_span`).
//! `LAYOUT` is the one variant table: the encoder writes through it and `pool_actions` decodes
//! through it, so the two cannot drift apart.

use crate::{Felt, SecretFelt};

/// `ClientAction` variants in ABI order (`privacy::actions::ClientAction`) with their members in
/// ABI order; `is_span` says which members are spans.
pub(super) const LAYOUT: [(&str, &[&str]); 10] = [
    ("SetViewingKey", &["random"]),
    (
        "OpenChannel",
        &["recipient_addr", "index", "random", "salt"],
    ),
    (
        "OpenSubchannel",
        &[
            "recipient_addr",
            "recipient_public_key",
            "channel_key",
            "index",
            "token",
            "salt",
        ],
    ),
    (
        "CreateEncNote",
        &[
            "recipient_addr",
            "recipient_public_key",
            "token",
            "amount",
            "index",
            "salt",
        ],
    ),
    (
        "CreateOpenNote",
        &[
            "recipient_addr",
            "recipient_public_key",
            "token",
            "index",
            "random",
        ],
    ),
    ("Deposit", &["token", "amount"]),
    ("UseNote", &["channel_key", "token", "index"]),
    ("Withdraw", &["to_addr", "token", "amount", "random"]),
    ("InvokeExternal", &["contract_address", "calldata"]),
    (
        "ComputeAndInvoke",
        &[
            "contract_address",
            "compute_additional_data",
            "invoke_additional_data",
        ],
    ),
];

/// Whether a `LAYOUT` member is a `Span<felt252>` (`InvokeExternalInput.calldata`,
/// `ComputeAndInvokeInput`'s two additional-data spans); every other member is one felt.
pub(super) fn is_span(member: &str) -> bool {
    matches!(
        member,
        "calldata" | "compute_additional_data" | "invoke_additional_data"
    )
}

/// One client action, with the ABI's member types: `u32` indices, `u128` amounts and note salt,
/// felt spans for the invoke phase's data, felts elsewhere. Channel keys are derived from the
/// viewing key and held as secrets. Its `Debug` prints the variant and its spans' lengths only:
/// randoms, salts, amounts and recipients are private too.
#[derive(Clone, PartialEq, Eq)]
pub enum ClientAction {
    SetViewingKey {
        random: Felt,
    },
    OpenChannel {
        recipient_addr: Felt,
        index: u32,
        random: Felt,
        salt: Felt,
    },
    OpenSubchannel {
        recipient_addr: Felt,
        recipient_public_key: Felt,
        channel_key: SecretFelt,
        index: u32,
        token: Felt,
        salt: Felt,
    },
    CreateEncNote {
        recipient_addr: Felt,
        recipient_public_key: Felt,
        token: Felt,
        amount: u128,
        index: u32,
        salt: u128,
    },
    CreateOpenNote {
        recipient_addr: Felt,
        recipient_public_key: Felt,
        token: Felt,
        index: u32,
        random: Felt,
    },
    Deposit {
        token: Felt,
        amount: u128,
    },
    UseNote {
        channel_key: SecretFelt,
        token: Felt,
        index: u32,
    },
    Withdraw {
        to_addr: Felt,
        token: Felt,
        amount: u128,
        random: Felt,
    },
    /// The pool calls `contract_address` with `calldata`.
    InvokeExternal {
        contract_address: Felt,
        calldata: Vec<Felt>,
    },
    /// The pool runs `contract_address`'s computation over `compute_additional_data`, then its
    /// invoke over `invoke_additional_data` (a shadow account's, through the anonymizer).
    ComputeAndInvoke {
        contract_address: Felt,
        compute_additional_data: Vec<Felt>,
        invoke_additional_data: Vec<Felt>,
    },
}

impl std::fmt::Debug for ClientAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (variant, members) = self.members();
        let mut out = f.debug_struct(variant);
        for (name, values) in &members {
            if is_span(name) {
                out.field(name, &format_args!("[{} felts]", values.len()));
            }
        }
        out.finish_non_exhaustive()
    }
}

/// A member's name and its felts: one, or a span's items (without its length).
type Member = (&'static str, Vec<SecretFelt>);

fn member(name: &'static str, value: impl Into<Felt>) -> Member {
    (name, vec![SecretFelt::new(value.into())])
}

fn span(name: &'static str, values: &[Felt]) -> Member {
    (name, values.iter().copied().map(SecretFelt::new).collect())
}

impl ClientAction {
    /// The variant's name and its members, named.
    fn members(&self) -> (&'static str, Vec<Member>) {
        match self {
            Self::SetViewingKey { random } => ("SetViewingKey", vec![member("random", *random)]),
            Self::OpenChannel {
                recipient_addr,
                index,
                random,
                salt,
            } => (
                "OpenChannel",
                vec![
                    member("recipient_addr", *recipient_addr),
                    member("index", *index),
                    member("random", *random),
                    member("salt", *salt),
                ],
            ),
            Self::OpenSubchannel {
                recipient_addr,
                recipient_public_key,
                channel_key,
                index,
                token,
                salt,
            } => (
                "OpenSubchannel",
                vec![
                    member("recipient_addr", *recipient_addr),
                    member("recipient_public_key", *recipient_public_key),
                    ("channel_key", vec![channel_key.clone()]),
                    member("index", *index),
                    member("token", *token),
                    member("salt", *salt),
                ],
            ),
            Self::CreateEncNote {
                recipient_addr,
                recipient_public_key,
                token,
                amount,
                index,
                salt,
            } => (
                "CreateEncNote",
                vec![
                    member("recipient_addr", *recipient_addr),
                    member("recipient_public_key", *recipient_public_key),
                    member("token", *token),
                    member("amount", *amount),
                    member("index", *index),
                    member("salt", *salt),
                ],
            ),
            Self::CreateOpenNote {
                recipient_addr,
                recipient_public_key,
                token,
                index,
                random,
            } => (
                "CreateOpenNote",
                vec![
                    member("recipient_addr", *recipient_addr),
                    member("recipient_public_key", *recipient_public_key),
                    member("token", *token),
                    member("index", *index),
                    member("random", *random),
                ],
            ),
            Self::Deposit { token, amount } => (
                "Deposit",
                vec![member("token", *token), member("amount", *amount)],
            ),
            Self::UseNote {
                channel_key,
                token,
                index,
            } => (
                "UseNote",
                vec![
                    ("channel_key", vec![channel_key.clone()]),
                    member("token", *token),
                    member("index", *index),
                ],
            ),
            Self::Withdraw {
                to_addr,
                token,
                amount,
                random,
            } => (
                "Withdraw",
                vec![
                    member("to_addr", *to_addr),
                    member("token", *token),
                    member("amount", *amount),
                    member("random", *random),
                ],
            ),
            Self::InvokeExternal {
                contract_address,
                calldata,
            } => (
                "InvokeExternal",
                vec![
                    member("contract_address", *contract_address),
                    span("calldata", calldata),
                ],
            ),
            Self::ComputeAndInvoke {
                contract_address,
                compute_additional_data,
                invoke_additional_data,
            } => (
                "ComputeAndInvoke",
                vec![
                    member("contract_address", *contract_address),
                    span("compute_additional_data", compute_additional_data),
                    span("invoke_additional_data", invoke_additional_data),
                ],
            ),
        }
    }

    /// How many felts `encode` appends: the index, each member's felt, a span's length and items.
    fn encoded_len(&self) -> usize {
        let (_, members) = self.members();
        1 + members
            .iter()
            .map(|(name, values)| usize::from(is_span(name)) + values.len())
            .sum::<usize>()
    }

    /// Appends the variant index and members, through `LAYOUT`; `None` if the action's members
    /// are not the layout's, in its order. A span is its length, then its felts. The members are
    /// cloned in rather than moved, so the temporaries drop (and wipe) their own copies.
    fn encode(&self, out: &mut Vec<SecretFelt>) -> Option<()> {
        let (variant, members) = self.members();
        let index = LAYOUT.iter().position(|(name, layout)| {
            *name == variant
                && layout
                    .iter()
                    .copied()
                    .eq(members.iter().map(|(name, _)| *name))
        })?;
        out.push(SecretFelt::new(Felt::from(index)));
        for (name, values) in members {
            if is_span(name) {
                out.push(SecretFelt::new(Felt::from(values.len())));
            } else if values.len() != 1 {
                return None;
            }
            out.extend(values.iter().cloned());
        }
        Some(())
    }
}

/// `compile_actions(user, viewing_key, actions)` calldata: `[user, viewing_key, len, ...]`. Every
/// felt is held as a secret, since the key and channel keys are among them, in one buffer sized
/// exactly up front: a reallocation would free a block still holding them, unwiped. `None` only
/// if an action and `LAYOUT` disagree.
pub fn compile_actions_calldata(
    user: &Felt,
    viewing_key: &SecretFelt,
    actions: &[ClientAction],
) -> Option<Vec<SecretFelt>> {
    let len = 3 + actions.iter().map(ClientAction::encoded_len).sum::<usize>();
    let mut calldata = Vec::with_capacity(len);
    calldata.push(SecretFelt::new(*user));
    calldata.push(viewing_key.clone());
    calldata.push(SecretFelt::new(Felt::from(actions.len())));
    for action in actions {
        action.encode(&mut calldata)?;
    }
    debug_assert_eq!(calldata.len(), len);
    debug_assert_eq!(calldata.capacity(), len);
    Some(calldata)
}

#[cfg(test)]
#[path = "client_actions_tests.rs"]
mod tests;
