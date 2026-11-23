//! Each staking action (P13 `staking::{calls, review, prepare}`) against what the extension's
//! `wallet.stakingPrepare` and `wallet.stakingExecute` did (`fixtures/extension-staking.jsonl`,
//! block `action`): the request rules, the pinned read on the same wire, the calls estimated and
//! submitted (the calls the frozen invoke signs), the review sentence, the Activity row's text,
//! amount and token (a claim names the rewards token, every other action the staked token), and
//! the allowance filed; or the same refusal.

mod multisig_support;
mod staking_support;

use bramble_core::catalogs::networks;
use bramble_core::SEND_POLICY;
use multisig_support::TestPrimitives;
use serde_json::{json, Value};
use staking_support::*;
use starknet_wallet_core::send::call_decode::Known;
use starknet_wallet_core::send::calls::{parse_token_amount, Call};
use starknet_wallet_core::send::plan::Invoke;
use starknet_wallet_core::send::review_flags::ReviewContext;
use starknet_wallet_core::staking::calls::{build_staking_calls, BuildInput, StakingAction};
use starknet_wallet_core::staking::prepare::{prepare, PrepareContext, PrepareInput};
use starknet_wallet_core::staking::read::PoolOutcome;
use starknet_wallet_core::{Felt, NetworkId};

fn action(name: &str) -> StakingAction {
    match name {
        "stake" => StakingAction::Stake,
        "claim" => StakingAction::Claim,
        "unstake" => StakingAction::Unstake,
        "cancel-unstake" => StakingAction::CancelUnstake,
        "withdraw" => StakingAction::Withdraw,
        other => panic!("unknown action {other}"),
    }
}

// The extension's calls carry felts as starknet.js leaves them (`0x0471…`, unpadded hex).
fn address(value: &Value) -> Felt {
    Felt::from_hex(value.as_str().unwrap()).unwrap()
}

fn calls(value: &Value) -> Vec<Call> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|call| Call {
            contract_address: address(&call["contractAddress"]),
            entrypoint: call["entrypoint"].as_str().unwrap().into(),
            calldata: call["calldata"]
                .as_array()
                .unwrap()
                .iter()
                .map(address)
                .collect(),
        })
        .collect()
}

#[test]
fn each_action_is_prepared_as_the_extension_does() {
    let tokens = tokens();
    let account = account();
    let policy = policy(true);
    let network = policy.network(NetworkId::Mainnet).unwrap();
    let catalog = &networks::info(NetworkId::Mainnet).tokens;
    let review = ReviewContext {
        known: Known {
            tokens: catalog,
            nft_contracts: &[],
        },
        ..ReviewContext::default()
    };
    let primitives = TestPrimitives::new();
    for case in cases("action") {
        let name = case["name"].as_str().unwrap();
        let prepared = &case["prepared"];
        let action = action(case["action"].as_str().unwrap());
        let staker = case["staker"].as_str().unwrap();
        let input = match PrepareInput::new(action, case["amount"].as_str(), staker) {
            Err(refusal) => {
                assert_eq!(*prepared, json!({ "error": refusal.to_string() }), "{name}");
                assert!(case["rpc"].as_array().unwrap().is_empty(), "{name}");
                continue;
            }
            Ok(input) => input,
        };
        let token = tokens.iter().find(|t| case["tokenId"] == t.id.as_str());
        let token = token.unwrap();
        let mut read = input.read(network, &rewards(), &account, token).unwrap();
        let reading = run_read(&mut read, &case["rpc"], name);
        let context = PrepareContext {
            account: &account,
            staking: &policy,
            token,
            now: NOW,
            policy: &SEND_POLICY,
            review: &review,
            primitives: &primitives,
        };
        let preparation = match prepare(&input, reading.as_ref(), &context) {
            Err(refusal) => {
                assert_eq!(*prepared, json!({ "error": refusal.to_string() }), "{name}");
                assert!(
                    case["calls"].is_null() && case["activity"] == json!([]),
                    "{name}"
                );
                continue;
            }
            Ok(preparation) => preparation,
        };
        assert_eq!(prepared["review"], preparation.review, "{name}");
        let expected = calls(&case["calls"]);

        // The calls the action builds from the verified reading are the ones the extension
        // estimated and submitted.
        let reading = reading.unwrap();
        let PoolOutcome::Verified { pool, member } = &reading.pools()[0].outcome else {
            panic!("{name}: a prepared action has a verified pool");
        };
        let amount = input.amount.as_deref();
        let built = build_staking_calls(&BuildInput {
            account: account.address,
            action,
            amount: amount.and_then(|a| parse_token_amount(a, token.decimals).ok()),
            member: member.as_ref(),
            now: NOW,
            pool,
            token: reading.pools()[0].pair.token,
        })
        .unwrap();
        assert_eq!(built, expected, "{name}");

        // The frozen invoke signs exactly those calls, reviews them with the pool the one pinned
        // contract, and names the extension's Activity text, amount and token.
        let pinned = [preparation.pool];
        let reviewed = ReviewContext {
            known_contracts: &pinned,
            ..review
        };
        let invoke = Invoke::reviewed(
            &account,
            expected.clone(),
            &SEND_POLICY,
            &reviewed,
            &primitives,
        );
        let invoke = invoke.unwrap();
        assert_eq!(preparation.invoke.signed_calls(), expected, "{name}");
        assert_eq!(preparation.invoke.calls(), invoke.calls(), "{name}");
        assert_eq!(preparation.invoke.summary(), invoke.summary(), "{name}");
        let [row] = case["activity"].as_array().unwrap().as_slice() else {
            panic!("{name}: one Activity row expected");
        };
        assert_eq!(
            row["summary"],
            preparation.invoke.activity_summary(),
            "{name}"
        );
        assert_eq!(row["kind"], "invoke", "{name}");
        let asset = preparation.invoke.activity_asset().unwrap();
        assert_eq!(row["amount"].as_str(), asset.amount.as_deref(), "{name}");
        assert_eq!(row["token"], asset.symbol, "{name}");
        assert_eq!(address(&row["tokenAddress"]), asset.token_address, "{name}");

        // A stake files its exact-amount approval of the pool; nothing else grants one.
        let allowances: Vec<Value> = preparation
            .invoke
            .allowance_changes()
            .iter()
            .map(|change| {
                json!({
                    "amount": change.input.amount.to_decimal(),
                    "spender": change.input.spender,
                    "token": change.input.token,
                })
            })
            .collect();
        assert_eq!(json!(allowances), case["allowances"], "{name}");
    }
}
