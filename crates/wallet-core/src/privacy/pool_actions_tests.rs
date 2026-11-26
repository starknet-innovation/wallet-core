use super::*;
use crate::privacy::config::testing::proof_limits;

const ACCOUNT: Felt = Felt::from_hex_unchecked("0x123");
const POOL: Felt = Felt::from_hex_unchecked("0x9001");
const BOB: Felt = Felt::from_hex_unchecked("0xb0b");
const EVE: Felt = Felt::from_hex_unchecked("0xe5e");
const STRK: Felt =
    Felt::from_hex_unchecked("0x04718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d");
const KEY: Felt = Felt::from_hex_unchecked("0x654321");

fn key() -> SecretFelt {
    SecretFelt::new(KEY)
}

fn f(value: u128) -> Felt {
    Felt::from(value)
}

// The extension's `tests/support/multisig-callset.ts` builders: `[variant, ...members]`.
fn set_viewing_key() -> Vec<Felt> {
    vec![f(0), f(5)]
}
fn open_channel(recipient: Felt) -> Vec<Felt> {
    vec![f(1), recipient, f(0), f(1), f(2)]
}
fn deposit(token: Felt, amount: u128) -> Vec<Felt> {
    vec![f(5), token, f(amount)]
}
fn use_note(token: Felt) -> Vec<Felt> {
    vec![f(6), f(8), token, f(0)]
}
fn enc_note(recipient: Felt, token: Felt, amount: u128) -> Vec<Felt> {
    vec![f(3), recipient, f(9), token, f(amount), f(2), f(4)]
}
fn withdraw(recipient: Felt, token: Felt, amount: u128) -> Vec<Felt> {
    vec![f(7), recipient, token, f(amount), f(6)]
}
fn invoke_external(target: Felt) -> Vec<Felt> {
    vec![f(8), target, f(0)]
}
fn open_note(recipient: Felt, token: Felt) -> Vec<Felt> {
    vec![f(4), recipient, f(9), token, f(0), f(1)]
}

fn calldata_for(user: Felt, key: Felt, actions: &[Vec<Felt>]) -> Vec<Felt> {
    let mut calldata = vec![user, key, Felt::from(actions.len())];
    calldata.extend(actions.iter().flatten());
    calldata
}

fn decode(calldata: &[Felt]) -> Option<Vec<PoolAction>> {
    decode_compile_actions(calldata, &ACCOUNT, &key())
}

const ONE: u128 = 1_000_000_000_000_000_000;

// `strk20-multisig-decoded-review.test.ts`: every value-moving action is decoded.
#[test]
fn decodes_every_action_the_flows_produce() {
    let calldata = calldata_for(
        ACCOUNT,
        KEY,
        &[
            set_viewing_key(),
            open_channel(ACCOUNT),
            deposit(STRK, 3 * ONE),
            use_note(STRK),
            enc_note(BOB, STRK, ONE + ONE / 2),
            enc_note(ACCOUNT, STRK, ONE / 4),
            withdraw(BOB, STRK, 2 * ONE),
        ],
    );
    assert_eq!(
        decode(&calldata),
        Some(vec![
            PoolAction::Register,
            PoolAction::Channel { recipient: ACCOUNT },
            PoolAction::Deposit {
                token: STRK,
                amount: f(3 * ONE)
            },
            PoolAction::UseNote,
            PoolAction::Note {
                recipient: BOB,
                token: STRK,
                amount: f(ONE + ONE / 2)
            },
            PoolAction::Note {
                recipient: ACCOUNT,
                token: STRK,
                amount: f(ONE / 4)
            },
            PoolAction::Withdraw {
                recipient: BOB,
                token: STRK,
                amount: f(2 * ONE)
            },
        ])
    );
}

// The same file: anything the flows do not produce is refused rather than signed.
#[test]
fn refuses_what_the_flows_do_not_produce() {
    let one_deposit = [deposit(STRK, ONE)];
    assert!(decode(&calldata_for(EVE, KEY, &one_deposit)).is_none());
    assert!(decode(&calldata_for(ACCOUNT, KEY, &[invoke_external(BOB)])).is_none());
    assert!(decode(&calldata_for(ACCOUNT, KEY, &[open_note(BOB, STRK)])).is_none());
    // Felts Cairo could not deserialize as the ABI's u32 index or u128 salt.
    let mut wide_index = open_channel(BOB);
    wide_index[2] = Felt::from(1u64 << 32);
    assert!(decode(&calldata_for(
        ACCOUNT,
        KEY,
        &[wide_index, deposit(STRK, ONE)]
    ))
    .is_none());
    let mut wide_salt = enc_note(BOB, STRK, ONE);
    wide_salt[6] = Felt::from(u128::MAX) + Felt::ONE;
    assert!(decode(&calldata_for(ACCOUNT, KEY, &[wide_salt])).is_none());
    // An amount over u128, a zero token or recipient, an unknown variant.
    let mut wide_amount = deposit(STRK, 0);
    wide_amount[2] = Felt::from(u128::MAX) + Felt::ONE;
    assert!(decode(&calldata_for(ACCOUNT, KEY, &[wide_amount])).is_none());
    assert!(decode(&calldata_for(ACCOUNT, KEY, &[deposit(Felt::ZERO, ONE)])).is_none());
    assert!(decode(&calldata_for(
        ACCOUNT,
        KEY,
        &[withdraw(Felt::ZERO, STRK, ONE)]
    ))
    .is_none());
    assert!(decode(&calldata_for(ACCOUNT, KEY, &[vec![f(10), f(1)]])).is_none());
    // Trailing calldata, a short span, a count over the bound, a missing header.
    let mut trailing = calldata_for(ACCOUNT, KEY, &one_deposit);
    trailing.push(Felt::ONE);
    assert!(decode(&trailing).is_none());
    let mut short = calldata_for(ACCOUNT, KEY, &one_deposit);
    short.pop();
    assert!(decode(&short).is_none());
    let mut counted = calldata_for(ACCOUNT, KEY, &one_deposit);
    counted[2] = f(65);
    assert!(decode(&counted).is_none());
    assert!(decode(&[ACCOUNT, KEY]).is_none());
    // The key must sit in the key slot.
    assert!(decode(&calldata_for(ACCOUNT, Felt::ONE, &one_deposit)).is_none());
}

fn shield(amount: u128) -> EngineAction {
    EngineAction::Deposit {
        token: STRK,
        amount: f(amount),
        recipient: None,
    }
}

fn send(recipient: Felt, amount: u128) -> EngineAction {
    EngineAction::Transfer {
        token: STRK,
        recipient,
        amount: f(amount),
    }
}

fn unshield(recipient: Felt, amount: u128) -> EngineAction {
    EngineAction::Withdraw {
        token: STRK,
        recipient,
        amount: f(amount),
    }
}

fn bound(actions: &[Vec<Felt>], reviewed: &[EngineAction]) -> Result<(), ProofRefusal> {
    bound_call(calldata_for(ACCOUNT, KEY, actions), reviewed)
}

fn bound_call(calldata: Vec<Felt>, reviewed: &[EngineAction]) -> Result<(), ProofRefusal> {
    let call = Call {
        contract_address: POOL,
        entrypoint: COMPILE_ACTIONS.into(),
        calldata,
    };
    bound_sign_request(&[call], &POOL, &ACCOUNT, &key(), reviewed, &proof_limits())
}

#[test]
fn binds_each_flow_to_what_was_reviewed() {
    // Activation: register, the self channel, then the deposit.
    let activate = [set_viewing_key(), open_channel(ACCOUNT), deposit(STRK, ONE)];
    assert_eq!(
        bound(&activate, &[EngineAction::Register, shield(ONE)]),
        Ok(())
    );
    // A private send: notes spent, the recipient's note, change back to the account.
    let private_send = [
        open_channel(BOB),
        use_note(STRK),
        enc_note(BOB, STRK, ONE),
        enc_note(ACCOUNT, STRK, ONE / 2),
    ];
    assert_eq!(bound(&private_send, &[send(BOB, ONE)]), Ok(()));
    // An unshield with change.
    let out = [
        use_note(STRK),
        withdraw(BOB, STRK, ONE),
        enc_note(ACCOUNT, STRK, 7),
    ];
    assert_eq!(bound(&out, &[unshield(BOB, ONE)]), Ok(()));
    // A deposit for someone else carries their note.
    let gift = EngineAction::Deposit {
        token: STRK,
        amount: f(ONE),
        recipient: Some(BOB),
    };
    let gifted = [deposit(STRK, ONE), enc_note(BOB, STRK, ONE)];
    assert_eq!(bound(&gifted, std::slice::from_ref(&gift)), Ok(()));
    assert_eq!(
        bound(&[deposit(STRK, ONE)], &[gift]),
        Err(ProofRefusal::UnboundSignRequest)
    );
}

// One test per way a hostile engine could ask for more than was reviewed.
#[test]
fn a_swapped_recipient_is_refused() {
    let to_eve = [use_note(STRK), enc_note(EVE, STRK, ONE)];
    assert_eq!(
        bound(&to_eve, &[send(BOB, ONE)]),
        Err(ProofRefusal::UnboundSignRequest)
    );
    let out_to_eve = [withdraw(EVE, STRK, ONE)];
    assert_eq!(
        bound(&out_to_eve, &[unshield(BOB, ONE)]),
        Err(ProofRefusal::UnboundSignRequest)
    );
}

#[test]
fn an_extra_withdraw_is_refused() {
    let extra = [
        use_note(STRK),
        enc_note(BOB, STRK, ONE),
        withdraw(EVE, STRK, ONE),
    ];
    assert_eq!(
        bound(&extra, &[send(BOB, ONE)]),
        Err(ProofRefusal::UnboundSignRequest)
    );
    // Even a second copy of the reviewed withdrawal.
    let twice = [withdraw(BOB, STRK, ONE), withdraw(BOB, STRK, ONE)];
    assert_eq!(
        bound(&twice, &[unshield(BOB, ONE)]),
        Err(ProofRefusal::UnboundSignRequest)
    );
}

#[test]
fn an_external_invoke_is_refused() {
    let invoke = [deposit(STRK, ONE), invoke_external(EVE)];
    assert_eq!(
        bound(&invoke, &[shield(ONE)]),
        Err(ProofRefusal::UnboundSignRequest)
    );
}

#[test]
fn a_swapped_key_slot_is_refused() {
    let calldata = calldata_for(ACCOUNT, Felt::from(0x777u64), &[deposit(STRK, ONE)]);
    assert_eq!(
        bound_call(calldata, &[shield(ONE)]),
        Err(ProofRefusal::UnboundSignRequest)
    );
}

#[test]
fn trailing_calldata_is_refused() {
    let mut calldata = calldata_for(ACCOUNT, KEY, &[deposit(STRK, ONE)]);
    calldata.extend(withdraw(EVE, STRK, ONE));
    assert_eq!(
        bound_call(calldata, &[shield(ONE)]),
        Err(ProofRefusal::UnboundSignRequest)
    );
}

#[test]
fn a_missing_or_altered_action_and_an_unasked_registration_are_refused() {
    for (actions, reviewed) in [
        // Nothing moved for a reviewed send.
        (
            vec![open_channel(BOB), use_note(STRK)],
            vec![send(BOB, ONE)],
        ),
        // Another amount or token.
        (vec![deposit(STRK, ONE + 1)], vec![shield(ONE)]),
        (vec![deposit(EVE, ONE)], vec![shield(ONE)]),
        // A registration nobody asked for, and one missing.
        (
            vec![set_viewing_key(), deposit(STRK, ONE)],
            vec![shield(ONE)],
        ),
        (
            vec![deposit(STRK, ONE)],
            vec![EngineAction::Register, shield(ONE)],
        ),
    ] {
        assert_eq!(
            bound(&actions, &reviewed),
            Err(ProofRefusal::UnboundSignRequest)
        );
    }
}

#[test]
fn only_the_pools_one_compile_actions_is_considered() {
    let calldata = calldata_for(ACCOUNT, KEY, &[deposit(STRK, ONE)]);
    let call = |pool: Felt, entrypoint: &str| Call {
        contract_address: pool,
        entrypoint: entrypoint.into(),
        calldata: calldata.clone(),
    };
    let reviewed = [shield(ONE)];
    let check = |calls: &[Call]| {
        bound_sign_request(calls, &POOL, &ACCOUNT, &key(), &reviewed, &proof_limits())
    };
    assert_eq!(check(&[call(POOL, COMPILE_ACTIONS)]), Ok(()));
    for refused in [
        vec![],
        vec![call(POOL, COMPILE_ACTIONS), call(POOL, COMPILE_ACTIONS)],
        vec![call(EVE, COMPILE_ACTIONS)],
        vec![call(POOL, "apply_actions")],
    ] {
        assert_eq!(check(&refused), Err(ProofRefusal::UnboundSignRequest));
    }
    let mut limits = proof_limits();
    limits.invoke_calldata_felts = calldata.len() - 1;
    assert_eq!(
        bound_sign_request(
            &[call(POOL, COMPILE_ACTIONS)],
            &POOL,
            &ACCOUNT,
            &key(),
            &reviewed,
            &limits
        ),
        Err(ProofRefusal::UnboundSignRequest)
    );
}

#[test]
fn an_empty_review_or_a_span_that_moves_nothing_is_refused() {
    let setup_only = [open_channel(ACCOUNT), use_note(STRK)];
    assert_eq!(
        bound(&setup_only, &[]),
        Err(ProofRefusal::UnboundSignRequest)
    );
    assert_eq!(bound(&[], &[]), Err(ProofRefusal::UnboundSignRequest));
    assert_eq!(
        bound(&setup_only, &[send(BOB, ONE)]),
        Err(ProofRefusal::UnboundSignRequest)
    );
}

#[test]
fn a_channel_to_an_unreviewed_recipient_is_refused() {
    let to_eve = [open_channel(EVE), use_note(STRK), enc_note(BOB, STRK, ONE)];
    assert_eq!(
        bound(&to_eve, &[send(BOB, ONE)]),
        Err(ProofRefusal::UnboundSignRequest)
    );
    // A withdrawal's recipient gets no note, so no channel either.
    let out = [open_channel(BOB), withdraw(BOB, STRK, ONE)];
    assert_eq!(
        bound(&out, &[unshield(BOB, ONE)]),
        Err(ProofRefusal::UnboundSignRequest)
    );
    // Channels to the account and to the reviewed note recipient are allowed.
    let to_bob = [
        open_channel(ACCOUNT),
        open_channel(BOB),
        use_note(STRK),
        enc_note(BOB, STRK, ONE),
    ];
    assert_eq!(bound(&to_bob, &[send(BOB, ONE)]), Ok(()));
}
