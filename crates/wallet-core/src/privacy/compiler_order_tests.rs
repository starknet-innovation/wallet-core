//! What no recorded job covers yet (a generator follow-up records 2 and 3): several notes to one
//! recipient and token in one batch, a deposit riding with a note-funded spend, and a balanced
//! token. Each expected list is traced through the pinned SDK's `compiler.ts`:
//! `transformToClientActions` (phase order of the `clientActions` record, lines 230–240, flattened
//! at 496–498; `transformOpenSubchannel`, 312–341, opens a subchannel only when the channel lacks
//! the token, at index `tokens.size`; each created note takes the channel's `noteNonce`, 392–430,
//! which the simulator's `incrementNoteNonce` then advances) and `resolveNotes` (deficit only on
//! `balance < 0n`, 706–722; a surplus becomes a note to the surplus recipient, 737–769).

use super::*;

fn ready(bob: &[(Felt, u32)], own: &[(Felt, u32)]) -> OutgoingChannels {
    channels(
        vec![(ACCOUNT, keyed(1, own)), (BOB, keyed(2, bob))],
        Some(2),
    )
}

#[test]
fn two_notes_to_one_recipient_and_token_take_consecutive_indices() {
    // Two sends of STRK to Bob, funded by one note of 10, change to the account: Bob's subchannel
    // is opened once, at index 0; his notes take nonces 0 and 1.
    let held = notes(vec![(STRK, vec![note(STRK, 10, 1)])]);
    let reviewed = [send(STRK, BOB, 3), send(STRK, BOB, 4)];
    let (compiled, _) = run(&reviewed, &ready(&[], &[(STRK, 0)]), &held).unwrap();
    assert_eq!(
        shape(&compiled.actions),
        [
            "OpenSubchannel bob strk 0",
            "UseNote strk 1",
            "CreateEncNote bob strk 3 0",
            "CreateEncNote bob strk 4 1",
            "CreateEncNote account strk 3 0",
        ]
    );
    assert!(binds(&compiled, &reviewed));

    // A deposit for Bob and a send to Bob: the deposit's note first, then the send's, one
    // subchannel, nonces 0 and 1; the 2 short is one note of 2, so no change.
    let held = notes(vec![(STRK, vec![note(STRK, 2, 1)])]);
    let reviewed = [shield(STRK, 5, Some(BOB)), send(STRK, BOB, 2)];
    let (compiled, _) = run(&reviewed, &ready(&[], &[(STRK, 0)]), &held).unwrap();
    assert_eq!(
        shape(&compiled.actions),
        [
            "OpenSubchannel bob strk 0",
            "Deposit strk 5",
            "UseNote strk 1",
            "CreateEncNote bob strk 5 0",
            "CreateEncNote bob strk 2 1",
        ]
    );
    assert!(binds(&compiled, &reviewed));
}

#[test]
fn deposits_come_before_spent_notes() {
    // Deposit 5 STRK and send 8: the 3 short takes the note of 4, 1 comes back as change.
    let held = notes(vec![(STRK, vec![note(STRK, 4, 1)])]);
    let reviewed = [shield(STRK, 5, None), send(STRK, BOB, 8)];
    let (compiled, _) = run(&reviewed, &ready(&[(STRK, 0)], &[(STRK, 0)]), &held).unwrap();
    assert_eq!(
        shape(&compiled.actions),
        [
            "Deposit strk 5",
            "UseNote strk 1",
            "CreateEncNote bob strk 8 0",
            "CreateEncNote account strk 1 0",
        ]
    );
    assert!(binds(&compiled, &reviewed));

    // Deposit STRK, send ETH: STRK's surplus is a note to the account, after Bob's ETH note
    // (`balances` order is STRK then ETH; the surplus is pushed to `createNotes` last).
    let held = notes(vec![(ETH, vec![note(ETH, 3, 20)])]);
    let reviewed = [shield(STRK, 5, None), send(ETH, BOB, 3)];
    let (compiled, _) = run(&reviewed, &ready(&[(ETH, 0)], &[(STRK, 0)]), &held).unwrap();
    assert_eq!(
        shape(&compiled.actions),
        [
            "Deposit strk 5",
            "UseNote eth 20",
            "CreateEncNote bob eth 3 0",
            "CreateEncNote account strk 5 0",
        ]
    );
    assert!(binds(&compiled, &reviewed));
}

#[test]
fn a_balanced_token_selects_no_notes() {
    // Deposit 5 and withdraw 5 with a note on hand: balance 0 is no deficit, so no note is
    // spent and no change made.
    let held = notes(vec![(STRK, vec![note(STRK, 10, 1)])]);
    let reviewed = [
        shield(STRK, 5, None),
        ReviewedAction::Withdraw {
            token: STRK,
            recipient: EVE,
            amount: Felt::from(5u8),
        },
    ];
    let (compiled, _) = run(&reviewed, &ready(&[], &[(STRK, 0)]), &held).unwrap();
    assert_eq!(
        shape(&compiled.actions),
        ["Deposit strk 5", "Withdraw eve strk 5"]
    );
}
