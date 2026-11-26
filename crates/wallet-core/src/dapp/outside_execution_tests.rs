// The extension's `packages/features/transactions/tests/routing/outside-execution-calls.test.ts`
// (the reader), and the review half of `tests/background/approvals/outside-execution-sign.test.ts`
// and `outside-execution-trusted-recipients.test.ts`. Each test is named after its TS one. The
// envelopes are starknet.js 10.8's `outsideExecution.getTypedData` output, spelled out.

use serde_json::{json, Value};

use super::*;
use crate::allowances::record::AllowanceRecord;
use crate::catalog::TokenInfo;
use crate::dapp::typed_data::tests::BUDGET;
use crate::js::json;
use crate::send::call_decode::Known;
use crate::send::u256::U256;

const ACCOUNT: &str = "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcde";
const ETH: &str = "0x49d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7";
const MAX_U128: &str = "0xffffffffffffffffffffffffffffffff";
const HALF_U128: &str = "0x80000000000000000000000000000000";
// Same leading and trailing digits as the account, different address.
const LOOKALIKE: &str = "0x12340000000000000000000000000000000000000000000000000000000bcde";
const CONTACT: &str = "0x56780000000000000000000000000000000000000000000000000000000abc";
const CONTACT_LOOKALIKE: &str = "0x56781111111111111111111111111111111111111111111111111111110abc";

fn felt(hex: &str) -> Felt {
    Felt::from_hex(hex).unwrap()
}

fn hex(value: &Felt) -> String {
    felt::canonical(value)
}

/// `{ calldata, contractAddress, entrypoint }`.
fn call(contract: &str, entrypoint: &str, calldata: &[&str]) -> Call {
    Call {
        contract_address: felt(contract),
        entrypoint: entrypoint.into(),
        calldata: calldata.iter().map(|value| felt(value)).collect(),
    }
}

/// `outsideExecution.getTypedData(SN_MAIN, { caller, execute_after: 1, execute_before:
/// 999_999_999_999 }, 1, calls, version)`, through `JSON.stringify`.
fn envelope(version: u8, caller: &str, calls: &[Call]) -> Value {
    let calldata = |call: &Call| -> Vec<String> { call.calldata.iter().map(hex).collect() };
    let selector = |call: &Call| hex(&kms::selector(&call.entrypoint));
    if version == 1 {
        let calls: Vec<Value> = calls
            .iter()
            .map(|call| {
                json!({
                    "to": hex(&call.contract_address),
                    "selector": selector(call),
                    "calldata_len": call.calldata.len(),
                    "calldata": calldata(call),
                })
            })
            .collect();
        return json!({
            "types": {
                "StarkNetDomain": [
                    { "name": "name", "type": "felt" },
                    { "name": "version", "type": "felt" },
                    { "name": "chainId", "type": "felt" },
                ],
                "OutsideExecution": [
                    { "name": "caller", "type": "felt" },
                    { "name": "nonce", "type": "felt" },
                    { "name": "execute_after", "type": "felt" },
                    { "name": "execute_before", "type": "felt" },
                    { "name": "calls_len", "type": "felt" },
                    { "name": "calls", "type": "OutsideCall*" },
                ],
                "OutsideCall": [
                    { "name": "to", "type": "felt" },
                    { "name": "selector", "type": "felt" },
                    { "name": "calldata_len", "type": "felt" },
                    { "name": "calldata", "type": "felt*" },
                ],
            },
            "primaryType": "OutsideExecution",
            "domain": {
                "name": "Account.execute_from_outside",
                "version": "1",
                "chainId": "0x534e5f4d41494e",
            },
            "message": {
                "caller": caller,
                "nonce": 1,
                "execute_after": 1,
                "execute_before": 999_999_999_999u64,
                "calls_len": calls.len(),
                "calls": calls,
            },
        });
    }
    let calls: Vec<Value> = calls
        .iter()
        .map(|call| {
            json!({
                "To": hex(&call.contract_address),
                "Selector": selector(call),
                "Calldata": calldata(call),
            })
        })
        .collect();
    json!({
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
            "chainId": "0x534e5f4d41494e",
            "revision": "1",
        },
        "message": {
            "Caller": caller,
            "Nonce": 1,
            "Execute After": 1,
            "Execute Before": 999_999_999_999u64,
            "Calls": calls,
        },
    })
}

/// The sign tests' `outside(calls, version)`: caller `0x0`.
fn outside(calls: &[Call], version: u8) -> Value {
    envelope(version, "0x0", calls)
}

fn read(value: &Value) -> Result<Option<Vec<Call>>, WalletRpcError> {
    outside_execution_calls(&json::parse(&value.to_string()).unwrap())
}

// ---- outside-execution-calls.test.ts ----

/// `OPTIONS.caller: "ANY_CALLER"`, as starknet.js encodes it.
const ANY_CALLER: &str = "0x414e595f43414c4c4552";

fn reader_calls() -> Vec<Call> {
    vec![
        call(ETH, "transfer", &["0xabc", "0x5", "0x0"]),
        call(ETH, "approve", &["0xdef", "0x7", "0x0"]),
    ]
}

#[test]
fn recovers_named_entrypoints_from_a_v1_and_v2_envelope() {
    for version in [1, 2] {
        let typed = envelope(version, ANY_CALLER, &reader_calls());
        assert_eq!(read(&typed), Ok(Some(reader_calls())), "v{version}");
    }
}

#[test]
fn keys_the_layout_on_the_message_shape_not_the_domain_version_string() {
    let mut typed = envelope(2, ANY_CALLER, &reader_calls());
    typed["domain"]["version"] = json!("0x32");
    assert_eq!(read(&typed), Ok(Some(reader_calls())));
}

#[test]
fn keeps_an_unknown_selector_as_its_hex_spelling() {
    let typed = envelope(2, ANY_CALLER, &[call(ETH, "set_owner", &["0x1"])]);
    let selector = hex(&kms::selector("set_owner"));
    assert_eq!(read(&typed), Ok(Some(vec![call(ETH, &selector, &["0x1"])])));
}

#[test]
fn resolves_felts_the_way_the_hasher_does_decimal_number_v2_selector_name() {
    let mut typed = envelope(2, ANY_CALLER, &reader_calls());
    typed["message"]["Calls"][0]["Selector"] = json!("transfer");
    typed["message"]["Calls"][0]["Calldata"] = json!(["2748", 5, "0"]);
    assert_eq!(read(&typed), Ok(Some(reader_calls())));
}

#[test]
fn leaves_a_plain_snip12_message_alone() {
    assert_eq!(
        read(&json!({ "message": {}, "primaryType": "Mail" })),
        Ok(None)
    );
    assert_eq!(read(&json!("not typed data")), Ok(None));
}

#[test]
fn reports_a_missing_ambiguous_or_mistyped_call_list_as_malformed() {
    let malformed = Err(WalletRpcError::invalid_request_payload());

    let mut missing = envelope(1, ANY_CALLER, &reader_calls());
    missing["message"].as_object_mut().unwrap().remove("calls");
    assert_eq!(read(&missing), malformed);

    let mut both = envelope(1, ANY_CALLER, &reader_calls());
    both["message"]["Calls"] = json!([]);
    assert_eq!(read(&both), malformed);

    let mut mistyped = envelope(2, ANY_CALLER, &reader_calls());
    mistyped["message"]["Calls"][0]["Calldata"] = json!("0x1");
    assert_eq!(read(&mistyped), malformed);

    let mut bad_felt = envelope(2, ANY_CALLER, &reader_calls());
    bad_felt["message"]["Calls"][0]["To"] = json!({ "nested": true });
    assert_eq!(read(&bad_felt), malformed);

    assert_eq!(
        read(&json!({ "primaryType": "OutsideExecution" })),
        malformed
    );
}

// ---- outside-execution-sign.test.ts ----

fn eth() -> Vec<TokenInfo> {
    vec![TokenInfo {
        address: ETH.into(),
        decimals: 18,
        name: "Ether".into(),
        symbol: "ETH".into(),
    }]
}

/// The review of `typed` for the account on mainnet, through the typed-data gate first.
fn review(
    typed: &Value,
    trusted: &[Felt],
    standing: &[AllowanceRecord],
    stage: Stage,
) -> Result<Option<OutsideExecutionReview>, WalletRpcError> {
    let value = json::parse(&typed.to_string()).unwrap();
    let checked = CheckedTypedData::check(&value, &BUDGET).expect("the gate accepts it");
    let tokens = eth();
    let context = ReviewContext {
        known: Known {
            tokens: &tokens,
            nft_contracts: &[],
        },
        trusted,
        standing,
        ..ReviewContext::default()
    };
    review_outside_execution(
        &checked,
        NetworkId::Mainnet,
        &felt(ACCOUNT),
        &context,
        stage,
    )
}

/// The flag codes on the card queued for `typed`.
fn queued_flags(
    typed: &Value,
    trusted: &[Felt],
    standing: &[AllowanceRecord],
) -> Vec<&'static str> {
    let review = review(typed, trusted, standing, Stage::Queue)
        .unwrap()
        .expect("an outside execution");
    review.flags.iter().map(RiskFlag::code).collect()
}

/// Refused at approval: what a page sees for the extension's "closely mimics" or self-call
/// sentence (`errorForMessage` maps neither, so UNKNOWN_ERROR).
fn refused() -> Result<Option<OutsideExecutionReview>, WalletRpcError> {
    Err(WalletRpcError::unknown_error())
}

#[test]
fn queues_a_v1_and_v2_transfer_to_a_lookalike_with_its_flag_and_refuses_it_at_approval() {
    for version in [1, 2] {
        let typed = outside(
            &[call(ETH, "transfer", &[LOOKALIKE, "0x1", "0x0"])],
            version,
        );
        assert_eq!(queued_flags(&typed, &[], &[]), ["address-poisoning"]);
        assert_eq!(
            review(&typed, &[], &[], Stage::Sign),
            refused(),
            "v{version}"
        );
    }
}

#[test]
fn refuses_a_call_to_the_account_contract_itself() {
    let typed = outside(&[call(ACCOUNT, "upgrade", &["0x1"])], 2);
    assert_eq!(review(&typed, &[], &[], Stage::Queue), refused());
    assert_eq!(review(&typed, &[], &[], Stage::Sign), refused());
}

#[test]
fn refuses_an_outside_execution_the_reader_cannot_parse() {
    let typed = json!({
        "domain": {
            "chainId": "0x534e5f4d41494e",
            "name": "Account.execute_from_outside",
            "revision": "1",
            "version": "2",
        },
        "message": { "Nonce": "0x1" },
        "primaryType": "OutsideExecution",
        "types": {
            "OutsideExecution": [{ "name": "Nonce", "type": "felt" }],
            "StarknetDomain": [
                { "name": "name", "type": "shortstring" },
                { "name": "version", "type": "shortstring" },
                { "name": "chainId", "type": "shortstring" },
                { "name": "revision", "type": "shortstring" },
            ],
        },
    });
    assert_eq!(
        review(&typed, &[], &[], Stage::Queue),
        Err(WalletRpcError::invalid_request_payload())
    );
}

#[test]
fn queues_an_unlimited_approve_with_its_flag_and_decoded_summary() {
    let typed = outside(&[call(ETH, "approve", &[CONTACT, MAX_U128, MAX_U128])], 2);
    let review = review(&typed, &[], &[], Stage::Queue).unwrap().unwrap();
    let codes: Vec<_> = review.flags.iter().map(RiskFlag::code).collect();
    assert!(codes.contains(&"unlimited-approval"), "{codes:?}");
    assert_eq!(review.summaries.len(), 2);
    assert_eq!(
        review.summaries[0],
        "OutsideExecution · Account.execute_from_outside"
    );
    assert!(
        review.summaries[1].contains("Allow"),
        "{}",
        review.summaries[1]
    );
}

// The selector-to-name map covers every decoded entrypoint, not only transfer/approve: v1
// carries raw selector hashes and decimal calldata.
#[test]
fn flags_a_v1_transfer_from_to_a_lookalike_with_decimal_calldata() {
    let mut typed = outside(
        &[call(
            ETH,
            "transfer_from",
            &[ACCOUNT, LOOKALIKE, "0x1", "0x0"],
        )],
        1,
    );
    for value in typed["message"]["calls"][0]["calldata"]
        .as_array_mut()
        .unwrap()
    {
        *value = json!(felt(value.as_str().unwrap()).to_string());
    }
    assert!(queued_flags(&typed, &[], &[]).contains(&"address-poisoning"));
}

#[test]
fn flags_increase_allowance_grants_that_add_up_to_an_unlimited_allowance() {
    let grant = call(ETH, "increase_allowance", &[CONTACT, HALF_U128, "0x0"]);
    let typed = outside(&[grant.clone(), grant], 2);
    assert!(queued_flags(&typed, &[], &[]).contains(&"unlimited-approval"));
}

/// The recorded allowance of `CONTACT` on ETH for the account: `2^128 - 10`.
fn standing() -> AllowanceRecord {
    AllowanceRecord {
        id: String::new(),
        account_id: format!("SN_MAIN:{ACCOUNT}"),
        network: NetworkId::Mainnet,
        token: ETH.into(),
        spender: CONTACT.into(),
        amount: U256 {
            high: 0,
            low: u128::MAX - 9,
        },
        lower_bound: false,
        revoking: false,
        revoke_hash: None,
    }
}

#[test]
fn flags_a_small_increase_allowance_that_tops_up_a_standing_allowance() {
    let typed = outside(
        &[call(ETH, "increase_allowance", &[CONTACT, "0x64", "0x0"])],
        2,
    );
    assert!(queued_flags(&typed, &[], &[standing()]).contains(&"unlimited-approval"));
}

#[test]
fn does_not_flag_the_same_small_increase_allowance_with_no_standing_allowance() {
    let typed = outside(
        &[call(ETH, "increase_allowance", &[CONTACT, "0x64", "0x0"])],
        2,
    );
    assert!(!queued_flags(&typed, &[], &[]).contains(&"unlimited-approval"));
}

#[test]
fn refuses_at_approval_when_a_contact_added_meanwhile_makes_the_payee_a_lookalike() {
    let typed = outside(
        &[call(ETH, "transfer", &[CONTACT_LOOKALIKE, "0x1", "0x0"])],
        2,
    );
    assert_eq!(queued_flags(&typed, &[], &[]), Vec::<&str>::new());
    assert_eq!(
        review(&typed, &[felt(CONTACT)], &[], Stage::Sign),
        refused()
    );
}

#[test]
fn leaves_plain_snip12_messages_unchanged() {
    let typed = json!({
        "domain": { "chainId": "0x534e5f4d41494e", "name": "Example", "revision": "1", "version": "1" },
        "message": { "contents": "hello" },
        "primaryType": "Mail",
        "types": {
            "Mail": [{ "name": "contents", "type": "felt" }],
            "StarknetDomain": [
                { "name": "name", "type": "felt" },
                { "name": "version", "type": "felt" },
                { "name": "chainId", "type": "felt" },
            ],
        },
    });
    for stage in [Stage::Queue, Stage::Sign] {
        assert_eq!(review(&typed, &[], &[], stage), Ok(None));
    }
    let value = json::parse(&typed.to_string()).unwrap();
    assert_eq!(describe_typed_data(&value), "Mail · Example");
}

// The card's flag sits last among its call's, in `verifyCalls`'s order, with its sentence.
#[test]
fn the_poisoning_flag_carries_the_extensions_sentence() {
    let typed = outside(&[call(ETH, "transfer", &[LOOKALIKE, "0x1", "0x0"])], 2);
    let review = review(&typed, &[], &[], Stage::Queue).unwrap().unwrap();
    let flag = &review.flags[0];
    assert_eq!(
        flag.message(),
        "The recipient closely mimics an address you saved or paid but is a different account. Re-check every character before sending."
    );
    assert_eq!(flag.text().key, "review.flag.addressPoisoning");
}

#[path = "outside_execution_trusted_tests.rs"]
mod trusted;

// `verify-spender-poisoning.test.ts` (Bramble #1038) through this route: a grant whose spender
// mimics a contact is refused at approval, as the invoke route refuses it (`refuse_poisoned`),
// and an approve to the contact itself is not.
#[test]
fn refuses_at_approval_a_grant_to_a_spender_that_mimics_a_contact() {
    let grant = |spender| outside(&[call(ETH, "approve", &[spender, "0x1", "0x0"])], 2);
    let contact = [felt(CONTACT)];
    assert_eq!(
        review(&grant(CONTACT_LOOKALIKE), &contact, &[], Stage::Sign),
        refused()
    );
    assert!(review(&grant(CONTACT), &contact, &[], Stage::Sign).is_ok());
}
