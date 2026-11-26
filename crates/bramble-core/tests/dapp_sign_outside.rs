//! `DappSession`'s SNIP-9 outside executions over a real vault (`src/dapp_sign.rs`): reviewed
//! as an invoke's calls are, flagged on the card, refused at approval when a payee mimics an
//! address the person knows. After the extension's
//! `tests/background/approvals/outside-execution-sign.test.ts`.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

use crate::common;

use bramble_core::{DappCard, DappSession, RiskFlagCode};
use common::account_id;
use common::dapp::*;
use common::imported;

const STRK: &str = "0x04718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d";
const MAX_U128: &str = "0xffffffffffffffffffffffffffffffff";

/// starknet.js's SNIP-9 v2 `getTypedData` on Sepolia, with `calls` as `(to, entrypoint,
/// calldata)`.
fn outside_execution(calls: &[(&str, &str, Vec<String>)]) -> String {
    let calls: Vec<serde_json::Value> = calls
        .iter()
        .map(|(to, entrypoint, calldata)| {
            serde_json::json!({ "To": to, "Selector": entrypoint, "Calldata": calldata })
        })
        .collect();
    serde_json::json!({
        "types": {
            "StarknetDomain": [
                { "name": "name", "type": "shortstring" },
                { "name": "version", "type": "shortstring" },
                { "name": "chainId", "type": "shortstring" },
                { "name": "revision", "type": "shortstring" },
            ],
            "OutsideExecution": [
                { "name": "Caller", "type": "ContractAddress" },
                { "name": "Nonce", "type": "felt" },
                { "name": "Execute After", "type": "u128" },
                { "name": "Execute Before", "type": "u128" },
                { "name": "Calls", "type": "Call*" },
            ],
            "Call": [
                { "name": "To", "type": "ContractAddress" },
                { "name": "Selector", "type": "selector" },
                { "name": "Calldata", "type": "felt*" },
            ],
        },
        "primaryType": "OutsideExecution",
        "domain": {
            "name": "Account.execute_from_outside",
            "version": "2",
            "chainId": "SN_SEPOLIA",
            "revision": "1",
        },
        "message": {
            "Caller": "0x414e595f43414c4c4552",
            "Nonce": "0x1",
            "Execute After": "0x1",
            "Execute Before": "0xe8d4a50fff",
            "Calls": calls,
        },
    })
    .to_string()
}

fn sign_card(dapp: &DappSession) -> (Vec<String>, Vec<RiskFlagCode>) {
    match dapp.pending_approvals(NOW)[0].card.clone() {
        DappCard::SignTypedData {
            outside_execution,
            call_summaries,
            flags,
            ..
        } => (
            {
                // Flagged by the primary type alone, calls or none.
                assert!(outside_execution);
                call_summaries
            },
            flags.into_iter().map(|flag| flag.code).collect(),
        ),
        other => panic!("not a sign card: {other:?}"),
    }
}

/// `address` with its visible `0x` + four and last four kept (`addressLabel`), the rest changed.
fn lookalike(address: &str) -> String {
    let padded = format!("{:0>64}", address.trim_start_matches("0x"));
    format!("0x{}{}{}", &padded[..4], "1".repeat(56), &padded[60..])
}

// `outside-execution-sign.test.ts` over a real vault: "queues an unlimited approve with its flag
// and decoded summary", then signs on approval like any typed data.
#[test]
fn an_outside_execution_is_reviewed_as_an_invoke_and_signs_on_approval() {
    let (_dir, _clock, core) = imported();
    let dapp = connected(&core);
    let spender = "0x0123000000000000000000000000000000000000000000000000000000000456";
    let document = outside_execution(&[(
        STRK,
        "approve",
        vec![spender.into(), MAX_U128.into(), MAX_U128.into()],
    )]);
    let card = pending_id(sign_request(&dapp, SITE, &document, 1));
    let (summaries, flags) = sign_card(&dapp);
    assert_eq!(summaries.len(), 1, "{summaries:?}");
    assert!(
        flags.contains(&RiskFlagCode::UnlimitedApproval),
        "{flags:?}"
    );
    let reply = approved(&dapp, card, account_id(&core));
    let signature = signature(&reply);
    assert!(verifies(&core, &document, &signature[0], &signature[1]));
}

// An outside execution with no calls is still one: the card carries the flag (the extension's
// `signing.outsideExecution` warning) with nothing to summarise.
#[test]
fn an_outside_execution_with_no_calls_is_still_flagged() {
    let (_dir, _clock, core) = imported();
    let dapp = connected(&core);
    pending_id(sign_request(&dapp, SITE, &outside_execution(&[]), 1));
    let (summaries, flags) = sign_card(&dapp);
    assert!(
        summaries.is_empty() && flags.is_empty(),
        "{summaries:?} {flags:?}"
    );
}

// "refuses a call to the account contract itself".
#[test]
fn an_outside_execution_calling_the_account_itself_is_refused() {
    let (_dir, _clock, core) = imported();
    let dapp = connected(&core);
    let own = core.active_account().unwrap().unwrap().address;
    let document = outside_execution(&[(own.as_str(), "transfer", vec!["0x1".into()])]);
    assert_eq!(code(&sign_request(&dapp, SITE, &document, 1)), 163);
    assert!(dapp.pending_approvals(NOW).is_empty());
}

// "queues a v2 transfer to a lookalike with its flag and refuses it at approval".
#[test]
fn an_outside_execution_paying_a_lookalike_is_flagged_then_refused() {
    let (_dir, _clock, core) = imported();
    let dapp = connected(&core);
    let own = core.active_account().unwrap().unwrap().address;
    let document = outside_execution(&[(
        STRK,
        "transfer",
        vec![lookalike(&own), "0x1".into(), "0x0".into()],
    )]);
    let card = pending_id(sign_request(&dapp, SITE, &document, 1));
    let (_, flags) = sign_card(&dapp);
    assert!(flags.contains(&RiskFlagCode::AddressPoisoning), "{flags:?}");
    assert_eq!(code(&approved(&dapp, card, account_id(&core))), 163);
}

// A Ledger account's outside execution is reviewed as any other's: a lookalike payee is flagged
// on the card and refused at approval, before the device is asked for anything.
#[test]
fn a_ledger_accounts_outside_execution_paying_a_lookalike_never_reaches_the_device() {
    let (_dir, _clock, core) = imported();
    add_ledger(&core, &ledger_key());
    let dapp = connected(&core);
    let account = core.active_account().unwrap().unwrap();
    let document = outside_execution(&[(
        STRK,
        "transfer",
        vec![lookalike(&account.address), "0x1".into(), "0x0".into()],
    )]);
    let card = pending_id(sign_request(&dapp, SITE, &document, 1));
    let (_, flags) = sign_card(&dapp);
    assert!(flags.contains(&RiskFlagCode::AddressPoisoning), "{flags:?}");
    let chain = ready_chain("argent 0.4.0", &account.public_key);
    let mut device = common::ledger_device::Device::holding(ledger_key());
    let step = dapp.approve_sign_typed_data(card.clone(), account.id, NOW);
    let reply = settled_reply(drive(&dapp, &card, step, &chain, Some(&mut device)));
    assert_eq!(code(&reply), 163);
    assert_eq!(device.apdus, 0);
}
