//! `Invoke::new` against the extension's own cases, each named with its source. The plan's
//! handling of a batch is in `bramble-core`'s `tests/transactions_invoke.rs`.

use super::*;
use crate::catalog::TokenInfo;
use crate::kms::NetworkId;
use crate::send::calls::{transfer_execute_calldata, TRANSFER_SELECTOR};
use crate::send::types::Authorization;
use crate::send::u256::U256;

/// Test-only: the selector is sha3's Keccak (a dev-dependency); nothing here verifies.
struct Keccak;

impl StarkPrimitives for Keccak {
    fn selector(&self, entrypoint: &str) -> Option<Felt> {
        use sha3::{Digest, Keccak256};
        let mut digest: [u8; 32] = Keccak256::digest(entrypoint.as_bytes()).into();
        digest[0] &= 0x03;
        Some(Felt::from_bytes_be(&digest))
    }

    fn verify(&self, _: &Felt, _: &Felt, _: &Felt, _: &Felt) -> bool {
        false
    }
}

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

fn invoke(calls: Vec<Call>) -> Result<Invoke, InvokeRefusal> {
    let tokens = [eth()];
    let known = Known {
        tokens: &tokens,
        nft_contracts: &[],
    };
    Invoke::new(&account(), calls, &known, &[felt(CONTACT)], &Keccak)
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
        Keccak.selector("transfer"),
        Felt::from_hex(TRANSFER_SELECTOR).ok()
    );
    assert_eq!(batch.summary, "Send 1.5 ETH to 0x3fa7…41a3");
}

#[test]
fn a_batch_compiles_every_call_in_order() {
    let batch = invoke(vec![
        call(ETH, "approve", &["0x5", "0x1", "0x0"]),
        call("0x123456789abcdef1122", "upgrade", &["0x1"]),
    ])
    .unwrap();
    let selector = |name: &str| Keccak.selector(name).unwrap();
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
        "Allow 0x5…0x5 to spend 0.000000000000000001 ETH (+1 more)"
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
