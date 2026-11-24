//! The submission's order and its refusals; G7h's `review` block drives it end to end in
//! `bramble-core`'s `gasless_parity.rs`.

use super::*;
use crate::accounts::AccountFamily;
use crate::gasless::tests::{call, felt_of, v2_classes, POLICY};
use crate::gasless::typed_data::tests::{standard, NOW, RECIPIENT, USDC};
use crate::gasless::{GaslessAccount, UserFee};
use crate::NetworkId;
use serde_json::{json, Value};

const ACCOUNT: &str = "0x5dccf9cfdcabb9e4ef45814bda1b056bb23063d0e9b64e5a845e9dfb3d23f10";
const ARGENT_04: &str = "0x36078334509b514626504edc9fb252328d1a240e4e948bef8d0c08dff45927f";

fn session() -> GaslessSession<'static> {
    let account = GaslessAccount {
        network: NetworkId::Mainnet,
        address: felt_of(ACCOUNT),
        family: AccountFamily::Argent,
        class_hash: felt_of(ARGENT_04),
        deployed: true,
        deployment: None,
    };
    let calls = [call(USDC, "transfer", &[RECIPIENT, "0x10", "0x0"])];
    GaslessSession::open(&POLICY, &account, &calls, &v2_classes()).unwrap()
}

fn reviewed() -> GaslessFeeOffer {
    GaslessFeeOffer {
        amount: "2000".into(),
        decimals: 6,
        label: "USDC".into(),
        token_address: USDC.into(),
    }
}

fn reply(status: u16, body: Value) -> HttpOutcome {
    HttpOutcome {
        status,
        retry_after: None,
        body: Some(body.to_string().into_bytes()),
        timed_out: false,
    }
}

fn result(value: Value) -> HttpOutcome {
    reply(200, json!({"id": 1, "jsonrpc": "2.0", "result": value}))
}

fn built() -> HttpOutcome {
    result(json!({
        "fee": {
            "estimated_fee_in_gas_token": "0x1",
            "estimated_fee_in_strk": "0x1",
            "gas_token_price_in_strk": "0x1",
            "suggested_max_fee_in_gas_token": "0x7d0",
            "suggested_max_fee_in_strk": "0x1",
        },
        "parameters": {},
        "type": "invoke",
        "typed_data": standard(),
    }))
}

fn start() -> (SubmitRound<'static>, SubmitStep) {
    SubmitRound::start(session(), &reviewed(), Some(U256::from_u128(1000)), NOW).unwrap()
}

fn executes(step: &SubmitStep) -> bool {
    let SubmitStep::Post { executes, .. } = step else {
        panic!("not a post: {step:?}");
    };
    *executes
}

const SIGNATURE: [&str; 5] = ["0x1", "0x0", "0x3", "0x1", "0x2"];

fn signature() -> Vec<Felt> {
    SIGNATURE.iter().map(|felt| felt_of(felt)).collect()
}

#[test]
fn builds_signs_the_checked_hash_once_then_executes() {
    let (mut round, step) = start();
    assert!(!executes(&step));
    assert!(round.checked().is_none());
    let SubmitStep::Sign { hash } = round.accept(&built()).unwrap() else {
        panic!("no signature asked");
    };
    assert_eq!(Some(hash), round.checked().map(CheckedQuote::message_hash));
    // Nothing is judged while a signature is owed.
    let mut early = start().0;
    assert_eq!(
        early.signed(&signature(), NOW),
        Err(GaslessCode::Unavailable.into())
    );
    let step = round.signed(&signature(), NOW).unwrap();
    assert!(executes(&step));
    assert_eq!(
        round.signed(&signature(), NOW),
        Err(GaslessCode::Unavailable.into()),
        "a second signature is out of turn and ends the submission"
    );
}

fn executing() -> SubmitRound<'static> {
    let (mut round, _) = start();
    round.accept(&built()).unwrap();
    round.signed(&signature(), NOW).unwrap();
    round
}

#[test]
fn the_execute_answers_the_signed_fee_and_only_once() {
    let mut round = executing();
    let step = round
        .accept(&result(json!({"transaction_hash": "0x7a1ce5"})))
        .unwrap();
    assert_eq!(
        step,
        SubmitStep::Submitted(GaslessSubmission {
            transaction_hash: felt_of("0x7a1ce5"),
            user_fee: UserFee {
                amount: "1000".into(),
                token: "USDC".into(),
                token_address: USDC.into()
            }
        })
    );
    assert_eq!(
        round.accept(&result(json!({"transaction_hash": "0x7a1ce5"}))),
        Err(GaslessCode::Unavailable.into())
    );
}

#[test]
fn an_execute_without_a_usable_hash_is_lost() {
    for answer in [json!({"transaction_hash": "0x0"}), json!(null), json!({})] {
        let mut round = executing();
        assert_eq!(round.accept(&result(answer)), Err(GaslessError::Lost));
    }
    let mut round = executing();
    assert_eq!(
        round.accept(&reply(502, json!(""))),
        Err(GaslessError::Lost)
    );
}

#[test]
fn screening_repeats_keep_each_exchange_and_mark_an_execute_repeat() {
    let unavailable = reply(503, json!({"error": "screening_unavailable"}));
    let (mut round, _) = start();
    let step = round.accept(&unavailable).unwrap();
    assert!(matches!(
        step,
        SubmitStep::Post {
            delay_ms: 250,
            executes: false,
            ..
        }
    ));
    assert!(matches!(
        round.accept(&built()),
        Ok(SubmitStep::Sign { .. })
    ));
    let mut round = executing();
    let step = round.accept(&unavailable).unwrap();
    assert!(matches!(
        step,
        SubmitStep::Post {
            delay_ms: 250,
            executes: true,
            ..
        }
    ));
}

#[test]
fn a_refused_build_ends_the_submission_before_any_signature() {
    let (mut round, _) = start();
    let refused = reply(403, json!({"error": "screening_denied"}));
    assert_eq!(
        round.accept(&refused),
        Err(GaslessCode::ScreeningDenied.into())
    );
    assert!(round.checked().is_none());
    assert_eq!(round.accept(&built()), Err(GaslessCode::Unavailable.into()));
}

#[test]
fn no_reference_refuses_before_anything_is_posted() {
    let refused = SubmitRound::start(session(), &reviewed(), None, NOW).err();
    assert_eq!(refused, Some(GaslessCode::Unverifiable.into()));
}
