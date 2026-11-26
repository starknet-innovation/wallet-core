//! The invoke phase's binding: an invoke or a shadow-account invoke is signed for only when the
//! span carries exactly the reviewed one, its placeholders resolved independently of the
//! compiler.

use super::*;
use crate::privacy::proof::{CollectPolicy, InvokeArg};
use crate::privacy::shadow::{invoke_additional_data, OpenNoteRef};
use crate::send::calls::Call;

const DAPP: Felt = Felt::from_hex_unchecked("0xda99");

fn invoke(contract: Felt, calldata: &[Felt]) -> Vec<Felt> {
    let mut action = vec![f(8), contract, Felt::from(calldata.len())];
    action.extend_from_slice(calldata);
    action
}

fn compute(contract: Felt, compute: &[Felt], data: &[Felt]) -> Vec<Felt> {
    let mut action = vec![f(9), contract, Felt::from(compute.len())];
    action.extend_from_slice(compute);
    action.push(Felt::from(data.len()));
    action.extend_from_slice(data);
    action
}

fn open(recipient: Felt) -> ReviewedAction {
    ReviewedAction::OpenTransfer {
        token: STRK,
        recipient,
    }
}

fn reviewed_invoke(contract: Felt, calldata: &[InvokeArg]) -> ReviewedAction {
    ReviewedAction::Invoke {
        contract,
        calldata: calldata.to_vec(),
    }
}

fn swap() -> Vec<Call> {
    vec![Call {
        contract_address: EVE,
        entrypoint: "swap_exact_tokens".into(),
        calldata: vec![f(1), STRK, f(100)],
    }]
}

fn shadow(nonce: u128, collect: CollectPolicy) -> ReviewedAction {
    ReviewedAction::ShadowInvoke {
        dapp_name: DAPP,
        nonce: f(nonce),
        calls: swap(),
        collect,
    }
}

/// The anonymizer data for `swap()` over the span's open notes.
fn shadow_data(notes: &[(Felt, Felt)], collect: CollectPolicy) -> Vec<Felt> {
    let notes: Vec<OpenNoteRef> = notes
        .iter()
        .map(|(recipient, token)| OpenNoteRef {
            note_id: open_note_id(*recipient, *token),
            token: *token,
        })
        .collect();
    invoke_additional_data(&swap(), &notes, collect)
}

fn unbound() -> Result<(), ProofRefusal> {
    Err(ProofRefusal::UnboundSignRequest)
}

#[test]
fn a_reviewed_invoke_binds_with_its_placeholders_resolved() {
    let review = [
        open(BOB),
        reviewed_invoke(
            EVE,
            &[
                InvokeArg::PoolAddress,
                InvokeArg::OpenNoteId(0),
                InvokeArg::Felt(f(5)),
            ],
        ),
    ];
    let span = |calldata: &[Felt]| {
        vec![
            open_channel(BOB),
            open_note(BOB, STRK),
            invoke(EVE, calldata),
        ]
    };
    let id = open_note_id(BOB, STRK);
    assert_eq!(bound(&span(&[POOL, id, f(5)]), &review), Ok(()));
    // An invoke alone moves something by itself.
    let alone = [reviewed_invoke(EVE, &[InvokeArg::Felt(f(1))])];
    assert_eq!(bound(&[invoke(EVE, &[f(1)])], &alone), Ok(()));
    // Another contract, one felt changed, a felt dropped or added, the pool or note unresolved.
    for calldata in [
        vec![POOL, id, f(6)],
        vec![POOL, id],
        vec![POOL, id, f(5), f(0)],
        vec![f(0), id, f(5)],
        vec![POOL, f(0), f(5)],
        vec![POOL, id + Felt::ONE, f(5)],
    ] {
        assert_eq!(bound(&span(&calldata), &review), unbound(), "{calldata:?}");
    }
    let other = [
        open_channel(BOB),
        open_note(BOB, STRK),
        invoke(BOB, &[POOL, id, f(5)]),
    ];
    assert_eq!(bound(&other, &review), unbound());
}

#[test]
fn a_placeholder_names_the_note_the_span_creates_in_its_order() {
    // Two open notes: index 1 is the second the span creates, recomputed from the span.
    let review = [
        open(BOB),
        open(EVE),
        reviewed_invoke(EVE, &[InvokeArg::OpenNoteId(1)]),
    ];
    let notes = [open_note(BOB, STRK), open_note(EVE, STRK)];
    let with = |id: Felt| [notes[0].clone(), notes[1].clone(), invoke(EVE, &[id])];
    assert_eq!(bound(&with(open_note_id(EVE, STRK)), &review), Ok(()));
    // The other note's id, or one under another key, is refused.
    assert_eq!(bound(&with(open_note_id(BOB, STRK)), &review), unbound());
    let elsewhere = crate::privacy::hashes::compute_note_id(&SecretFelt::new(f(1)), &STRK, 0);
    assert_eq!(bound(&with(elsewhere), &review), unbound());
    // A review that names a note the span does not create never binds.
    let past = [open(BOB), reviewed_invoke(EVE, &[InvokeArg::OpenNoteId(1)])];
    let span = [
        open_note(BOB, STRK),
        invoke(EVE, &[open_note_id(BOB, STRK)]),
    ];
    assert_eq!(bound(&span, &past), unbound());
}

#[test]
fn an_invoke_must_be_reviewed_and_alone() {
    let review = [shield(ONE), reviewed_invoke(EVE, &[])];
    // Missing, unasked for, twice, or beside a compute-and-invoke.
    for actions in [
        vec![deposit(STRK, ONE)],
        vec![deposit(STRK, ONE), invoke(EVE, &[]), invoke(EVE, &[])],
        vec![
            deposit(STRK, ONE),
            invoke(EVE, &[]),
            compute(ANONYMIZER, &[], &[]),
        ],
    ] {
        assert_eq!(bound(&actions, &review), unbound());
    }
    assert_eq!(
        bound(&[deposit(STRK, ONE), invoke(EVE, &[])], &[shield(ONE)]),
        unbound()
    );
    assert_eq!(
        bound(&[deposit(STRK, ONE), invoke(EVE, &[])], &review),
        Ok(())
    );
    // Both kinds reviewed: the pool has one invoke phase, so no span binds.
    let both = [reviewed_invoke(EVE, &[]), shadow(0, CollectPolicy::All)];
    let span = [
        invoke(EVE, &[]),
        compute(
            ANONYMIZER,
            &[DAPP, f(0)],
            &shadow_data(&[], CollectPolicy::All),
        ),
    ];
    assert_eq!(bound(&span, &both), unbound());
}

#[test]
fn a_shadow_invoke_binds_to_the_anonymizer_and_its_data() {
    let collect = CollectPolicy::Exact(100);
    let review = [open(BOB), shadow(3, collect)];
    let span = |contract: Felt, compute_data: &[Felt], data: &[Felt]| {
        vec![
            open_channel(BOB),
            open_note(BOB, STRK),
            compute(contract, compute_data, data),
        ]
    };
    let data = shadow_data(&[(BOB, STRK)], collect);
    assert_eq!(
        bound(&span(ANONYMIZER, &[DAPP, f(3)], &data), &review),
        Ok(())
    );
    let mut changed = data.clone();
    *changed.last_mut().unwrap() = f(101);
    for refused in [
        span(EVE, &[DAPP, f(3)], &data),
        span(ANONYMIZER, &[DAPP, f(4)], &data),
        span(ANONYMIZER, &[f(1), f(3)], &data),
        span(ANONYMIZER, &[DAPP, f(3)], &changed),
        span(
            ANONYMIZER,
            &[DAPP, f(3)],
            &shadow_data(&[(BOB, STRK)], CollectPolicy::Diff),
        ),
        span(ANONYMIZER, &[DAPP, f(3)], &shadow_data(&[], collect)),
    ] {
        assert_eq!(bound(&refused, &review), unbound());
    }
    // No anonymizer configured: nothing binds.
    let calldata = secret(&calldata_for(
        ACCOUNT,
        KEY,
        &span(ANONYMIZER, &[DAPP, f(3)], &data),
    ));
    let none = Contracts {
        pool: POOL,
        anonymizer: None,
    };
    assert_eq!(
        bound_sign_request(&calldata, &ACCOUNT, &key(), &review, &none, &proof_limits()),
        unbound()
    );
}

#[test]
fn both_invoke_variants_round_trip_through_the_decoder() {
    let actions = [
        crate::privacy::client_actions::ClientAction::InvokeExternal {
            contract_address: EVE,
            calldata: vec![f(1), f(2)],
        },
        crate::privacy::client_actions::ClientAction::ComputeAndInvoke {
            contract_address: ANONYMIZER,
            compute_additional_data: vec![DAPP],
            invoke_additional_data: vec![],
        },
    ];
    let calldata =
        crate::privacy::client_actions::compile_actions_calldata(&ACCOUNT, &key(), &actions)
            .unwrap();
    assert_eq!(
        decode_compile_actions(&calldata, &ACCOUNT, &key()),
        Some(vec![
            PoolAction::Invoke {
                contract: EVE,
                calldata: vec![f(1), f(2)],
            },
            PoolAction::ComputeAndInvoke {
                contract: ANONYMIZER,
                compute: vec![DAPP],
                invoke: vec![],
            },
        ])
    );
    // A span longer than the calldata left, or a zero contract, does not decode.
    let long = calldata_for(ACCOUNT, KEY, &[vec![f(8), EVE, f(3), f(1)]]);
    assert!(decode(&long).is_none());
    let huge = calldata_for(ACCOUNT, KEY, &[vec![f(8), EVE, Felt::MAX]]);
    assert!(decode(&huge).is_none());
    assert!(decode(&calldata_for(ACCOUNT, KEY, &[invoke(Felt::ZERO, &[])])).is_none());
}
