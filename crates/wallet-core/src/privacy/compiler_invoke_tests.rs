//! Phases 8 and 9: the invoke phase comes last, draws nothing, resolves its placeholders, and
//! is refused where the extension refuses it.

use super::*;
use crate::privacy::proof::{CollectPolicy, InvokeArg};
use crate::privacy::shadow::{invoke_additional_data, OpenNoteRef};
use crate::send::calls::Call;

/// A registered account with a keyed channel to Bob under the key its viewing key derives.
fn ready() -> OutgoingChannels {
    let mut bob = keyed(2, &[(STRK, 0)]);
    bob.key = Some(compute_channel_key(&ACCOUNT, &key(), &BOB, &Felt::TWO));
    channels(vec![(ACCOUNT, keyed(1, &[(STRK, 0)])), (BOB, bob)], Some(2))
}

fn open(recipient: Felt) -> ReviewedAction {
    ReviewedAction::OpenTransfer {
        token: STRK,
        recipient,
    }
}

fn invoke(calldata: &[InvokeArg]) -> ReviewedAction {
    ReviewedAction::Invoke {
        contract: EVE,
        calldata: calldata.to_vec(),
    }
}

fn shadow() -> ReviewedAction {
    ReviewedAction::ShadowInvoke {
        dapp_name: Felt::from(0xda99u64),
        nonce: Felt::ONE,
        calls: vec![Call {
            contract_address: EVE,
            entrypoint: "ping".into(),
            calldata: vec![],
        }],
        collect: CollectPolicy::Diff,
    }
}

fn bob_note_id() -> Felt {
    compute_note_id(
        &compute_channel_key(&ACCOUNT, &key(), &BOB, &Felt::TWO),
        &STRK,
        0,
    )
}

#[test]
fn an_invoke_comes_last_with_its_placeholders_resolved_and_draws_nothing() {
    let reviewed = [
        invoke(&[
            InvokeArg::PoolAddress,
            InvokeArg::OpenNoteId(0),
            InvokeArg::Felt(Felt::from(5u8)),
        ]),
        open(BOB),
    ];
    let walks = walk_inputs(&ACCOUNT, &reviewed, &CONTRACTS).unwrap();
    assert!(walks.tokens.is_empty());
    let (compiled, reads) = run(&reviewed, &ready(), &AddressMap::default()).unwrap();
    assert_eq!(
        shape(&compiled.actions),
        ["CreateOpenNote bob strk 0", "InvokeExternal eve 3"]
    );
    // The open note's random only.
    assert_eq!(reads, [48]);
    let Some(ClientAction::InvokeExternal { calldata, .. }) = compiled.actions.last() else {
        panic!("no invoke");
    };
    assert_eq!(*calldata, [POOL, bob_note_id(), Felt::from(5u8)]);
    assert!(binds(&compiled, &reviewed));
}

#[test]
fn a_shadow_invoke_runs_through_the_anonymizer_over_the_batchs_open_notes() {
    let reviewed = [open(BOB), shadow()];
    let (compiled, _) = run(&reviewed, &ready(), &AddressMap::default()).unwrap();
    assert_eq!(
        shape(&compiled.actions),
        ["CreateOpenNote bob strk 0", "ComputeAndInvoke 0xa11"]
    );
    let Some(ClientAction::ComputeAndInvoke {
        compute_additional_data,
        invoke_additional_data: data,
        ..
    }) = compiled.actions.last()
    else {
        panic!("no compute-and-invoke");
    };
    assert_eq!(*compute_additional_data, [Felt::from(0xda99u64), Felt::ONE]);
    let ReviewedAction::ShadowInvoke { calls, .. } = shadow() else {
        unreachable!()
    };
    let note = OpenNoteRef {
        note_id: bob_note_id(),
        token: STRK,
    };
    assert_eq!(
        *data,
        invoke_additional_data(&calls, &[note], CollectPolicy::Diff)
    );
    assert!(binds(&compiled, &reviewed));
}

#[test]
fn refusals_fire_where_the_extension_refuses() {
    // Before any request: no anonymizer for a shadow invoke (checked first), then a second
    // invoke-phase action.
    let none = Contracts {
        pool: POOL,
        anonymizer: None,
    };
    let two = [invoke(&[]), shadow()];
    assert_eq!(
        walk_inputs(&ACCOUNT, &two, &none),
        Err(CompileRefusal::NoAnonymizer)
    );
    assert_eq!(
        walk_inputs(&ACCOUNT, &two, &CONTRACTS),
        Err(CompileRefusal::MultipleInvokes)
    );
    assert_eq!(
        walk_inputs(&ACCOUNT, &[invoke(&[]), invoke(&[])], &none),
        Err(CompileRefusal::MultipleInvokes)
    );
    // After the walks: a placeholder past the batch's open notes, once everything is drawn.
    let mut entropy = Counter::default();
    let past = [open(BOB), invoke(&[InvokeArg::OpenNoteId(1)])];
    let result = compile(
        &ACCOUNT,
        &key(),
        &past,
        &CONTRACTS,
        &ready(),
        &AddressMap::default(),
        &mut entropy,
    );
    assert_eq!(result.unwrap_err(), CompileRefusal::OpenNoteOutOfRange);
    assert_eq!(entropy.0, [48]);
}
