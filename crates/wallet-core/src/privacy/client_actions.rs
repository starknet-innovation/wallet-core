//! The privacy pool's `ClientAction` (`internal/client-actions.ts`) and its Cairo encoding: the
//! `compile_actions(user, viewing_key, Span<ClientAction>)` calldata the SDK builds with
//! `CallData.compile` (`internal/proof-invocation-factory.ts`, over `serialization.ts`).
//!
//! An enum is its variant index followed by its members in ABI order; every member here is one
//! felt. `LAYOUT` is the one variant table: the encoder writes through it and `pool_actions`
//! decodes through it, so the two cannot drift apart.

use crate::{Felt, SecretFelt};

/// `ClientAction` variants in ABI order (`privacy::actions::ClientAction`) with their one-felt
/// members in ABI order. Index 8 (`InvokeExternal`) and 9 (`ComputeAndInvoke`) carry arbitrary
/// calldata and are not listed, so they never decode.
pub(super) const LAYOUT: [(&str, &[&str]); 8] = [
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
];

/// One client action, with the ABI's member types: `u32` indices, `u128` amounts and note salt,
/// felts elsewhere. Channel keys are derived from the viewing key and held as secrets.
#[derive(Debug, Clone, PartialEq, Eq)]
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
}

type Member = (&'static str, SecretFelt);

fn member(name: &'static str, value: impl Into<Felt>) -> Member {
    (name, SecretFelt::new(value.into()))
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
                    ("channel_key", channel_key.clone()),
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
                    ("channel_key", channel_key.clone()),
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
        }
    }

    /// Appends the variant index and members, through `LAYOUT`; `None` if the action's members
    /// are not the layout's, in its order.
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
        out.extend(members.into_iter().map(|(_, value)| value));
        Some(())
    }
}

/// `compile_actions(user, viewing_key, actions)` calldata: `[user, viewing_key, len, ...]`. Every
/// felt is held as a secret, since the key and channel keys are among them. `None` only if an
/// action and `LAYOUT` disagree.
pub fn compile_actions_calldata(
    user: &Felt,
    viewing_key: &SecretFelt,
    actions: &[ClientAction],
) -> Option<Vec<SecretFelt>> {
    let mut calldata = vec![
        SecretFelt::new(*user),
        viewing_key.clone(),
        SecretFelt::new(Felt::from(actions.len())),
    ];
    for action in actions {
        action.encode(&mut calldata)?;
    }
    Some(calldata)
}

#[cfg(test)]
#[path = "client_actions_tests.rs"]
mod tests;
