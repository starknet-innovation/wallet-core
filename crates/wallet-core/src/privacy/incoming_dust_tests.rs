//! Dust cannot block a spend: thousands of tiny notes beside a few large ones, culled by the walk
//! and spent by the compiler exactly as the SDK's largest-first rule (`compiler.ts`
//! `resolveNotes`: a stable sort by `b.amount - a.amount`, taken until the deficit is covered)
//! picks over all of them.

use std::cmp::Reverse;

use super::*;
use crate::compute_channel_key;
use crate::privacy::client_actions::ClientAction;
use crate::privacy::compiler::{compile, CompileRefusal, Compiled, Contracts};
use crate::privacy::discovery::{Channel, TokenChannel};
use crate::privacy::entropy::Entropy;
use crate::privacy::hashes::compute_note_id;
use crate::privacy::outgoing::OutgoingChannels;
use crate::privacy::proof::ReviewedAction;
use crate::Error;

const SENDER_KEY: &str = "0x3c";
const BOB: &str = "0xb0b";

struct Zeros;

impl Entropy for Zeros {
    fn fill(&mut self, bytes: &mut [u8]) -> Result<(), Error> {
        bytes.fill(0);
        Ok(())
    }
}

/// One page of T1 notes from sender 0x5, `amounts[i]` at index `i`, with real note ids.
fn notes_page(amounts: &[u128]) -> String {
    let key = SecretFelt::new(felt(SENDER_KEY));
    let notes: Vec<String> = amounts
        .iter()
        .enumerate()
        .map(|(at, amount)| {
            let index = u32::try_from(at).unwrap();
            let id = crate::felt::canonical(&compute_note_id(&key, &felt(T1), index));
            note("0x5", T1, index, &id, &amount.to_string(), "3")
        })
        .collect();
    let channels = format!(r#"{{"channel_key":"{SENDER_KEY}","sender_addr":"0x5"}}"#);
    page(&channels, &notes, DONE)
}

/// 3,000 notes of 1 with 1,000, 700 and 500 among them.
fn dusted() -> Vec<u128> {
    let mut amounts = vec![1u128; 3_003];
    for (at, amount) in [(500, 1_000), (1_500, 700), (2_500, 500)] {
        amounts[at] = amount;
    }
    amounts
}

/// The SDK's selection over every note: indices, largest first in arrival order, until covered.
fn sdk_selection(amounts: &[u128], deficit: u128) -> Option<Vec<u32>> {
    let mut sorted: Vec<(usize, u128)> = amounts.iter().copied().enumerate().collect();
    sorted.sort_by_key(|(_, amount)| Reverse(*amount));
    let mut covered = 0;
    let mut picked = Vec::new();
    for (at, amount) in sorted {
        picked.push(u32::try_from(at).unwrap());
        covered += amount;
        if covered >= deficit {
            return Some(picked);
        }
    }
    None
}

fn spend(found: &SpendableNotes, amount: u128) -> Result<Compiled, CompileRefusal> {
    let (account, vk) = (felt("0x208"), key());
    let keyed = |recipient: Felt, public_key: u64| Channel {
        public_key: Felt::from(public_key),
        key: Some(compute_channel_key(
            &account,
            &vk,
            &recipient,
            &Felt::from(public_key),
        )),
        tokens: {
            let mut tokens = AddressMap::default();
            tokens.insert(felt(T1), TokenChannel::default());
            tokens
        },
    };
    let mut channels = AddressMap::default();
    channels.insert(account, keyed(account, 1));
    channels.insert(felt(BOB), keyed(felt(BOB), 2));
    let reviewed = [ReviewedAction::Transfer {
        token: felt(T1),
        recipient: felt(BOB),
        amount: Felt::from(amount),
    }];
    compile(
        &account,
        &vk,
        &reviewed,
        &Contracts {
            pool: felt("0x1"),
            anonymizer: None,
        },
        &OutgoingChannels {
            channels,
            total: Some(2),
        },
        found,
        &mut Zeros,
    )
}

fn spent(compiled: &Compiled) -> Vec<u32> {
    compiled
        .actions
        .iter()
        .filter_map(|action| match action {
            ClientAction::UseNote { index, .. } => Some(*index),
            _ => None,
        })
        .collect()
}

#[test]
fn dust_is_culled_and_the_spend_takes_what_the_sdk_takes() {
    let amounts = dusted();
    let found = spendable(walk_with(64), &notes_page(&amounts));
    // Kept: the three large notes and the 61 earliest dust notes; the other 2,939 are dropped.
    let kept: Vec<u32> = found
        .notes
        .get(&felt(T1))
        .unwrap()
        .iter()
        .map(|n| n.witness.nonce)
        .collect();
    let mut expected: Vec<u32> = (0..61).collect();
    expected.extend([500, 1_500, 2_500]);
    assert_eq!(kept, expected);
    assert_eq!(found.dropped.get(&felt(T1)), Some(&U256::from_u128(2_939)));

    // Whenever the SDK's selection fits, it is exactly the one made over the kept notes.
    for deficit in [1, 700, 1_000, 1_500, 2_200, 2_250] {
        let compiled = spend(&found, deficit).unwrap();
        assert_eq!(
            Some(spent(&compiled)),
            sdk_selection(&amounts, deficit),
            "{deficit}"
        );
    }
    // Every kept note: 64 spends and the note to Bob are past one batch.
    assert_eq!(
        spend(&found, 2_261).err(),
        Some(CompileRefusal::TooManyNotes)
    );
    // Held, but only with dropped notes: consolidate. Not held at all: insufficient.
    assert_eq!(
        spend(&found, 3_500).err(),
        Some(CompileRefusal::TooManyNotes)
    );
    assert_eq!(
        spend(&found, 5_201).err(),
        Some(CompileRefusal::InsufficientBalance)
    );
    assert!(spend(&found, 2_262).is_err(), "kept 2,261, dropped 1s");
}

#[test]
fn a_selection_past_one_batch_is_too_many_notes() {
    // 64 notes of 10, none dropped. 63 of them and the note to Bob are 64 actions: they fit.
    let found = spendable(walk_with(64), &notes_page(&[10; 64]));
    assert!(found.dropped.is_empty());
    assert_eq!(spent(&spend(&found, 630).unwrap()).len(), 63);
    // All 64 and the note to Bob are 65: past `MAX_ACTIONS`, and one consolidated note fits.
    assert_eq!(spend(&found, 640).err(), Some(CompileRefusal::TooManyNotes));
    // A wallet's larger `max_notes` is held to what one batch could spend.
    let found = spendable(walk_with(10_000), &notes_page(&[1; 100]));
    assert_eq!(found.notes.get(&felt(T1)).unwrap().len(), MAX_ACTIONS);
}
