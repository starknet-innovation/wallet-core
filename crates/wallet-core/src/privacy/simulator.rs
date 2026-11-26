//! The SDK's `PoolSimulator` (`internal/pool-simulator.ts`): the channel state the compiler reads
//! while it emits client actions, updated by each action as the pool would. No encryption, no
//! balance checks.
//!
//! Only the channel state is kept. The SDK's simulator also tracks the account's notes
//! (`setupNote`, `UseNote` removing one, a self `CreateEncNote` adding one), but on Bramble's
//! option path nothing reads them back before the actions are emitted (`hasNote` gates only a
//! `setupNote` call, and `updateRegistry`'s result is dropped with the job's registry), so they
//! change no output and are not ported.
//!
//! `transform` is the compiler's `transformToClientActions`, which emits each action through
//! the simulator. The SDK's asserts are refusals. Channel keys stay `SecretFelt`s, and no refusal carries one.

use super::client_actions::ClientAction;
use super::compiler::{Actions, CompileRefusal, Compiled};
use super::discovery::{AddressMap, Channel, Note, TokenChannel};
use super::entropy::{generate_random, generate_random120, Entropy};
use crate::{compute_channel_key, felt, stark_public_key, Felt, SecretFelt};

/// The simulated pool for one account.
pub(super) struct Simulator {
    account: Felt,
    viewing_key: SecretFelt,
    /// `nextChannelIndex`: the walk's `total`, `None` when the service gave none (the SDK's
    /// "total-only" fallback is not ported, so opening a channel without it is refused).
    next_channel_index: Option<u32>,
    channels: AddressMap<Channel>,
}

impl Simulator {
    /// `new PoolSimulator(userAddress, viewingKey, totalChannels)`.
    pub(super) fn new(account: Felt, viewing_key: SecretFelt, total: Option<u32>) -> Self {
        Self {
            account,
            viewing_key,
            next_channel_index: total,
            channels: AddressMap::default(),
        }
    }

    /// `setupChannel`: a fresh channel with the discovered public key and key; its token nonces
    /// are copied only when it has a key.
    pub(super) fn setup_channel(&mut self, recipient: Felt, channel: &Channel) {
        let tokens = match channel.key {
            Some(_) => channel.tokens.clone(),
            None => AddressMap::default(),
        };
        self.channels.insert(
            recipient,
            Channel {
                public_key: channel.public_key,
                key: channel.key.clone(),
                tokens,
            },
        );
    }

    pub(super) fn channel(&self, recipient: &Felt) -> Option<&Channel> {
        self.channels.get(recipient)
    }

    /// `getNextChannelIndex`.
    pub(super) fn next_channel_index(&self) -> Result<u32, CompileRefusal> {
        self.next_channel_index
            .ok_or(CompileRefusal::MissingChannelTotal)
    }

    /// `execute(action)`. `Deposit`, `UseNote` and `Withdraw` change no channel state.
    pub(super) fn execute(&mut self, action: &ClientAction) -> Result<(), CompileRefusal> {
        match action {
            ClientAction::SetViewingKey { .. } => self.set_viewing_key(),
            ClientAction::OpenChannel { recipient_addr, .. } => self.open_channel(recipient_addr),
            ClientAction::OpenSubchannel {
                recipient_addr,
                token,
                index,
                ..
            } => {
                let channel = self.channel_mut(recipient_addr)?;
                channel.tokens.insert(
                    *token,
                    TokenChannel {
                        token_index: *index,
                        note_nonce: 0,
                    },
                );
                Ok(())
            }
            ClientAction::CreateEncNote {
                recipient_addr,
                token,
                ..
            }
            | ClientAction::CreateOpenNote {
                recipient_addr,
                token,
                ..
            } => {
                // `incrementNoteNonce`: its `get` inserts the default before incrementing.
                let channel = self.channel_mut(recipient_addr)?;
                let nonces = channel
                    .tokens
                    .get_or_insert_with(*token, TokenChannel::default);
                nonces.note_nonce = nonces
                    .note_nonce
                    .checked_add(1)
                    .ok_or(CompileRefusal::IndexOverflow)?;
                Ok(())
            }
            ClientAction::Deposit { .. }
            | ClientAction::UseNote { .. }
            | ClientAction::Withdraw { .. } => Ok(()),
        }
    }

    /// `handleSetViewingKey`: the account's channel, with the public key of its viewing key and no
    /// key yet. Refused if the account already has one (it is registered).
    fn set_viewing_key(&mut self) -> Result<(), CompileRefusal> {
        let public_key = stark_public_key(&self.viewing_key)
            .ok()
            .and_then(|key| felt::parse(&key))
            .ok_or(CompileRefusal::InvalidViewingKey)?;
        if self.channels.contains_key(&self.account) {
            return Err(CompileRefusal::AlreadyRegistered);
        }
        self.channels.insert(
            self.account,
            Channel {
                public_key,
                key: None,
                tokens: AddressMap::default(),
            },
        );
        Ok(())
    }

    /// `handleOpenChannel`: the channel's key from the recipient's public key, then the next index.
    fn open_channel(&mut self, recipient: &Felt) -> Result<(), CompileRefusal> {
        let (account, viewing_key) = (self.account, self.viewing_key.clone());
        let channel = self.channel_mut(recipient)?;
        channel.key = Some(compute_channel_key(
            &account,
            &viewing_key,
            recipient,
            &channel.public_key,
        ));
        let next = self.next_channel_index()?;
        self.next_channel_index = Some(next.checked_add(1).ok_or(CompileRefusal::IndexOverflow)?);
        Ok(())
    }

    fn channel_mut(&mut self, recipient: &Felt) -> Result<&mut Channel, CompileRefusal> {
        self.channels
            .get_mut(recipient)
            .ok_or(CompileRefusal::RecipientNotRegistered)
    }
}

/// `execute`: the simulator takes the action, then it joins its phase's list.
fn emit(
    action: ClientAction,
    list: &mut Vec<ClientAction>,
    pool: &mut Simulator,
) -> Result<(), CompileRefusal> {
    pool.execute(&action)?;
    list.push(action);
    Ok(())
}

fn random(entropy: &mut impl Entropy) -> Result<Felt, CompileRefusal> {
    generate_random(entropy).map_err(|_| CompileRefusal::Entropy)
}

/// `transformToClientActions`, phases 1–7, each action executed on the simulator as it is
/// emitted; then the SDK's flattening order.
pub(super) fn transform(
    account: &Felt,
    actions: &Actions,
    use_notes: &[(Felt, &Note)],
    needed: &[Felt],
    pool: &mut Simulator,
    entropy: &mut impl Entropy,
) -> Result<Compiled, CompileRefusal> {
    // `autoSetup`: registering opens the account's own channel first, then every needed
    // recipient whose channel has no key (`addOpenChannel` deduplicates).
    let mut to_open: Vec<Felt> = Vec::new();
    if actions.register {
        to_open.push(*account);
    }
    for recipient in needed {
        let keyed = pool.channel(recipient).is_some_and(|c| c.key.is_some());
        if !keyed && !to_open.contains(recipient) {
            to_open.push(*recipient);
        }
    }

    // 1. SetViewingKey.
    let mut set_viewing_key = Vec::new();
    if actions.register {
        let action = ClientAction::SetViewingKey {
            random: random(entropy)?,
        };
        emit(action, &mut set_viewing_key, pool)?;
    }

    // 2. OpenChannel: `random`, then `salt`.
    let mut open_channels = Vec::new();
    for recipient in &to_open {
        if pool.channel(recipient).is_none() {
            return Err(CompileRefusal::RecipientNotRegistered);
        }
        let index = pool.next_channel_index()?;
        let random_value = random(entropy)?;
        let salt = random(entropy)?;
        let action = ClientAction::OpenChannel {
            recipient_addr: *recipient,
            index,
            random: random_value,
            salt,
        };
        emit(action, &mut open_channels, pool)?;
    }

    // 3. No explicit OpenSubchannels on Bramble's path; 6. adds the implicit ones here.
    let mut open_subchannels = Vec::new();

    // 4. Deposit.
    let mut deposits = Vec::new();
    for (token, amount) in &actions.deposits {
        let action = ClientAction::Deposit {
            token: *token,
            amount: *amount,
        };
        emit(action, &mut deposits, pool)?;
    }

    // 5. UseNote.
    let mut spent = Vec::new();
    for (token, note) in use_notes {
        let action = ClientAction::UseNote {
            channel_key: note.witness.channel_key.clone(),
            token: *token,
            index: note.witness.nonce,
        };
        emit(action, &mut spent, pool)?;
    }

    // 6. CreateEncNote / CreateOpenNote, each after its subchannel if the channel lacks the token.
    let mut created = Vec::new();
    for note in &actions.create_notes {
        let channel = pool
            .channel(&note.recipient)
            .ok_or(CompileRefusal::RecipientNotRegistered)?;
        if !channel.tokens.contains_key(&note.token) {
            let channel_key = channel
                .key
                .clone()
                .ok_or(CompileRefusal::MissingChannelKey)?;
            let action = ClientAction::OpenSubchannel {
                recipient_addr: note.recipient,
                recipient_public_key: channel.public_key,
                channel_key,
                index: u32::try_from(channel.tokens.len())
                    .map_err(|_| CompileRefusal::IndexOverflow)?,
                token: note.token,
                salt: random(entropy)?,
            };
            emit(action, &mut open_subchannels, pool)?;
        }
        let channel = pool
            .channel(&note.recipient)
            .ok_or(CompileRefusal::RecipientNotRegistered)?;
        // Present by now; the SDK's `get` would insert the zero default otherwise.
        let index = channel
            .tokens
            .get(&note.token)
            .map_or(0, |nonces| nonces.note_nonce);
        let action = match note.amount {
            None => ClientAction::CreateOpenNote {
                recipient_addr: note.recipient,
                recipient_public_key: channel.public_key,
                token: note.token,
                index,
                random: random(entropy)?,
            },
            Some(amount) => ClientAction::CreateEncNote {
                recipient_addr: note.recipient,
                recipient_public_key: channel.public_key,
                token: note.token,
                amount,
                index,
                salt: generate_random120(entropy).map_err(|_| CompileRefusal::Entropy)?,
            },
        };
        emit(action, &mut created, pool)?;
    }

    // 7. Withdraw.
    let mut withdraws = Vec::new();
    for (token, recipient, amount) in &actions.withdraws {
        let action = ClientAction::Withdraw {
            to_addr: *recipient,
            token: *token,
            amount: *amount,
            random: random(entropy)?,
        };
        emit(action, &mut withdraws, pool)?;
    }

    let user_linkage = open_channels.len() > 1;
    let actions = [
        set_viewing_key,
        open_channels,
        open_subchannels,
        deposits,
        spent,
        created,
        withdraws,
    ]
    .concat();
    Ok(Compiled {
        actions,
        user_linkage,
    })
}
