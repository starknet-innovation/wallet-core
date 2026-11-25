//! The gasless export's journal (`recordRequiredRelaySignature`) and its execute repeats: the row
//! filed before the execute is posted, what a lost answer leaves, a row that cannot be stored,
//! and the pause before a repeat.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

mod gasless_export_support;
mod send_support;

use bramble_core::{
    activity_needs_observation, GaslessFeeError, GaslessSubmitStep, HttpOutcome, NetworkId,
    PriceView, SendStep, StepReply,
};
use gasless_export_support::*;
use serde_json::json;
use starknet_wallet_core::{felt, kms};

fn lost() -> HttpOutcome {
    HttpOutcome {
        status: 502,
        retry_after: None,
        body: None,
        timed_out: false,
    }
}

// `recordRequiredRelaySignature`: the row is filed before the execute is posted, so a lost
// answer leaves it pending, unobserved, and guarding a second signature until its deadline.
#[test]
fn a_lost_execute_leaves_a_pending_journal_row_that_guards_a_second_signature() {
    let (_dir, core) = wallet();
    let plan = transfer(&core, ACCOUNT_1, "0");
    let (fee, offer) = offered(&plan);
    post(fee.choose(offer, Some(prices()), NOW).unwrap());
    let typed = typed_data(AMOUNT, "0x3e8");
    post(fee.accept_submit_reply(built(&typed), NOW).unwrap());
    let rows = core.activity(account_id(&core)).unwrap();
    assert_eq!(rows.len(), 1, "filed before the execute was handed out");
    assert!(matches!(
        fee.accept_submit_reply(lost(), NOW),
        Err(GaslessFeeError::Lost { .. })
    ));
    let rows = core.activity(account_id(&core)).unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    let hash = kms::typed_data_message_hash(&typed.to_string(), &address(&core)).unwrap();
    assert_eq!(row.hash, felt::canonical(&hash));
    assert_eq!(row.status, bramble_core::TxStatus::Pending);
    assert_eq!(
        row.user_fee.as_ref().map(|fee| fee.amount.as_str()),
        Some("1000")
    );
    let relay = row.relay.as_ref().unwrap();
    assert_eq!(
        (relay.nonce.as_str(), relay.execute_before),
        ("0x5", NOW_S + 600)
    );
    assert!(!activity_needs_observation(row.clone(), NOW));
    // Re-preparing the same send offers no second gasless signature until the first lapses.
    let again = transfer(&core, ACCOUNT_1, "0");
    assert!(again
        .clone()
        .gasless_offers(Some(prices()), balances(), NOW)
        .is_none());
    let lapsed = (NOW_S + 600) * 1000;
    let fresh = PriceView {
        as_of_ms: Some(lapsed as f64),
        ..prices()
    };
    assert!(again
        .gasless_offers(Some(fresh), balances(), lapsed)
        .is_some());
}

// INVOKE_NOT_JOURNALED: a row that cannot be stored means no execute is handed out.
#[cfg(unix)]
#[test]
fn an_execute_that_cannot_be_journaled_is_never_posted() {
    use std::os::unix::fs::PermissionsExt;
    let (dir, core) = wallet();
    let plan = transfer(&core, ACCOUNT_1, "0");
    let (fee, offer) = offered(&plan);
    post(fee.choose(offer, Some(prices()), NOW).unwrap());
    let read_only = std::fs::Permissions::from_mode(0o555);
    std::fs::set_permissions(dir.path(), read_only).unwrap();
    let typed = typed_data(AMOUNT, "0x3e8");
    let refused = fee.accept_submit_reply(built(&typed), NOW);
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(
        refused,
        Err(GaslessFeeError::Refused {
            reason: "The transaction could not be recorded before sending. Nothing was submitted — try again.".into(),
            retryable: false
        })
    );
    assert!(core.activity(account_id(&core)).unwrap().is_empty());
    assert_eq!(
        fee.accept_submit_reply(lost(), NOW),
        Err(GaslessFeeError::NotFound)
    );
}

// The offers stand for the review they were made for: a re-estimate replaces it.
#[test]
fn offers_from_an_earlier_review_are_not_chosen_after_a_re_estimate() {
    let (_dir, core) = wallet();
    let mut node = Node::new(NetworkId::Mainnet, true);
    let plan = transfer_on(&core, ACCOUNT_1, "0", &mut node);
    let (fee, offer) = offered(&plan);
    node.invoke_estimate["l2_gas_consumed"] = json!("0x2468a0");
    let first = plan.advance(StepReply::RetryEstimate).unwrap();
    assert_eq!(
        run_until(&plan, &core, &mut node, first, never),
        SendStep::Review
    );
    assert_eq!(
        fee.choose(offer, Some(prices()), NOW),
        Err(GaslessFeeError::NotFound)
    );
    // The plan was left as it was: the new review is still the person's to confirm.
    assert!(plan.current_review().is_some());
    assert!(plan
        .clone()
        .gasless_offers(Some(prices()), balances(), NOW)
        .is_some());
}

// `postGasless`: an execute whose screening was unavailable is posted again after a pause, and
// only if the wallet is still unlocked when the pause ends.
#[test]
fn an_execute_repeat_waits_and_is_refused_if_the_wallet_locked_meanwhile() {
    let unavailable = || HttpOutcome {
        status: 503,
        retry_after: None,
        body: Some(br#"{"error":"screening_unavailable"}"#.to_vec()),
        timed_out: false,
    };
    for lock in [false, true] {
        let (_dir, core) = wallet();
        let plan = transfer(&core, ACCOUNT_1, "0");
        let (fee, offer) = offered(&plan);
        post(fee.choose(offer, Some(prices()), NOW).unwrap());
        assert_eq!(fee.resume(), Err(GaslessFeeError::NotFound));
        let typed = typed_data(AMOUNT, "0x3e8");
        let execute = post(fee.accept_submit_reply(built(&typed), NOW).unwrap());
        assert_eq!(
            fee.accept_submit_reply(unavailable(), NOW),
            Ok(GaslessSubmitStep::Wait { delay_ms: 250 })
        );
        if lock {
            core.lock();
            assert_eq!(fee.resume(), Err(GaslessFeeError::Locked));
            assert_eq!(fee.resume(), Err(GaslessFeeError::NotFound));
        } else {
            assert_eq!(post(fee.resume().unwrap()), execute, "the same signed body");
        }
    }
}
