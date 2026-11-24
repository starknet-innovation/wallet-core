//! `Invoke::reviewed` against the extension's own cases, each named with its source. The plan's
//! handling of a batch is in `bramble-core`'s `tests/transactions_invoke.rs`.

use super::*;
use crate::catalog::TokenInfo;
use crate::kms::NetworkId;
use crate::send::call_decode::Known;
use crate::send::calls::{transfer_execute_calldata, TRANSFER_SELECTOR};
use crate::send::fees::Headroom;
use crate::send::review_flags::{ReviewContext, RiskFlag};
use crate::send::types::Authorization;
use crate::send::u256::U256;

// `verify.test.ts`'s account, token and contact.
const ACCOUNT: &str = "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcde";
const ETH: &str = "0x049d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7";
const CONTACT: &str = "0x777aa7890abcdef1234567890abcdef1234567890abcdef1234567890aabbcc";

fn felt(value: &str) -> Felt {
    Felt::from_hex(value).unwrap()
}

fn account() -> Account {
    Account {
        network: NetworkId::Mainnet,
        address: felt(ACCOUNT),
        public_key: Felt::from(3u8),
        authorization: Authorization::StarkPair,
        deployment: None,
    }
}

fn eth() -> TokenInfo {
    TokenInfo {
        address: ETH.into(),
        decimals: 18,
        name: "Ether".into(),
        symbol: "ETH".into(),
    }
}

fn call(contract: &str, entrypoint: &str, calldata: &[&str]) -> Call {
    Call {
        contract_address: felt(contract),
        entrypoint: entrypoint.into(),
        calldata: calldata.iter().map(|value| felt(value)).collect(),
    }
}

/// Bramble's `INVOKE_LIMITS` numbers, as `SEND_POLICY` sets them.
const POLICY: Policy = Policy {
    fee_margin_percent: 50,
    reviewed_fee_headroom: Headroom {
        numerator: 3,
        denominator: 2,
    },
    confirmation_delays_ms: &[0],
    max_invoke_calls: 16,
    max_call_calldata_felts: 4_096,
};

fn invoke(calls: Vec<Call>) -> Result<Invoke, InvokeRefusal> {
    let tokens = [eth()];
    let trusted = [felt(CONTACT)];
    let context = ReviewContext {
        known: Known {
            tokens: &tokens,
            nft_contracts: &[],
        },
        trusted: &trusted,
        ..ReviewContext::default()
    };
    Invoke::reviewed(&account(), calls, &POLICY, &context)
}

#[test]
fn an_empty_batch_is_refused() {
    // `rpc-network-ops.test.ts`, "rejects an empty call list before touching the network".
    assert_eq!(invoke(Vec::new()), Err(InvokeRefusal::NoCalls));
    assert_eq!(InvokeRefusal::NoCalls.to_string(), "No calls to submit");
}

#[test]
fn one_transfer_call_is_the_transfer_calldata() {
    let amount = U256::from_u128(1_500_000_000_000_000_000);
    let recipient = felt("0x3fa78c2861131190760b151f3ee110f473940eca3fec8cc423ecc307b5241a3");
    let batch = invoke(vec![Call {
        contract_address: felt(ETH),
        entrypoint: "transfer".into(),
        calldata: vec![recipient, Felt::from(amount.low), Felt::ZERO],
    }])
    .unwrap();
    assert_eq!(
        Some(batch.calldata),
        transfer_execute_calldata(&felt(ETH), &recipient, &amount)
    );
    assert_eq!(
        Some(crate::kms::selector("transfer")),
        Felt::from_hex(TRANSFER_SELECTOR).ok()
    );
    assert_eq!(batch.summary, "Send 1.5 ETH to 0x03fa…41a3");
}

#[test]
fn a_batch_compiles_every_call_in_order() {
    let batch = invoke(vec![
        call(ETH, "approve", &["0x5", "0x1", "0x0"]),
        call("0x123456789abcdef1122", "upgrade", &["0x1"]),
    ])
    .unwrap();
    let selector = |name: &str| crate::kms::selector(name);
    let expected = vec![
        Felt::from(2u8),
        felt(ETH),
        selector("approve"),
        Felt::from(3u8),
        Felt::from(5u8),
        Felt::ONE,
        Felt::ZERO,
        felt("0x123456789abcdef1122"),
        selector("upgrade"),
        Felt::ONE,
        Felt::ONE,
    ];
    assert_eq!(batch.calldata, expected);
    assert_eq!(
        batch.summary,
        "Allow 0x0000…0005 to spend 0.000000000000000001 ETH (+1 more)"
    );
}

#[test]
fn unknown_entrypoints_are_summarised_without_guessing() {
    // `verify.test.ts`, "summarizes unknown entrypoints without guessing".
    let batch = invoke(vec![call("0x123456789abcdef1122", "upgrade", &["0x1"])]).unwrap();
    assert!(
        batch.summary.starts_with("Call upgrade on"),
        "{}",
        batch.summary
    );
}

#[test]
fn a_payee_that_mimics_a_saved_contact_is_refused() {
    // `verify.test.ts`, "flags address poisoning against saved contacts"; the refusal is
    // `refuseOnPoisonedRecipient`'s, with the verifier's sentence.
    let poisoned = "0x777aa7890abcdef9999999999999999999999999999999999999990aabbcc";
    let refused = invoke(vec![call(ETH, "transfer", &[poisoned, "0x1", "0x0"])]);
    assert_eq!(
        refused,
        Err(InvokeRefusal::Review(CallReviewError::PoisonedRecipient))
    );
    assert_eq!(
        refused.unwrap_err().to_string(),
        "The recipient closely mimics an address you saved or paid but is a different account. Re-check every character before sending."
    );
}

#[test]
fn a_camel_case_transfer_from_payee_is_checked() {
    // `verify.test.ts`, "flags address-poisoning on a camelCase transferFrom recipient": a twin
    // of the account itself.
    let poisoned = format!(
        "{}ffffffffffffffffffffffffffffffffffffffffffffffffffff{}",
        &ACCOUNT[..8],
        &ACCOUNT[ACCOUNT.len() - 4..]
    );
    let refused = invoke(vec![call(
        ETH,
        "transferFrom",
        &[ACCOUNT, &poisoned, "0xde0b6b3a7640000", "0x0"],
    )]);
    assert_eq!(
        refused,
        Err(InvokeRefusal::Review(CallReviewError::PoisonedRecipient))
    );
}

#[test]
fn the_from_field_is_never_mistaken_for_the_recipient() {
    // `verify-nft-recipient-enforcement.test.ts`, "the from field is never mistaken for the
    // recipient": a lookalike in `from` refuses nothing.
    let lookalike = "0x777aa7890abcdef9999999999999999999999999999999999999990aabbcc";
    let other = "0x3fa78c2861131190760b151f3ee110f473940eca3fec8cc423ecc307b5241a3";
    assert!(invoke(vec![call(
        "0xabc",
        "transfer_from",
        &[lookalike, other, "0x7", "0x0"]
    )])
    .is_ok());
}

#[test]
fn a_swap_beneficiary_that_mimics_a_contact_is_refused() {
    // `verify-dapp-avnu-swap.test.ts`, "a beneficiary that mimics a saved contact is refused like
    // any poisoned recipient": the payee is read from the selector's felt even when the route
    // does not decode.
    let poisoned = "0x777aa7890abcdef9999999999999999999999999999999999999990aabbcc";
    let mut calldata = vec!["0x0"; 8];
    calldata.push(poisoned);
    assert_eq!(
        invoke(vec![call("0xabc", "multi_route_swap", &calldata)]),
        Err(InvokeRefusal::Review(CallReviewError::PoisonedRecipient))
    );
}

#[test]
fn a_call_to_the_account_itself_is_refused_in_any_spelling() {
    // `verify.test.ts`, "refuses account self-calls through felt aliases": a padded spelling of
    // the account is the account; a call to a token is not a self-call.
    let padded = format!("0x0{}", &ACCOUNT[2..]);
    let refused = invoke(vec![call(&padded, "upgrade", &["0x1"])]);
    assert_eq!(
        refused,
        Err(InvokeRefusal::Review(CallReviewError::AccountSelfCall))
    );
    assert_eq!(
        refused.unwrap_err().to_string(),
        "Dapps cannot call the wallet account contract itself"
    );
    // Anywhere in the batch, not only alone (the multisig rule allows a lone signer change).
    let batch = vec![
        call(ETH, "transfer", &[ETH, "0x1", "0x0"]),
        call(ACCOUNT, "change_owner", &[]),
    ];
    assert_eq!(
        invoke(batch),
        Err(InvokeRefusal::Review(CallReviewError::AccountSelfCall))
    );
    assert!(invoke(vec![call(ETH, "transfer", &[ETH, "0x1", "0x0"])]).is_ok());
}

#[test]
fn a_batch_must_fit_the_policy() {
    // `providerInvokeRequest`'s bounds (`INVOKE_LIMITS`), applied to every batch.
    let one = || call(ETH, "approve", &["0x5", "0x1", "0x0"]);
    assert!(invoke(vec![one(); 16]).is_ok());
    assert_eq!(invoke(vec![one(); 17]), Err(InvokeRefusal::TooManyCalls));
    let long = |felts: usize| Call {
        calldata: vec![Felt::ONE; felts],
        ..call("0xabc", "execute", &[])
    };
    assert!(invoke(vec![long(4_096)]).is_ok());
    assert_eq!(
        invoke(vec![long(4_097)]),
        Err(InvokeRefusal::CalldataTooLong)
    );
}

#[test]
fn every_call_names_a_contract_and_a_grammatical_entrypoint() {
    // `readProviderInvokeCall`: a non-zero contract, `^[A-Za-z_][A-Za-z0-9_]*$`, 64 at most.
    for name in [
        "",
        "1transfer",
        "trans-fer",
        "transfer()",
        "é",
        &"a".repeat(65),
    ] {
        assert_eq!(
            invoke(vec![call(ETH, name, &[])]),
            Err(InvokeRefusal::InvalidCall),
            "{name:?}"
        );
    }
    for name in ["Transfer", "_x", "transferFrom", &"a".repeat(64)] {
        assert!(invoke(vec![call(ETH, name, &[])]).is_ok(), "{name}");
    }
    assert_eq!(
        invoke(vec![call("0x0", "transfer", &[])]),
        Err(InvokeRefusal::InvalidCall)
    );
}

const UNKNOWN: &str = "0x6b17d1f2e12c4247f8bce6e563a40f2275d5e88540f0e9899d9ecfeeb8e0aff";
const MAX_U128: &str = "0xffffffffffffffffffffffffffffffff";

/// A transfer to the contact, then an unlimited approve on an unknown contract.
fn transfer_then_approve() -> Vec<Call> {
    vec![
        call(ETH, "transfer", &[CONTACT, "0x1", "0x0"]),
        call(UNKNOWN, "approve", &["0x999", MAX_U128, MAX_U128]),
        call(UNKNOWN, "approve", &["0x998", MAX_U128, MAX_U128]),
    ]
}

#[test]
fn every_call_keeps_its_summary_kind_and_flags() {
    let batch = invoke(transfer_then_approve()).unwrap();
    // The multisig-shaped line is unchanged.
    assert_eq!(
        batch.summary(),
        "Send 0.000000000000000001 ETH to 0x0777…bbcc (+2 more)"
    );
    let calls = batch.calls();
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[0].kind, CallKind::Transfer);
    assert_eq!(
        calls[0].summary,
        "Send 0.000000000000000001 ETH to 0x0777…bbcc"
    );
    assert!(calls[0].flags.is_empty());
    assert_eq!(calls[1].kind, CallKind::Approve);
    assert_eq!(
        calls[1].summary,
        "Authorize 0x0000…0999 for amount or NFT #115792089237316195423570985008687907853269984665640564039457584007913129639935 on unknown contract 0x06b1…0aff"
    );
    let unknown = RiskFlag::UnknownContract {
        contract: felt(UNKNOWN),
    };
    assert_eq!(
        calls[1].flags,
        [unknown.clone(), RiskFlag::UnlimitedApproval]
    );
    assert_eq!(calls[2].flags, calls[1].flags);
    // The card's list: each line once, in call order (`dedupeFlags`).
    assert_eq!(batch.flags(), [&unknown, &RiskFlag::UnlimitedApproval]);
}

#[test]
fn the_whole_context_reaches_the_flags() {
    // `Invoke::reviewed` with a pinned contract (`knownContracts`) and the caller's token list.
    let tokens = [eth()];
    let pinned = [felt(UNKNOWN)];
    let contracts = [felt(CONTACT)];
    let context = ReviewContext {
        known: Known {
            tokens: &tokens,
            nft_contracts: &[],
        },
        trusted: &[],
        known_contracts: &pinned,
        token_contracts: &contracts,
        standing: &[],
    };
    let batch = Invoke::reviewed(&account(), transfer_then_approve(), &POLICY, &context).unwrap();
    assert_eq!(
        batch.flags(),
        [
            &RiskFlag::TokenContractRecipient,
            &RiskFlag::UnlimitedApproval
        ]
    );
    // The refusals still come first: a poisoned payee is refused, not flagged.
    let trusted = [felt(CONTACT)];
    let poisoned = vec![call(
        ETH,
        "transfer",
        &[
            "0x777aa7890abcdef9999999999999999999999999999999999999990aabbcc",
            "0x1",
            "0x0",
        ],
    )];
    let context = ReviewContext {
        trusted: &trusted,
        ..context
    };
    assert_eq!(
        Invoke::reviewed(&account(), poisoned, &POLICY, &context),
        Err(InvokeRefusal::Review(CallReviewError::PoisonedRecipient))
    );
}

#[test]
fn the_activity_text_is_the_builders_or_the_review_line() {
    let batch = invoke(transfer_then_approve()).unwrap();
    assert_eq!(batch.activity_summary(), batch.summary());
    // swap-ops.ts's line, kept as given.
    let swap = "Swap 5 USDC for ~0.001 ETH via avnu";
    assert_eq!(
        batch.clone().with_activity_summary(swap).activity_summary(),
        swap
    );
    // An empty text leaves the review line.
    assert_eq!(
        batch.clone().with_activity_summary("").activity_summary(),
        batch.summary()
    );
    // `clampActivitySummary`: 200 UTF-16 units kept, 201 cut to 199 and an ellipsis.
    let exact = "a".repeat(199) + "é";
    assert_eq!(
        batch
            .clone()
            .with_activity_summary(&exact)
            .activity_summary(),
        exact
    );
    let long = "a".repeat(199) + "😀";
    assert_eq!(
        batch
            .clone()
            .with_activity_summary(&long)
            .activity_summary(),
        "a".repeat(199) + "…"
    );
}

#[test]
fn a_dapps_batch_names_its_origin_and_its_own_count() {
    // provider-ops.ts: "<activityOrigin(origin)> submitted N call(s)".
    let three = invoke(transfer_then_approve()).unwrap();
    assert_eq!(
        three
            .with_dapp_origin("https://app.avnu.fi")
            .activity_summary(),
        "https://app.avnu.fi submitted 3 calls"
    );
    let one = invoke(vec![call(ETH, "transfer", &[CONTACT, "0x1", "0x0"])]).unwrap();
    // `activityOrigin`: 80 units kept; a longer origin keeps its last 79 behind an ellipsis.
    let at_bound = format!("https://{}", "a".repeat(72));
    assert_eq!(
        one.clone().with_dapp_origin(&at_bound).activity_summary(),
        format!("{at_bound} submitted 1 call")
    );
    let past = format!("https://app.example.com.{}.evil.io", "b".repeat(70));
    let kept: String = past.chars().skip(past.chars().count() - 79).collect();
    assert_eq!(
        one.with_dapp_origin(&past).activity_summary(),
        format!("…{kept} submitted 1 call")
    );
}
