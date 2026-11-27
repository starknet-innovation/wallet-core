//! The extension's NFT send cases, each named with its source: `collectibles/nft.test.ts` (the
//! calls), `routing/verify-nft-transfer.test.ts` (the review), `nft-inputs.ts` and `nft-ops.ts`
//! (the request and its refusals). G-NFT's golden requests arrive with the generator.

use super::*;
use crate::kms::NetworkId;
use crate::send::fees::Headroom;
use crate::send::multisig_review::CallReviewError;
use crate::send::review_flags::RiskFlag;
use crate::send::types::{Authorization, PendingKind};

// `verify-nft-transfer.test.ts`'s collection, account and contact.
const NFT_CONTRACT: &str = "0x0511a2b3c4d5e6f7a8b9c0d1e2f30411223344556677889900aabbccddeeff00";
const ACCOUNT: &str = "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcde";
const CONTACT: &str = "0x777aa7890abcdef1234567890abcdef1234567890abcdef1234567890aabbcc";
const POISONED: &str = "0x777aa7890abcdef9999999999999999999999999999999999999990aabbcc";

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

fn request(recipient: &str, token_id: u128) -> NftTransferRequest {
    NftTransferRequest {
        network: NetworkId::Mainnet,
        contract: felt(NFT_CONTRACT),
        token_id: U256::from_u128(token_id),
        recipient: recipient.into(),
    }
}

fn prepare(
    request: &NftTransferRequest,
    owned: Option<NftStandard>,
) -> Result<Invoke, NftTransferRefusal> {
    let trusted = [felt(CONTACT)];
    let review = ReviewContext {
        trusted: &trusted,
        ..ReviewContext::default()
    };
    prepare_transfer(&account(), request, owned, &POLICY, &review)
}

fn decimals(call: &Call) -> Vec<String> {
    call.calldata.iter().map(Felt::to_string).collect()
}

#[test]
fn an_erc721_send_is_transfer_from_with_a_u256_token_id() {
    // `nft.test.ts`, "builds transfer_from calldata with a u256 token id" and "allows token id
    // zero".
    let (contract, from, to) = (Felt::from(3u8), Felt::ONE, Felt::TWO);
    let call = transfer_call(NftStandard::Erc721, contract, from, to, U256::from_u128(5)).unwrap();
    assert_eq!(
        (call.contract_address, call.entrypoint.as_str()),
        (contract, "transfer_from")
    );
    assert_eq!(decimals(&call), ["1", "2", "5", "0"]);
    let zero = transfer_call(NftStandard::Erc721, contract, from, to, U256::ZERO).unwrap();
    assert_eq!(decimals(&zero), ["1", "2", "0", "0"]);
    let high = U256 { high: 7, low: 9 };
    let call = transfer_call(NftStandard::Erc721, contract, from, to, high).unwrap();
    assert_eq!(decimals(&call), ["1", "2", "9", "7"]);
}

#[test]
fn an_erc1155_send_moves_one_edition_with_empty_data() {
    // `nft.test.ts`, "builds a one-edition safe_transfer_from with empty data".
    let call = transfer_call(
        NftStandard::Erc1155,
        Felt::from(3u8),
        Felt::ONE,
        Felt::TWO,
        U256::from_u128(342),
    )
    .unwrap();
    assert_eq!(call.entrypoint, "safe_transfer_from");
    assert_eq!(decimals(&call), ["1", "2", "342", "0", "1", "0", "0"]);
}

#[test]
fn a_zero_contract_sender_or_recipient_is_refused() {
    // `nft.test.ts`, "rejects zero contract, sender, or recipient".
    let id = U256::from_u128(1);
    for (contract, from, to) in [
        (Felt::ZERO, Felt::ONE, Felt::TWO),
        (Felt::from(3u8), Felt::ZERO, Felt::TWO),
        (Felt::from(3u8), Felt::ONE, Felt::ZERO),
    ] {
        for standard in [NftStandard::Erc721, NftStandard::Erc1155] {
            let refused = transfer_call(standard, contract, from, to, id).unwrap_err();
            assert_eq!(refused.to_string(), "Invalid NFT transfer address");
        }
    }
}

#[test]
fn a_token_id_is_a_base_units_integer_that_fits_a_u256() {
    // `nft-inputs.ts`'s `isTokenId` (`isBaseUnitsAmount`, at most 78 digits); `cairo.uint256`
    // throws past 2^256 - 1.
    assert_eq!(parse_token_id("0"), Some(U256::ZERO));
    assert_eq!(parse_token_id("342"), Some(U256::from_u128(342)));
    let max = "115792089237316195423570985008687907853269984665640564039457584007913129639935";
    assert_eq!(
        parse_token_id(max),
        Some(U256 {
            high: u128::MAX,
            low: u128::MAX
        })
    );
    let over = "115792089237316195423570985008687907853269984665640564039457584007913129639936";
    for text in [
        "",
        "05",
        "00",
        "-1",
        "+1",
        "1.0",
        "0x5",
        " 5",
        over,
        &"1".repeat(79),
    ] {
        assert_eq!(parse_token_id(text), None, "{text:?}");
    }
}

#[test]
fn an_erc721_send_is_reviewed_as_a_send_of_the_nft() {
    // The recipient reads in decimal, as the extension's live send names it (its calldata is
    // compiled before the review; `extension-nfts.jsonl` `transfer`).
    // `verify-nft-transfer.test.ts`, "decodes transfer_from into a human NFT summary"; the
    // collection raises no `unknown-contract` (`handleReviewNftTransfer` drops it).
    let invoke = prepare(&request(CONTACT, 5), Some(NftStandard::Erc721)).unwrap();
    let [call] = invoke.signed_calls() else {
        panic!("one call");
    };
    assert_eq!(call.entrypoint, "transfer_from");
    assert_eq!(call.calldata[..2], [felt(ACCOUNT), felt(CONTACT)]);
    assert_eq!(invoke.summary(), "Send NFT #5 to 3377621288099693150824796764586984523076137443513370157537360673686401235916");
    assert_eq!(invoke.activity_summary(), "Send NFT #5 to 3377621288099693150824796764586984523076137443513370157537360673686401235916");
    assert!(invoke.flags().is_empty(), "{:?}", invoke.flags());
}

#[test]
fn an_erc1155_send_names_the_edition_count() {
    // `verify-nft-transfer.test.ts`, "decodes the token, amount, and recipient".
    let invoke = prepare(&request(CONTACT, 342), Some(NftStandard::Erc1155)).unwrap();
    assert_eq!(invoke.signed_calls()[0].entrypoint, "safe_transfer_from");
    assert_eq!(invoke.summary(), "Send 1 of NFT #342 to 3377621288099693150824796764586984523076137443513370157537360673686401235916");
    assert!(invoke.flags().is_empty());
}

#[test]
fn a_poisoned_recipient_is_refused_for_either_standard() {
    // `verify-nft-transfer.test.ts`, "flags a poisoned recipient" and "applies the
    // address-poisoning gate to the ERC1155 recipient"; `handleTransferNft` refuses through
    // `refuseOnPoisonedRecipient`, in the shared verifier's sentence.
    for standard in [NftStandard::Erc721, NftStandard::Erc1155] {
        let refused = prepare(&request(POISONED, 5), Some(standard)).unwrap_err();
        assert_eq!(
            refused,
            NftTransferRefusal::Invoke(InvokeRefusal::Review(CallReviewError::PoisonedRecipient))
        );
        assert_eq!(
            refused.to_string(),
            "The recipient closely mimics an address you saved or paid but is a different account. Re-check every character before sending."
        );
    }
}

#[test]
fn an_nft_sent_to_the_account_itself_is_flagged() {
    // `verify-nft-transfer.test.ts`, "flags an NFT sent back to the wallet's own address".
    let invoke = prepare(&request(ACCOUNT, 5), Some(NftStandard::Erc721)).unwrap();
    assert_eq!(invoke.flags(), [&RiskFlag::SelfTransfer]);
}

#[test]
fn the_row_names_the_recipient_and_no_amount_or_token() {
    // `nft-transfer-submit.ts`'s `activityFor`: kind invoke, the review's summary, the recipient
    // as counterparty.
    let invoke = prepare(&request(CONTACT, 5), Some(NftStandard::Erc721)).unwrap();
    assert_eq!(
        invoke.pending_kind(),
        PendingKind::Invoke {
            asset: None,
            counterparty: Some(felt(CONTACT)),
            allowances: Vec::new(),
        }
    );
}

#[test]
fn the_request_is_refused_in_the_extensions_order_and_words() {
    // `readTransferNftInput`: the recipient is a non-zero lowercase-hex felt as typed.
    for recipient in [
        "0x0",
        "0x777AA7890abcdef1234567890abcdef1234567890abcdef1234567890aabbcc",
        " 0x777aa7890abcdef1234567890abcdef1234567890abcdef1234567890aabbcc",
        "777aa7890abcdef1234567890abcdef1234567890abcdef1234567890aabbcc",
        "contact.stark",
    ] {
        let refused = prepare(&request(recipient, 5), Some(NftStandard::Erc721)).unwrap_err();
        assert_eq!(refused.to_string(), "invalid transfer nft request");
    }
    let mut zero = request(CONTACT, 5);
    zero.contract = Felt::ZERO;
    let refused = prepare(&zero, Some(NftStandard::Erc721)).unwrap_err();
    assert_eq!(refused, NftTransferRefusal::InvalidRequest);
    // `prepareNftTransfer`: the account's network, then the ownership read.
    let mut sepolia = request(CONTACT, 5);
    sepolia.network = NetworkId::Sepolia;
    let refused = prepare(&sepolia, None).unwrap_err();
    assert_eq!(refused.to_string(), "switch to the NFT's network first");
    let refused = prepare(&request(CONTACT, 5), None).unwrap_err();
    assert_eq!(refused.to_string(), "this account does not own that NFT");
}

#[test]
fn a_collection_at_the_account_itself_is_refused() {
    // `refuseOnAccountSelfCall`, which every batch takes.
    let mut own = request(CONTACT, 5);
    own.contract = felt(ACCOUNT);
    let refused = prepare(&own, Some(NftStandard::Erc721)).unwrap_err();
    assert_eq!(
        refused,
        NftTransferRefusal::Invoke(InvokeRefusal::Review(CallReviewError::AccountSelfCall))
    );
}
