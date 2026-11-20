//! The receipt fee a settled send records (P2), against a canned node through `WalletCore`.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

mod send_support;

use bramble_core::{SendStep, StepReply, TxStatus};
use send_support::*;
use serde_json::json;

/// The receipt's fee is read once, after the status that settled the send (`tx-reconcile.ts`),
/// and only for this wallet's own record; a refresh of a row that has its fee reads no receipt.
#[test]
fn a_settled_send_records_its_receipt_fee_once() {
    let (_dir, core) = wallet();
    let mut node = sepolia(true);
    let plan = reviewed(&core, &mut node, ACCOUNT_1);
    let step = plan.advance(StepReply::Confirm).unwrap();
    let broadcast = run_until(&plan, &core, &mut node, step, never);
    assert!(matches!(broadcast, SendStep::Broadcast { .. }));
    let mut receipts = 0;
    let first = plan.advance(StepReply::Proceed).unwrap();
    let done = run_until(&plan, &core, &mut node, first, |method, _| {
        receipts += usize::from(method == "starknet_getTransactionReceipt");
        false
    });
    assert!(matches!(
        done,
        SendStep::Finished {
            status: TxStatus::Accepted,
            ..
        }
    ));
    assert_eq!(receipts, 1);
    let account = oz_id(&core, bramble_core::NetworkId::Sepolia, 0);
    let record = core.activity(account.clone()).unwrap().remove(0);
    assert_eq!(record.origin, Some(bramble_core::ActivityOrigin::Wallet));
    let fee = record.fee_paid.clone().unwrap();
    assert_eq!(
        (fee.amount.as_str(), fee.unit),
        ("0x2386f26fc10000", bramble_core::FeeUnit::Fri)
    );
    assert_eq!(
        bramble_core::activity_fee_text(record.clone()).as_deref(),
        Some("0.01 STRK")
    );
    // Sepolia has no fiat, so no price was captured, and none is shown.
    assert_eq!(record.submitted_price, None);

    let refresh = core.clone().refresh_activity(account, record.hash).unwrap();
    let (id, method, ..) = rpc_parts(&refresh.advance(StepReply::Start).unwrap());
    assert_eq!(method, "starknet_getTransactionStatus");
    let accepted =
        json!({"result": {"finality_status": "ACCEPTED_ON_L2", "execution_status": "SUCCEEDED"}});
    let step = refresh.advance(rpc_reply(id, &accepted)).unwrap();
    assert!(matches!(
        step,
        SendStep::Finished {
            status: TxStatus::Accepted,
            ..
        }
    ));
}
