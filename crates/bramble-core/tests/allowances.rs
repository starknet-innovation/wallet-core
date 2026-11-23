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
    CallKind, FeeAffordability, RevokeError, RiskFlagCode, RpcReply, RpcRequest, ScanStep,
    SendStep, StepReply, TokenPreferences, TransferPlan, WalletCore,
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

fn seed(dir: &Path, owner: &str, amount: &str) {
    let record = json!([{"accountId": owner, "amount": amount, "id": id(owner, STRK_ID),
        "networkId": "SN_SEPOLIA", "spender": SPENDER, "token": STRK_ID}]);
    std::fs::write(dir.join(FILE), record.to_string()).unwrap();
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

// The log is what keeps a residual grant visible and revokable, so it is filed before the row
// and never depends on the row's write: here the activity file cannot be written at all, and
// the revoke is in the log under the hash before the write is handed out.
#[test]
fn a_revoke_is_filed_before_its_row_and_without_it() {
    let (dir, core) = wallet();
    seed(dir.path(), &account(), "1000");
    std::fs::create_dir(dir.path().join("activity.v1.json.tmp")).unwrap();
    let mut node = sepolia(true);
    let plan = reviewed_revoke(&core, &mut node, &id(&account(), STRK_ID));
    let mut step = plan.advance(StepReply::Confirm).unwrap();
    while let SendStep::Rpc { .. } = &step {
        let (id, method, params, is_write) = rpc_parts(&step);
        let signed = is_write.then(|| {
            let filed = log(dir.path());
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
    assert!(
        core.activity(account()).unwrap().is_empty(),
        "the row could not be written"
    );
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
