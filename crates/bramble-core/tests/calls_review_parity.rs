//! P9 and P1's call summaries against `extension-call-review.jsonl`, written by Bramble's
//! `scripts/mobile-parity-calls/` from the extension's own code:
//!
//! - `decode`: `send::call_decode::decode_call` and `recipient_for_flags` agree with `decodeCall`
//!   and `recipientForFlags` on every call shape, spelling and near-miss;
//! - `flags`: `Invoke::reviewed` over the same batch, context and allowance log reports each
//!   call's decode and the card's flags as `verifyCalls` does, and refuses exactly where
//!   `refuseOnPoisonedRecipient` does, with its sentence;
//! - `submit`: the same four-call batch, as a `Kind::Invoke` plan, puts the extension's requests
//!   on the wire in order (execute calldata, v3 hash, fee bounds), signs the same hashes, and is
//!   accepted under the same hash, or refused where the extension's fee ceiling refuses;
//! - `refusal`: what the invoke path refuses before reading anything is refused by `reviewed`,
//!   and the batches on each bound pass.

mod multisig_support;
mod plan_support;

use bramble_core::catalogs::accounts::{send_authorization, AccountFamily};
use bramble_core::catalogs::networks::registry;
use bramble_core::SEND_POLICY;
use multisig_support::TestPrimitives;
use plan_support::*;
use serde_json::Value;
use starknet_wallet_core::allowances::record::AllowanceRecord;
use starknet_wallet_core::catalog::TokenInfo;
use starknet_wallet_core::send::call_decode::{decode_call, recipient_for_flags, CallKind, Known};
use starknet_wallet_core::send::calls::Call;
use starknet_wallet_core::send::multisig_review::CallReviewError;
use starknet_wallet_core::send::plan::{Account, Invoke, InvokeRefusal, Kind, Plan};
use starknet_wallet_core::send::review_flags::ReviewContext;
use starknet_wallet_core::send::types::{SendFailureCode, SendStep, StepReply};
use starknet_wallet_core::{Felt, NetworkId};

fn cases(block: &str) -> Vec<Value> {
    let path = format!(
        "{}/tests/fixtures/extension-call-review.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|case| case["block"] == block)
        .collect()
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap()
}

fn felts(values: &Value) -> Vec<Felt> {
    values
        .as_array()
        .unwrap()
        .iter()
        .map(|v| f(text(v)))
        .collect()
}

fn call(value: &Value) -> Call {
    Call {
        contract_address: f(text(&value["contractAddress"])),
        entrypoint: text(&value["entrypoint"]).into(),
        calldata: felts(&value["calldata"]),
    }
}

fn calls(values: &Value) -> Vec<Call> {
    values.as_array().unwrap().iter().map(call).collect()
}

fn tokens(values: &Value) -> Vec<TokenInfo> {
    values
        .as_array()
        .unwrap()
        .iter()
        .map(|token| TokenInfo {
            address: text(&token["address"]).into(),
            decimals: token["decimals"].as_u64().unwrap() as u8,
            name: text(&token["name"]).into(),
            symbol: text(&token["symbol"]).into(),
        })
        .collect()
}

fn strings(values: &Value) -> Vec<String> {
    values
        .as_array()
        .unwrap()
        .iter()
        .map(|v| text(v).into())
        .collect()
}

fn kind(name: &str) -> CallKind {
    match name {
        "approve" => CallKind::Approve,
        "nft-approval" => CallKind::NftApproval,
        "operator-approval" => CallKind::OperatorApproval,
        "swap" => CallKind::Swap,
        "transfer" => CallKind::Transfer,
        "unknown" => CallKind::Unknown,
        other => panic!("unmapped kind {other}"),
    }
}

fn network_of(id: &str) -> NetworkId {
    NetworkId::from_catalog_id(id).unwrap()
}

/// An OpenZeppelin account at `address`; the review reads only its network and address.
fn actor(network: NetworkId, address: Felt) -> Account {
    Account {
        network,
        address,
        public_key: Felt::ONE,
        authorization: send_authorization(AccountFamily::OpenZeppelin),
        deployment: None,
    }
}

fn reviewed(
    account: &Account,
    batch: Vec<Call>,
    context: &ReviewContext,
) -> Result<Invoke, InvokeRefusal> {
    Invoke::reviewed(
        account,
        batch,
        &SEND_POLICY,
        context,
        &TestPrimitives::new(),
    )
}

#[test]
fn every_call_decodes_as_the_extension_decodes_it() {
    let cases = cases("decode");
    assert_eq!(cases.len(), 46);
    for case in cases {
        let name = text(&case["name"]);
        let tokens = tokens(&case["tokens"]);
        let nft_contracts = strings(&case["nftContracts"]);
        let known = Known {
            tokens: &tokens,
            nft_contracts: &nft_contracts,
        };
        let call = call(&case["call"]);
        let decoded = decode_call(
            &call.contract_address,
            &call.entrypoint,
            &call.calldata,
            &known,
        );
        assert_eq!(decoded.kind, kind(text(&case["decoded"]["kind"])), "{name}");
        assert_eq!(decoded.summary, text(&case["decoded"]["summary"]), "{name}");
        // `recipientForFlags` answers "" where the selector names no payee or its felt is absent.
        let recipient = match text(&case["recipient"]) {
            "" => None,
            address => Some(f(address)),
        };
        assert_eq!(
            recipient_for_flags(&call.entrypoint, &call.calldata),
            recipient,
            "{name}"
        );
    }
}

#[test]
fn every_batch_is_flagged_and_refused_as_the_extension_does() {
    let cases = cases("flags");
    assert_eq!(cases.len(), 32);
    for case in cases {
        let name = text(&case["name"]);
        let context = &case["context"];
        let network = network_of(text(&context["account"]["networkId"]));
        let account = actor(network, f(text(&context["account"]["address"])));
        let tokens = tokens(&context["tokens"]);
        let nft_contracts = strings(&context["nftContracts"]);
        // `addressPoisoningFlag`'s trusted set; the account itself the core adds.
        let trusted: Vec<Felt> = ["ownAccounts", "addressBook", "recentRecipients"]
            .iter()
            .flat_map(|list| felts(&context[*list]))
            .collect();
        let known_contracts = felts(&context["knownContracts"]);
        // `tokenContractRecipientFlag`'s list beyond the tracked tokens: STRK and the curated
        // registry on the account's network.
        let token_contracts: Vec<Felt> = std::iter::once(f(STRK))
            .chain(
                registry()
                    .tokens
                    .iter()
                    .filter(|token| token.network_id == network.catalog_id())
                    .map(|token| f(&token.address)),
            )
            .collect();
        let standing: Vec<AllowanceRecord> = case["standing"]
            .as_array()
            .unwrap()
            .iter()
            .map(|record| AllowanceRecord::from_json(record).expect(name))
            .collect();
        let review = ReviewContext {
            known: Known {
                tokens: &tokens,
                nft_contracts: &nft_contracts,
            },
            trusted: &trusted,
            known_contracts: &known_contracts,
            token_contracts: &token_contracts,
            standing: &standing,
        };
        let batch_calls = calls(&case["calls"]);
        let result = reviewed(&account, batch_calls.clone(), &review);
        let expected: Vec<(CallKind, &str)> = case["decoded"]
            .as_array()
            .unwrap()
            .iter()
            .map(|call| (kind(text(&call["kind"])), text(&call["summary"])))
            .collect();
        if let Some(refusal) = case["refusal"].as_str() {
            assert_eq!(
                result,
                Err(InvokeRefusal::Review(CallReviewError::PoisonedRecipient)),
                "{name}"
            );
            assert_eq!(result.unwrap_err().to_string(), refusal, "{name}");
            // A refused batch has no `Invoke`, so its decode is read call by call with the same
            // `known`. Its co-flags (`flags`) are left unasserted: the core computes them only
            // inside `Invoke::reviewed` (`review_flags::call_flags` is crate-private).
            let decoded: Vec<_> = batch_calls
                .iter()
                .map(|call| {
                    decode_call(
                        &call.contract_address,
                        &call.entrypoint,
                        &call.calldata,
                        &review.known,
                    )
                })
                .collect();
            let decoded: Vec<(CallKind, &str)> = decoded
                .iter()
                .map(|call| (call.kind, call.summary.as_str()))
                .collect();
            assert_eq!(decoded, expected, "{name}");
            continue;
        }
        let batch = result.unwrap_or_else(|refusal| panic!("{name}: {refusal}"));
        let decoded: Vec<(CallKind, &str)> = batch
            .calls()
            .iter()
            .map(|call| (call.kind, call.summary.as_str()))
            .collect();
        assert_eq!(decoded, expected, "{name}");
        let flags: Vec<(&str, String)> = batch
            .flags()
            .into_iter()
            .map(|flag| (flag.code(), flag.message()))
            .collect();
        let expected: Vec<(&str, String)> = case["flags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|flag| (text(&flag["code"]), text(&flag["message"]).to_string()))
            .collect();
        assert_eq!(flags, expected, "{name}");
    }
}

/// The scenario with its wire phases as `plan_support` reads them, one JSON string per exchange.
fn as_capture(scenario: &Value) -> Value {
    let mut scenario = scenario.clone();
    for phase in ["reviewInvoke", "reviewDeployment", "submit"] {
        let lines: Vec<Value> = scenario[phase]
            .as_array()
            .unwrap()
            .iter()
            .map(|exchange| Value::String(exchange.to_string()))
            .collect();
        scenario[phase] = Value::Array(lines);
    }
    scenario
}

fn batch_plan(scenario: &Value, account: Account) -> Plan {
    let batch = reviewed(
        &account,
        calls(&scenario["calls"]),
        &ReviewContext::default(),
    )
    .unwrap_or_else(|refusal| panic!("{refusal}"));
    Plan::new(account, Kind::Invoke(Box::new(batch)), SEND_POLICY)
}

#[test]
fn a_multicall_goes_on_the_wire_as_the_extension_sends_it() {
    let mut accepted = 0;
    for case in cases("submit") {
        let scenario = as_capture(&case);
        let name = text(&scenario["name"]);
        if let Some(refusal) = scenario["error"].as_str() {
            // The fee rose past the reviewed ceiling: refused after the re-estimate, unsigned.
            assert_eq!(
                refusal,
                "The network fee for this transaction rose above the amount you reviewed. Nothing was submitted — review it again.",
                "{name}"
            );
            let host = Host::new();
            let mut plan = batch_plan(
                &scenario,
                account(&scenario, "openzeppelin", host.public_key()),
            );
            let step = plan.advance(&host, StepReply::Start).unwrap();
            let step = drive(&mut plan, &host, step, &review_calls(&scenario), &scenario);
            assert_eq!(step, SendStep::Review, "{name}");
            let step = plan.advance(&host, StepReply::Confirm).unwrap();
            let step = drive(
                &mut plan,
                &host,
                step,
                &phase(&scenario, "submit"),
                &scenario,
            );
            let SendStep::Failed { failure } = step else {
                panic!("{name}: not refused, {step:?}");
            };
            assert_eq!(failure.code, SendFailureCode::ReviewedFeeExceeded, "{name}");
            assert!(host.signed.borrow().is_empty(), "{name}");
            continue;
        }
        // Same requests in order, the same hashes signed, accepted under the same hash.
        run_with(&scenario, "openzeppelin", batch_plan);
        accepted += 1;
    }
    assert_eq!(accepted, 3);
}

#[test]
fn what_the_invoke_path_refuses_up_front_is_refused() {
    let cases = cases("refusal");
    assert_eq!(cases.len(), 13);
    let primitives = TestPrimitives::new();
    for case in cases {
        let name = text(&case["name"]);
        let address = case["account"].as_str().map_or(Felt::from(0x5au8), f);
        let account = actor(NetworkId::Mainnet, address);
        let result = Invoke::reviewed(
            &account,
            calls(&case["calls"]),
            &SEND_POLICY,
            &ReviewContext::default(),
            &primitives,
        );
        if case["accepted"].as_bool().unwrap() {
            assert!(result.is_ok(), "{name}: {result:?}");
            continue;
        }
        let refusal = result.err().unwrap_or_else(|| panic!("{name}: accepted"));
        match case["error"].as_str() {
            // `submitInvokeTransaction` and `refuseOnAccountSelfCall` refuse in words.
            Some(sentence) => assert_eq!(refusal.to_string(), sentence, "{name}"),
            // `providerInvokeRequest` answers null: the core names which bound or rule.
            None => {
                let expected = match name {
                    "one call too many" => InvokeRefusal::TooManyCalls,
                    "calldata one felt too long" => InvokeRefusal::CalldataTooLong,
                    "an entrypoint one character too long"
                    | "an entrypoint starting with a digit"
                    | "an entrypoint with a hyphen"
                    | "an empty entrypoint"
                    | "a zero contract" => InvokeRefusal::InvalidCall,
                    other => panic!("unmapped refusal {other}"),
                };
                assert_eq!(refusal, expected, "{name}");
            }
        }
    }
}
