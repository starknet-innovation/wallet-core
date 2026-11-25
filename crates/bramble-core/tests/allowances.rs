//! The allowance manager through mobile's exports, end to end against a canned node: the revoke
//! (the one export that builds a call batch) from its id to its plan, the change its pending
//! record files in the allowance log before the activity row, the confirmation read an untrusted
//! contract needs, and the listing that settles a revoke on a reading or its own outcome. The
//! mechanism is wallet-core's `allowances` (its unit tests carry the extension's cases).
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

mod send_support;

use std::path::Path;
use std::sync::Arc;

use bramble_core::{
    AllowanceNotice, AllowanceSource, CallKind, FeeAffordability, NetworkId, RevokeError,
    RiskFlagCode, RpcReply, RpcRequest, ScanStep, SendStep, StepReply, TokenPreferences,
    TransferPlan, TxStatus, WalletCore,
};
use send_support::*;
use serde_json::{json, Value};

/// STRK as the log and the ids spell it (`canonicalFelt`: unpadded).
const STRK_ID: &str = "0x4718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d";
const SPENDER: &str = "0x41fd22b238fa21cfcf5dd45a8548974d8263b3a531a60388411c5e230f97023";
/// A contract the person does not track.
const UNKNOWN: &str = "0x6b1e8a14c1d7c0a1f4f4b0d4e1c5b2a3e1d3c4b5a69788796a5b4c3d2e1f001";

const FILE: &str = "allowances.v1.json";

fn account() -> String {
    format!("SN_SEPOLIA:{ACCOUNT_0}")
}

fn id(account: &str, token: &str) -> String {
    format!("{account}:{token}:{SPENDER}")
}

fn prefs() -> Arc<TokenPreferences> {
    TokenPreferences::new(None)
}

/// A log holding one grant of `amount` STRK units to `SPENDER` by each of `owners` (the test's
/// own data).
fn seed_all(dir: &Path, owners: &[&str], amount: &str) {
    let records: Vec<Value> = owners
        .iter()
        .map(|owner| {
            json!({"accountId": owner, "amount": amount, "id": id(owner, STRK_ID),
                "networkId": "SN_SEPOLIA", "spender": SPENDER, "token": STRK_ID})
        })
        .collect();
    std::fs::write(dir.join(FILE), Value::Array(records).to_string()).unwrap();
}

fn seed(dir: &Path, owner: &str, amount: &str) {
    seed_all(dir, &[owner], amount);
}

fn log(dir: &Path) -> Vec<Value> {
    let text = std::fs::read_to_string(dir.join(FILE)).unwrap_or_else(|_| "[]".into());
    serde_json::from_str::<Value>(&text)
        .unwrap()
        .as_array()
        .unwrap()
        .clone()
}

fn reply(request: &RpcRequest, answer: Value) -> RpcReply {
    let body: Value = serde_json::from_str(&request.body).unwrap();
    let mut reply = json!({"jsonrpc": "2.0", "id": request.id});
    for (key, value) in answer.as_object().unwrap() {
        reply[key] = value.clone();
    }
    assert_eq!(body["id"], json!(request.id));
    RpcReply {
        id: request.id,
        status: 200,
        body: Some(reply.to_string()),
    }
}

fn method(request: &RpcRequest) -> String {
    let body: Value = serde_json::from_str(&request.body).unwrap();
    body["method"].as_str().unwrap().to_string()
}

/// A node with no `Approval` event, at block 0, answering every `allowance` read with `amount`.
fn chain(request: &RpcRequest, amount: &str) -> Value {
    match method(request).as_str() {
        "starknet_getEvents" => json!({"result": {"events": []}}),
        "starknet_blockNumber" => json!({"result": 0}),
        "starknet_call" => json!({"result": [amount, "0x0"]}),
        other => panic!("unexpected {other}"),
    }
}

fn list(core: &WalletCore, allowance: &str) -> bramble_core::AllowanceList {
    let listing = core.allowance_listing(account(), prefs()).unwrap();
    loop {
        let calls = listing.next_calls();
        let Some(request) = calls.first() else {
            return listing.finish().unwrap();
        };
        let answer = chain(request, allowance);
        assert_eq!(listing.accept(reply(request, answer)), ScanStep::Continue);
    }
}

/// The revoke's plan, run to its review.
fn reviewed_revoke(core: &Arc<WalletCore>, node: &mut Node, id: &str) -> Arc<TransferPlan> {
    let revoke = core
        .clone()
        .prepare_revoke(account(), id.into(), prefs())
        .unwrap();
    assert!(revoke.next_calls().is_empty(), "STRK is trusted: no read");
    let plan = revoke.plan(Some("100000000000000000000".into())).unwrap();
    let first = plan.advance(StepReply::Start).unwrap();
    assert_eq!(run_until(&plan, core, node, first, never), SendStep::Review);
    plan
}

/// Submits the revoke of the seeded grant and lets the chain settle it at `status`.
fn submit_revoke(core: &Arc<WalletCore>, dir: &Path, status: Value) -> String {
    let mut node = sepolia(true);
    node.status = status;
    let plan = reviewed_revoke(core, &mut node, &id(&account(), STRK_ID));
    let step = plan.advance(StepReply::Confirm).unwrap();
    let SendStep::Broadcast { hash } = run_until(&plan, core, &mut node, step, never) else {
        panic!("no broadcast");
    };
    // Filed under the signed hash, marked until the chain confirms it.
    let filed = log(dir);
    assert_eq!(filed.len(), 1);
    assert_eq!(
        (
            &filed[0]["revoking"],
            &filed[0]["revokeHash"],
            &filed[0]["amount"]
        ),
        (&json!(true), &json!(hash), &json!("1000"))
    );
    let step = plan.advance(StepReply::Proceed).unwrap();
    assert!(matches!(
        run_until(&plan, core, &mut node, step, never),
        SendStep::Finished { .. }
    ));
    hash
}

#[test]
fn the_revoke_is_reviewed_as_one_approve_and_its_fee_against_the_balance() {
    let (dir, core) = wallet();
    seed(dir.path(), &account(), "1000");
    let mut node = sepolia(true);
    let plan = reviewed_revoke(&core, &mut node, &id(&account(), STRK_ID));
    let review = plan.current_review().unwrap();
    assert!(review.can_confirm);
    assert_eq!(review.affordability, FeeAffordability::Sufficient);
    let calls = plan.reviewed_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        (
            calls[0].kind,
            calls[0].contract_address.as_str(),
            calls[0].entrypoint.as_str()
        ),
        (CallKind::Approve, STRK_ID, "approve")
    );
    assert_eq!(review.summary, calls[0].summary);
    assert!(plan.risk_flags().is_empty(), "{:?}", plan.risk_flags());
}

#[test]
fn a_submitted_revoke_settles_on_a_zero_reading() {
    let (dir, core) = wallet();
    seed(dir.path(), &account(), "1000");
    let hash = submit_revoke(
        &core,
        dir.path(),
        json!({"execution_status": "SUCCEEDED",
        "finality_status": "ACCEPTED_ON_L2"}),
    );
    let row = &core.activity(account()).unwrap()[0];
    assert_eq!(
        (row.hash.as_str(), row.status),
        (hash.as_str(), TxStatus::Accepted)
    );
    let listed = list(&core, "0x0");
    assert!(listed.allowances.is_empty(), "{listed:?}");
    assert!(listed.notices.is_empty());
    assert!(
        log(dir.path()).is_empty(),
        "the zero reading dropped the record"
    );
}

// A reverted revoke clears its mark on its own outcome, read from the wallet's activity row,
// though the chain still reports the grant.
#[test]
fn a_reverted_revoke_clears_its_mark_on_its_own_outcome() {
    let (dir, core) = wallet();
    seed(dir.path(), &account(), "1000");
    submit_revoke(
        &core,
        dir.path(),
        json!({"execution_status": "REVERTED",
        "finality_status": "ACCEPTED_ON_L2"}),
    );
    let listed = list(&core, "0x3e8");
    assert_eq!(listed.allowances.len(), 1);
    let row = &listed.allowances[0];
    assert_eq!(
        (
            row.source,
            row.revoking,
            row.amount.as_str(),
            row.unconfirmed_token
        ),
        (AllowanceSource::Wallet, false, "1000", false)
    );
    assert_eq!(log(dir.path())[0].get("revoking"), None);
}

// The log is what keeps a residual grant visible and revokable, so it is filed before the row
// and never depends on the row's write: here the activity file cannot be written at all, and
// the revoke is in the log under the hash before the write is handed out.
/// Signs the revoke of the seeded grant and runs it to its broadcast, checking it is in the log
/// before the write leaves.
fn broadcast_revoke(core: &Arc<WalletCore>, dir: &Path) {
    let mut node = sepolia(true);
    let plan = reviewed_revoke(core, &mut node, &id(&account(), STRK_ID));
    let mut step = plan.advance(StepReply::Confirm).unwrap();
    while let SendStep::Rpc { .. } = &step {
        let (id, method, params, is_write) = rpc_parts(&step);
        let signed = is_write.then(|| {
            let filed = log(dir);
            assert_eq!(
                filed[0]["revoking"],
                json!(true),
                "filed before the write left"
            );
            filed[0]["revokeHash"].as_str().unwrap().to_string()
        });
        let answer = node.reply(&method, &params, signed.as_deref());
        step = plan.advance(rpc_reply(id, &answer)).unwrap();
    }
    assert!(matches!(step, SendStep::Broadcast { .. }), "{step:?}");
}

#[test]
fn a_revoke_is_filed_before_its_row_and_without_it() {
    let (dir, core) = wallet();
    seed(dir.path(), &account(), "1000");
    std::fs::create_dir(dir.path().join("activity.v1.json.tmp")).unwrap();
    broadcast_revoke(&core, dir.path());
    assert!(
        core.activity(account()).unwrap().is_empty(),
        "the row could not be written"
    );
}

// Mobile files a revoke when it signs it, before the node accepts it. With no row there is no
// outcome to clear the mark, so a reading that still shows the grant settles it as failed and
// the grant is offered again.
#[test]
fn a_rowless_revoke_the_chain_still_reports_is_offered_again() {
    let (dir, core) = wallet();
    seed(dir.path(), &account(), "1000");
    std::fs::create_dir(dir.path().join("activity.v1.json.tmp")).unwrap();
    broadcast_revoke(&core, dir.path());
    assert!(core.activity(account()).unwrap().is_empty());
    let listed = list(&core, "0x3e8");
    assert_eq!(listed.allowances.len(), 1);
    assert_eq!(
        (
            listed.allowances[0].revoking,
            listed.allowances[0].amount.as_str()
        ),
        (false, "1000")
    );
    assert_eq!(log(dir.path())[0].get("revoking"), None);
}

// Without a reading nothing is known, so a rowless revoke keeps its mark.
#[test]
fn a_rowless_revoke_with_no_reading_keeps_its_mark() {
    let (dir, core) = wallet();
    seed(dir.path(), &account(), "1000");
    std::fs::create_dir(dir.path().join("activity.v1.json.tmp")).unwrap();
    broadcast_revoke(&core, dir.path());
    let listing = core.allowance_listing(account(), prefs()).unwrap();
    while let Some(request) = listing.next_calls().first() {
        if method(request) == "starknet_call" {
            listing.give_up();
        } else {
            listing.accept(reply(request, chain(request, "0x3e8")));
        }
    }
    let listed = listing.finish().unwrap();
    assert!(listed.allowances[0].revoking, "{listed:?}");
    assert_eq!(log(dir.path())[0]["revoking"], json!(true));
}

// A revoke whose row is still pending keeps its mark while the chain shows the grant: only its
// own outcome or a zero reading settles it.
#[test]
fn a_pending_revoke_keeps_its_mark_while_the_grant_stands() {
    let (dir, core) = wallet();
    seed(dir.path(), &account(), "1000");
    broadcast_revoke(&core, dir.path());
    assert_eq!(
        core.activity(account()).unwrap()[0].status,
        TxStatus::Pending
    );
    let listed = list(&core, "0x3e8");
    assert!(listed.allowances[0].revoking, "{listed:?}");
    assert_eq!(log(dir.path())[0]["revoking"], json!(true));
}

#[test]
fn an_untrusted_contract_is_revoked_only_once_the_chain_answers_allowance() {
    let (_dir, core) = wallet();
    let external = id(&account(), UNKNOWN);
    let prepare = || {
        core.clone()
            .prepare_revoke(account(), external.clone(), prefs())
            .unwrap()
    };
    // No answer at all (every endpoint failed), and a node that will not answer `allowance`.
    let unanswered = prepare();
    assert_eq!(unanswered.next_calls().len(), 1);
    assert_eq!(
        unanswered.plan(None).err(),
        Some(RevokeError::UnconfirmedToken)
    );
    let refused = prepare();
    let request = refused.next_calls().remove(0);
    assert_eq!(method(&request), "starknet_call");
    let error = json!({"error": {"code": 21, "message": "Invalid message selector"}});
    assert_eq!(refused.accept(reply(&request, error)), ScanStep::Continue);
    assert!(refused.next_calls().is_empty());
    assert_eq!(
        refused.plan(None).err(),
        Some(RevokeError::UnconfirmedToken)
    );
    // Answered: an ERC-20, whatever amount it reported; still an unknown contract on the card.
    let answered = prepare();
    let request = answered.next_calls().remove(0);
    answered.accept(reply(&request, json!({"result": ["0x0", "0x0"]})));
    let plan = answered.plan(None).unwrap();
    let mut node = sepolia(true);
    let first = plan.advance(StepReply::Start).unwrap();
    assert_eq!(
        run_until(&plan, &core, &mut node, first, never),
        SendStep::Review
    );
    let flags = plan.risk_flags();
    assert_eq!(flags.len(), 1);
    assert_eq!(flags[0].code, RiskFlagCode::UnknownContract);
    assert!(flags[0]
        .message
        .contains("is not a token or contact this wallet knows"));
    assert_eq!(
        plan.current_review().unwrap().affordability,
        FeeAffordability::Unknown
    );
}

#[test]
fn a_revoke_names_only_this_accounts_grants() {
    let (dir, core) = wallet();
    let other = format!("SN_SEPOLIA:{ACCOUNT_1}");
    seed(dir.path(), &other, "1000");
    let revoke = |id: String| core.clone().prepare_revoke(account(), id, prefs()).err();
    assert_eq!(revoke(id(&other, STRK_ID)), Some(RevokeError::OtherAccount));
    // Another account's pair spelled as an id never names this account's grant.
    assert_eq!(revoke(id(&other, UNKNOWN)), Some(RevokeError::NotFound));
    let mainnet = format!("SN_MAIN:{ACCOUNT_0}");
    assert_eq!(revoke(id(&mainnet, STRK_ID)), Some(RevokeError::NotFound));
    assert_eq!(revoke(String::new()), Some(RevokeError::InvalidRequest));
    assert_eq!(revoke("x".repeat(241)), Some(RevokeError::InvalidRequest));
}

// A plan holds the session it was prepared in: a lock and a fresh unlock void the revoke.
#[test]
fn a_lock_voids_a_prepared_revoke() {
    let (dir, core) = wallet();
    seed(dir.path(), &account(), "1000");
    let revoke = core
        .clone()
        .prepare_revoke(account(), id(&account(), STRK_ID), prefs())
        .unwrap();
    core.lock();
    core.unlock(PASSWORD.into()).unwrap();
    let plan = revoke.plan(None).unwrap();
    let mut node = sepolia(true);
    let first = plan.advance(StepReply::Start).unwrap();
    assert_eq!(
        run_until(&plan, &core, &mut node, first, never),
        SendStep::Review
    );
    let step = plan.advance(StepReply::Confirm).unwrap();
    let failure = failure(run_until(&plan, &core, &mut node, step, never));
    assert_eq!(failure.code, bramble_core::SendFailureCode::Locked);
    assert_eq!(plan.signatures_made(), 0);
    assert_eq!(log(dir.path())[0].get("revoking"), None);
}

#[test]
fn a_listing_without_an_endpoint_says_its_scan_is_incomplete() {
    let (dir, core) = wallet();
    // Another account's grant is neither listed nor read.
    seed_all(
        dir.path(),
        &[&account(), &format!("SN_SEPOLIA:{ACCOUNT_1}")],
        "1000",
    );
    let listing = core.allowance_listing(account(), prefs()).unwrap();
    assert_eq!(method(&listing.next_calls()[0]), "starknet_getEvents");
    assert!(listing.finish().is_err(), "not before its reads");
    listing.give_up();
    // An early finish issues nothing, so the tracked read is still handed out.
    assert!(listing.finish().is_err(), "not before its tracked read");
    // One tracked read, this account's grant; the stored amount stands with no answer.
    let read = listing.next_calls().remove(0);
    assert_eq!(method(&read), "starknet_call");
    assert!(read.body.contains(ACCOUNT_0) && !read.body.contains(ACCOUNT_1));
    listing.give_up();
    assert!(listing.next_calls().is_empty());
    let listed = listing.finish().unwrap();
    assert_eq!(listed.notices, [AllowanceNotice::ScanIncomplete]);
    assert_eq!(listed.allowances.len(), 1);
    let row = &listed.allowances[0];
    assert_eq!(
        (row.amount.as_str(), row.source, row.account_id.as_str()),
        ("1000", AllowanceSource::Wallet, account().as_str())
    );
    assert_eq!(row.network, NetworkId::Sepolia);
}

// Bramble #1038: a grant to a lookalike of the account is refused, but revoking one is exempt, or
// a poisoned grant could never be taken back. The revoke reaches its review.
#[test]
fn a_grant_to_a_lookalike_spender_can_still_be_revoked() {
    const TWIN: &str = "0x2083610000000000000000000000000000000000000000000000000000098d9";
    let (dir, core) = wallet();
    let id = format!("{}:{STRK_ID}:{TWIN}", account());
    let record = json!([{"accountId": account(), "amount": "1000", "id": id,
        "networkId": "SN_SEPOLIA", "spender": TWIN, "token": STRK_ID}]);
    std::fs::write(dir.path().join(FILE), record.to_string()).unwrap();
    // `reviewed_revoke` asserts the plan reaches `SendStep::Review`.
    reviewed_revoke(&core, &mut sepolia(true), &id);
}
