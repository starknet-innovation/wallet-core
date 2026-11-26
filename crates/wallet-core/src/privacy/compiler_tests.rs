use super::*;
use crate::privacy::client_actions::compile_actions_calldata;
use crate::privacy::discovery::{Channel, TokenChannel, Witness};
use crate::privacy::pool_actions::{decode_compile_actions, matches_review};
use crate::{compute_channel_key, felt, stark_public_key, Error};

const ACCOUNT: Felt = Felt::from_hex_unchecked("0x123");
const BOB: Felt = Felt::from_hex_unchecked("0xb0b");
const EVE: Felt = Felt::from_hex_unchecked("0xe5e");
const STRK: Felt = Felt::from_hex_unchecked("0x5717");
const ETH: Felt = Felt::from_hex_unchecked("0xe7");
const KEY: Felt = Felt::from_hex_unchecked("0x654321");
const POOL: Felt = Felt::from_hex_unchecked("0x9001");
const ANONYMIZER: Felt = Felt::from_hex_unchecked("0xa11");
const CONTRACTS: Contracts = Contracts {
    pool: POOL,
    anonymizer: Some(ANONYMIZER),
};
/// The sender's channel key the account's notes were created under.
const SENDER_KEY: Felt = Felt::from_hex_unchecked("0x5e4de2");

fn key() -> SecretFelt {
    SecretFelt::new(KEY)
}

/// Deterministic bytes, recording each read's length.
#[derive(Default)]
struct Counter(Vec<usize>);

impl Entropy for Counter {
    fn fill(&mut self, bytes: &mut [u8]) -> Result<(), Error> {
        bytes.fill(u8::try_from(self.0.len() + 1).unwrap_or(0xff));
        self.0.push(bytes.len());
        Ok(())
    }
}

fn keyed(public_key: u64, tokens: &[(Felt, u32)]) -> Channel {
    let mut map = AddressMap::default();
    for (index, (token, nonce)) in tokens.iter().enumerate() {
        let token_index = u32::try_from(index).unwrap();
        map.insert(
            *token,
            TokenChannel {
                token_index,
                note_nonce: *nonce,
            },
        );
    }
    Channel {
        public_key: Felt::from(public_key),
        // Replaced by the derived key in `channels`.
        key: Some(SecretFelt::new(Felt::ONE)),
        tokens: map,
    }
}

fn precomputed(public_key: u64) -> Channel {
    Channel {
        public_key: Felt::from(public_key),
        key: None,
        tokens: AddressMap::default(),
    }
}

/// The walk's result; each keyed channel gets the key the viewing key derives, as the service's
/// must be.
fn channels(entries: Vec<(Felt, Channel)>, total: Option<u32>) -> OutgoingChannels {
    let mut channels = AddressMap::default();
    for (recipient, mut channel) in entries {
        if channel.key.is_some() {
            let derived = compute_channel_key(&ACCOUNT, &key(), &recipient, &channel.public_key);
            channel.key = Some(derived);
        }
        channels.insert(recipient, channel);
    }
    OutgoingChannels { channels, total }
}

fn note(token: Felt, amount: u128, nonce: u32) -> Note {
    let channel_key = SecretFelt::new(SENDER_KEY);
    Note {
        id: compute_note_id(&channel_key, &token, nonce),
        amount,
        witness: Witness {
            channel_key,
            nonce,
            r: SecretFelt::new(Felt::TWO),
        },
        sender: EVE,
        open: false,
    }
}

fn notes(entries: Vec<(Felt, Vec<Note>)>) -> SpendableNotes {
    let mut notes = AddressMap::default();
    for (token, list) in entries {
        notes.insert(token, list);
    }
    notes.into()
}

fn send(token: Felt, recipient: Felt, amount: u128) -> ReviewedAction {
    ReviewedAction::Transfer {
        token,
        recipient,
        amount: Felt::from(amount),
    }
}

fn shield(token: Felt, amount: u128, recipient: Option<Felt>) -> ReviewedAction {
    ReviewedAction::Deposit {
        token,
        amount: Felt::from(amount),
        recipient,
    }
}

fn run(
    reviewed: &[ReviewedAction],
    found: &OutgoingChannels,
    notes: &SpendableNotes,
) -> Result<(Compiled, Vec<usize>), CompileRefusal> {
    let mut entropy = Counter::default();
    let compiled = compile(
        &ACCOUNT,
        &key(),
        reviewed,
        &CONTRACTS,
        found,
        notes,
        &mut entropy,
    )?;
    Ok((compiled, entropy.0))
}

fn refused(
    reviewed: &[ReviewedAction],
    found: &OutgoingChannels,
    notes: &SpendableNotes,
) -> CompileRefusal {
    run(reviewed, found, notes).map(|_| ()).unwrap_err()
}

fn name(felt: &Felt) -> String {
    let names = [
        (ACCOUNT, "account"),
        (BOB, "bob"),
        (EVE, "eve"),
        (STRK, "strk"),
        (ETH, "eth"),
    ];
    names
        .iter()
        .find(|(value, _)| value == felt)
        .map_or_else(|| format!("{felt:#x}"), |(_, name)| (*name).to_owned())
}

/// Each action's variant and public members; randoms and keys left out.
fn shape(actions: &[ClientAction]) -> Vec<String> {
    actions
        .iter()
        .map(|action| match action {
            ClientAction::SetViewingKey { .. } => "SetViewingKey".into(),
            ClientAction::OpenChannel {
                recipient_addr,
                index,
                ..
            } => format!("OpenChannel {} {index}", name(recipient_addr)),
            ClientAction::OpenSubchannel {
                recipient_addr,
                token,
                index,
                ..
            } => format!(
                "OpenSubchannel {} {} {index}",
                name(recipient_addr),
                name(token)
            ),
            ClientAction::CreateEncNote {
                recipient_addr,
                token,
                amount,
                index,
                ..
            } => format!(
                "CreateEncNote {} {} {amount} {index}",
                name(recipient_addr),
                name(token)
            ),
            ClientAction::CreateOpenNote {
                recipient_addr,
                token,
                index,
                ..
            } => format!(
                "CreateOpenNote {} {} {index}",
                name(recipient_addr),
                name(token)
            ),
            ClientAction::Deposit { token, amount } => format!("Deposit {} {amount}", name(token)),
            ClientAction::UseNote { token, index, .. } => {
                format!("UseNote {} {index}", name(token))
            }
            ClientAction::Withdraw {
                to_addr,
                token,
                amount,
                ..
            } => format!("Withdraw {} {} {amount}", name(to_addr), name(token)),
            ClientAction::InvokeExternal {
                contract_address,
                calldata,
            } => format!(
                "InvokeExternal {} {}",
                name(contract_address),
                calldata.len()
            ),
            ClientAction::ComputeAndInvoke {
                contract_address, ..
            } => format!("ComputeAndInvoke {}", name(contract_address)),
        })
        .collect()
}

/// `pool_actions`' binding over our own `compile_actions` calldata.
fn binds(compiled: &Compiled, reviewed: &[ReviewedAction]) -> bool {
    let calldata = compile_actions_calldata(&ACCOUNT, &key(), &compiled.actions).unwrap();
    decode_compile_actions(&calldata, &ACCOUNT, &key())
        .is_some_and(|decoded| matches_review(&decoded, reviewed, &ACCOUNT, &CONTRACTS))
}

#[test]
fn a_send_groups_by_token_selects_largest_first_stably_and_returns_change() {
    let reviewed = [
        send(STRK, BOB, 3),
        ReviewedAction::Withdraw {
            token: ETH,
            recipient: EVE,
            amount: Felt::TWO,
        },
        send(STRK, EVE, 1),
    ];
    // Recipients: the account, then note recipients token by token (a withdrawal's needs no
    // channel); tokens in `balances` order, where withdrawals come before created notes.
    assert_eq!(
        walk_inputs(&ACCOUNT, &reviewed, &CONTRACTS).unwrap(),
        WalkInputs {
            recipients: vec![ACCOUNT, BOB, EVE],
            tokens: vec![ETH, STRK],
        }
    );
    let found = channels(
        vec![
            (ACCOUNT, keyed(1, &[(STRK, 4), (ETH, 2)])),
            (BOB, keyed(2, &[(STRK, 9)])),
            (EVE, keyed(3, &[(ETH, 0)])),
        ],
        Some(3),
    );
    // The deficit of 4 STRK takes the first of the two 4s and stops there.
    let held = notes(vec![
        (
            STRK,
            vec![
                note(STRK, 3, 10),
                note(STRK, 4, 11),
                note(STRK, 4, 12),
                note(STRK, 1, 13),
            ],
        ),
        (ETH, vec![note(ETH, 5, 20)]),
    ]);
    let (compiled, reads) = run(&reviewed, &found, &held).unwrap();
    assert_eq!(
        shape(&compiled.actions),
        [
            "OpenSubchannel eve strk 1",
            "UseNote eth 20",
            "UseNote strk 11",
            "CreateEncNote bob strk 3 9",
            "CreateEncNote eve strk 1 0",
            "CreateEncNote account eth 3 2",
            "Withdraw eve eth 2",
        ]
    );
    // Draws in execution order: the subchannel's salt between the two notes it sits between.
    assert_eq!(reads, [15, 48, 15, 15, 48]);
    assert!(!compiled.user_linkage);
    assert!(binds(&compiled, &reviewed));
}

#[test]
fn registering_opens_the_own_channel_first_with_the_viewing_public_key() {
    let reviewed = [
        ReviewedAction::Register,
        shield(STRK, 7, Some(BOB)),
        shield(STRK, 3, None),
    ];
    let walks = walk_inputs(&ACCOUNT, &reviewed, &CONTRACTS).unwrap();
    assert_eq!(walks.recipients, [ACCOUNT, BOB]);
    assert!(walks.tokens.is_empty());
    let found = channels(vec![(BOB, precomputed(2))], Some(5));
    let (compiled, reads) = run(&reviewed, &found, &SpendableNotes::default()).unwrap();
    assert_eq!(
        shape(&compiled.actions),
        [
            "SetViewingKey",
            "OpenChannel account 5",
            "OpenChannel bob 6",
            "OpenSubchannel bob strk 0",
            "OpenSubchannel account strk 0",
            "Deposit strk 7",
            "Deposit strk 3",
            "CreateEncNote bob strk 7 0",
            "CreateEncNote account strk 3 0",
        ]
    );
    assert_eq!(reads, [48, 48, 48, 48, 48, 48, 15, 48, 15]);
    assert!(compiled.user_linkage);
    assert!(binds(&compiled, &reviewed));
    // Each channel key is computed from the recipient's public key; the account's own is its
    // viewing key's.
    let own = felt::parse(&stark_public_key(&key()).unwrap()).unwrap();
    let opened: Vec<(Felt, Felt, SecretFelt)> = compiled
        .actions
        .iter()
        .filter_map(|action| match action {
            ClientAction::OpenSubchannel {
                recipient_addr,
                recipient_public_key,
                channel_key,
                ..
            } => Some((*recipient_addr, *recipient_public_key, channel_key.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        opened,
        [
            (
                BOB,
                Felt::TWO,
                compute_channel_key(&ACCOUNT, &key(), &BOB, &Felt::TWO)
            ),
            (
                ACCOUNT,
                own,
                compute_channel_key(&ACCOUNT, &key(), &ACCOUNT, &own)
            ),
        ]
    );
}

#[test]
fn an_open_transfer_touches_no_balance() {
    let reviewed = [ReviewedAction::OpenTransfer {
        token: STRK,
        recipient: BOB,
    }];
    let walks = walk_inputs(&ACCOUNT, &reviewed, &CONTRACTS).unwrap();
    assert_eq!(walks.recipients, [ACCOUNT, BOB]);
    assert!(walks.tokens.is_empty());
    // No channel to open, so no total is needed.
    let found = channels(
        vec![(ACCOUNT, keyed(1, &[])), (BOB, keyed(2, &[(STRK, 4)]))],
        None,
    );
    let (compiled, reads) = run(&reviewed, &found, &SpendableNotes::default()).unwrap();
    assert_eq!(shape(&compiled.actions), ["CreateOpenNote bob strk 4"]);
    assert_eq!(reads, [48]);
    assert!(binds(&compiled, &reviewed));
}

#[test]
fn every_refusal_is_its_own_variant() {
    let none = SpendableNotes::default();
    let ready = channels(
        vec![(ACCOUNT, keyed(1, &[(STRK, 0)])), (BOB, keyed(2, &[]))],
        Some(2),
    );
    for bad in [Felt::ZERO, Felt::from(u128::MAX) + Felt::ONE] {
        let reviewed = [ReviewedAction::Deposit {
            token: STRK,
            amount: bad,
            recipient: None,
        }];
        assert_eq!(
            walk_inputs(&ACCOUNT, &reviewed, &CONTRACTS).unwrap_err(),
            CompileRefusal::InvalidAmount
        );
        assert_eq!(
            refused(&reviewed, &ready, &none),
            CompileRefusal::InvalidAmount
        );
    }
    assert_eq!(
        refused(
            &[ReviewedAction::Register, shield(STRK, 1, None)],
            &ready,
            &none
        ),
        CompileRefusal::AlreadyRegistered
    );
    assert_eq!(
        refused(&[shield(STRK, 1, Some(EVE))], &ready, &none),
        CompileRefusal::RecipientNotRegistered
    );
    let unopened = channels(vec![(ACCOUNT, keyed(1, &[])), (BOB, precomputed(2))], None);
    assert_eq!(
        refused(&[shield(STRK, 1, Some(BOB))], &unopened, &none),
        CompileRefusal::MissingChannelTotal
    );
    // A created channel whose key is not the viewing key's, before anything is drawn.
    let mut wrong = channels(
        vec![(ACCOUNT, keyed(1, &[])), (BOB, keyed(2, &[]))],
        Some(2),
    );
    if let Some(bob) = wrong.channels.get_mut(&BOB) {
        bob.key = Some(SecretFelt::new(Felt::from(2000u64)));
    }
    let mut entropy = Counter::default();
    let result = compile(
        &ACCOUNT,
        &key(),
        &[shield(STRK, 1, Some(BOB))],
        &CONTRACTS,
        &wrong,
        &SpendableNotes::default(),
        &mut entropy,
    );
    assert_eq!(result.unwrap_err(), CompileRefusal::ChannelKeyMismatch);
    assert!(entropy.0.is_empty());
    let mut forged = note(STRK, 5, 1);
    forged.witness.nonce = 2;
    assert_eq!(
        refused(
            &[send(STRK, BOB, 1)],
            &ready,
            &notes(vec![(STRK, vec![forged])])
        ),
        CompileRefusal::WitnessMismatch
    );
    // Short: refused before anything is drawn.
    let mut entropy = Counter::default();
    let short = notes(vec![(STRK, vec![note(STRK, 2, 1), note(STRK, 2, 2)])]);
    let result = compile(
        &ACCOUNT,
        &key(),
        &[send(STRK, BOB, 5)],
        &CONTRACTS,
        &ready,
        &short,
        &mut entropy,
    );
    assert_eq!(result.unwrap_err(), CompileRefusal::InsufficientBalance);
    assert!(entropy.0.is_empty());
}

#[path = "compiler_fixture_tests.rs"]
mod fixture;

#[path = "compiler_invoke_tests.rs"]
mod invoke;
