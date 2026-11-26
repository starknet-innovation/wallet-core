//! `DappSession`'s `wallet_addInvokeTransaction` (`src/dapp_invoke.rs`) end to end against a
//! canned node, after the extension's `tests/background/provider/provider-invoke.test.ts` and
//! `approvals/approval-invoke-execution.test.ts`: the gates, the card's review, the plan the
//! approval hands the host, and the page's answer at each way the plan can end.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

use crate::send_support;

use std::sync::Arc;

use bramble_core::{
    DappCard, DappInvokeStart, DappReply, DappSession, DappSettled, NetworkId, SendStep, StepReply,
    TokenPreferences, TransferPlan, WalletCore,
};
use send_support::*;

const SITE: &str = "https://app.example";
const NOW: f64 = 1_800_000_000_000.0;
const RECIPIENT: &str = "0x41fd22b238fa21cfcf5dd45a8548974d8263b3a531a60388411c5e230f97023";

fn ask(dapp: &DappSession, method: &str, params: Option<&str>, request_id: u64) -> DappReply {
    dapp.request(
        SITE.into(),
        method.into(),
        params.map(str::to_owned),
        NetworkId::Sepolia,
        request_id,
    )
    .unwrap()
}

fn code(reply: &DappReply) -> i64 {
    match reply {
        DappReply::Error { code, .. } => *code,
        other => panic!("not an error: {other:?}"),
    }
}

fn pending(reply: DappReply) -> String {
    match reply {
        DappReply::Pending { approval_id } => approval_id,
        other => panic!("not pending: {other:?}"),
    }
}

/// One STRK transfer of one base unit to `to`.
fn transfer(to: &str) -> String {
    format!(
        r#"{{"calls":[{{"contractAddress":"{STRK}","entrypoint":"transfer","calldata":["{to}","0x1","0x0"]}}]}}"#
    )
}

fn connected() -> (tempfile::TempDir, Arc<WalletCore>, Arc<DappSession>) {
    let (dir, core) = wallet();
    let dapp = DappSession::new(core.clone(), TokenPreferences::new(None));
    let card = pending(ask(&dapp, "wallet_requestAccounts", None, 100));
    let id = core.active_account().unwrap().unwrap().id;
    dapp.approve_connect(card, id, NOW);
    (dir, core, dapp)
}

fn plan(start: DappInvokeStart) -> Arc<TransferPlan> {
    match start {
        DappInvokeStart::Plan { plan } => plan,
        DappInvokeStart::Settled { settled } => panic!("refused: {settled:?}"),
    }
}

#[test]
fn an_approved_batch_answers_the_hash_the_node_accepted() {
    let (_dir, core, dapp) = connected();
    let card = pending(ask(
        &dapp,
        "wallet_addInvokeTransaction",
        Some(&transfer(RECIPIENT)),
        1,
    ));
    let listed = dapp.pending_approvals(NOW);
    let DappCard::Invoke {
        call_summaries,
        network,
        ..
    } = &listed[0].card
    else {
        panic!("not an invoke card: {:?}", listed[0].card);
    };
    assert_eq!(*network, NetworkId::Sepolia);
    assert_eq!(call_summaries.len(), 1);

    let plan = plan(dapp.approve_invoke(card.clone(), Some("10000000000000000000".into()), NOW));
    assert!(dapp.pending_approvals(NOW).is_empty());
    // Approving the card is not consent to sign: the plan's own review is.
    let mut node = sepolia(true);
    let first = plan.advance(StepReply::Start).unwrap();
    assert_eq!(
        run_until(&plan, &core, &mut node, first, never),
        SendStep::Review
    );
    assert!(dapp.settle_invoke(card.clone()).is_empty());
    assert_eq!(plan.signatures_made(), 0);

    let step = plan.advance(StepReply::Confirm).unwrap();
    let SendStep::Broadcast { hash } = run_until(&plan, &core, &mut node, step, never) else {
        panic!("no broadcast");
    };
    let padded = format!("0x{:0>64}", hash.trim_start_matches("0x"));
    assert_eq!(
        dapp.settle_invoke(card.clone()),
        vec![DappSettled {
            request_id: 1,
            reply: DappReply::Result {
                json: format!(r#"{{"transaction_hash":"{padded}"}}"#),
            },
        }]
    );
    assert!(dapp.settle_invoke(card).is_empty());
}

#[test]
fn a_review_closed_before_signing_is_the_persons_refusal() {
    let (_dir, core, dapp) = connected();
    let card = pending(ask(
        &dapp,
        "wallet_addInvokeTransaction",
        Some(&transfer(RECIPIENT)),
        1,
    ));
    let plan = plan(dapp.approve_invoke(card.clone(), None, NOW));
    let mut node = sepolia(true);
    let first = plan.advance(StepReply::Start).unwrap();
    run_until(&plan, &core, &mut node, first, never);
    plan.close();
    assert_eq!(code(&dapp.settle_invoke(card)[0].reply), 113);
}

#[test]
fn the_gates_refuse_before_any_card() {
    let (_dir, core, dapp) = connected();
    let address = core.active_account().unwrap().unwrap().address;
    // The account contract itself.
    assert_eq!(
        code(&ask(
            &dapp,
            "wallet_addInvokeTransaction",
            Some(&transfer_on(&address)),
            1
        )),
        163
    );
    // Not connected: another site.
    let other = dapp
        .request(
            "https://other.example".into(),
            "wallet_addInvokeTransaction".into(),
            Some(transfer(RECIPIENT)),
            NetworkId::Sepolia,
            2,
        )
        .unwrap();
    assert_eq!(code(&other), 163);
    // Locked.
    core.lock();
    assert_eq!(
        code(&ask(
            &dapp,
            "wallet_addInvokeTransaction",
            Some(&transfer(RECIPIENT)),
            3
        )),
        163
    );
    assert!(dapp.pending_approvals(NOW).is_empty());
}

/// A call to `contract` itself.
fn transfer_on(contract: &str) -> String {
    format!(
        r#"{{"calls":[{{"contractAddress":"{contract}","entrypoint":"transfer","calldata":["{RECIPIENT}","0x1","0x0"]}}]}}"#
    )
}

#[test]
fn a_site_whose_account_moved_is_asked_to_reconnect_and_its_card_is_refused() {
    let (_dir, core, dapp) = connected();
    let card = pending(ask(
        &dapp,
        "wallet_addInvokeTransaction",
        Some(&transfer(RECIPIENT)),
        1,
    ));
    let first = core.active_account().unwrap().unwrap().id;
    let second = core
        .account_records(NetworkId::Sepolia)
        .unwrap()
        .into_iter()
        .find(|record| record.id != first)
        .unwrap()
        .id;
    core.select_account(second).unwrap();
    // Approving for an account no longer active: refused, nothing to sign.
    let DappInvokeStart::Settled { settled } = dapp.approve_invoke(card, None, NOW) else {
        panic!("a plan for a moved account");
    };
    assert_eq!(code(&settled[0].reply), 163);
    // Asked again, the site is refused and shown a connect card.
    assert_eq!(
        code(&ask(
            &dapp,
            "wallet_addInvokeTransaction",
            Some(&transfer(RECIPIENT)),
            2
        )),
        163
    );
    assert!(matches!(
        dapp.pending_approvals(NOW)[0].card,
        DappCard::Connect
    ));
}

#[test]
fn a_page_that_went_closes_its_plan() {
    let (_dir, core, dapp) = connected();
    let card = pending(ask(
        &dapp,
        "wallet_addInvokeTransaction",
        Some(&transfer(RECIPIENT)),
        1,
    ));
    let plan = plan(dapp.approve_invoke(card.clone(), None, NOW));
    let mut node = sepolia(true);
    let first = plan.advance(StepReply::Start).unwrap();
    assert_eq!(
        run_until(&plan, &core, &mut node, first, never),
        SendStep::Review
    );
    dapp.withdraw(vec![1]);
    // The person's confirm can no longer reach a signature.
    let confirmed = plan.advance(StepReply::Confirm);
    assert!(
        !matches!(confirmed, Ok(SendStep::Rpc { .. })),
        "{confirmed:?}"
    );
    assert_eq!(plan.signatures_made(), 0);
    assert!(dapp.settle_invoke(card).is_empty());
}

/// `address` with its visible `0x` + four and last four kept (`addressLabel`), the rest changed.
fn lookalike(address: &str) -> String {
    let padded = format!("{:0>64}", address.trim_start_matches("0x"));
    format!("0x{}{}{}", &padded[..4], "1".repeat(56), &padded[60..])
}

// "flags a transfer to a lookalike on the card and refuses it at execution" (Sherlock 140): the
// site learns nothing at request, and nothing reaches a signature.
#[test]
fn a_batch_paying_a_lookalike_is_flagged_then_refused_at_approval() {
    let (_dir, core, dapp) = connected();
    let own = core.active_account().unwrap().unwrap().address;
    let card = pending(ask(
        &dapp,
        "wallet_addInvokeTransaction",
        Some(&transfer(&lookalike(&own))),
        1,
    ));
    let DappCard::Invoke { flags, .. } = &dapp.pending_approvals(NOW)[0].card else {
        panic!("not an invoke card");
    };
    let codes: Vec<_> = flags.iter().map(|flag| flag.code).collect();
    assert!(
        codes.contains(&bramble_core::RiskFlagCode::AddressPoisoning),
        "{codes:?}"
    );
    let DappInvokeStart::Settled { settled } = dapp.approve_invoke(card, None, NOW) else {
        panic!("a plan paying a lookalike");
    };
    assert_eq!(code(&settled[0].reply), 163);
}
