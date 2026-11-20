//! One case per decoder in `verify-decode.ts`, including the shapes that fall back to the opaque
//! line. The extension's own transfer summary is pinned by the multisig flows fixture.

use super::*;

const CONTRACT: u64 = 0x1234_5678_9abc;

fn felts(values: &[u64]) -> Vec<Felt> {
    values.iter().map(|value| Felt::from(*value)).collect()
}

fn summary(entrypoint: &str, calldata: &[u64], known: &Known) -> String {
    call_summary(&Felt::from(CONTRACT), entrypoint, &felts(calldata), known)
}

fn token() -> Vec<TokenInfo> {
    vec![TokenInfo {
        address: format!("0x{CONTRACT:064x}"),
        decimals: 2,
        name: "Test".into(),
        symbol: "TST".into(),
    }]
}

const OPAQUE: &str = "Call {} on 0x1234…9abc";

fn opaque(entrypoint: &str) -> String {
    OPAQUE.replace("{}", entrypoint)
}

#[test]
fn transfers_and_allowances_name_the_token_when_it_is_known() {
    let tokens = token();
    let known = Known {
        tokens: &tokens,
        nft_contracts: &[],
    };
    let none = Known::default();
    assert_eq!(
        summary("transfer", &[0x77, 150, 0], &known),
        "Send 1.5 TST to 0x77…0x77"
    );
    assert_eq!(
        summary("transfer", &[0x77, 150, 0], &none),
        "Send 150 units to 0x77…0x77"
    );
    assert_eq!(
        summary("approve", &[0x77, 100, 0], &known),
        "Allow 0x77…0x77 to spend 1 TST"
    );
    assert_eq!(
        summary("increase_allowance", &[0x77, 100, 0], &known),
        "Allow 0x77…0x77 to spend 1 TST more"
    );
    assert_eq!(
        summary("increaseAllowance", &[0x77, 5, 0], &none),
        "Authorize 0x77…0x77 for amount or NFT #5 on unknown contract 0x1234…9abc"
    );
    let watched = [format!("0x{CONTRACT:x}")];
    let nft = Known {
        tokens: &[],
        nft_contracts: &watched,
    };
    assert_eq!(
        summary("approve", &[0x77, 5, 0], &nft),
        "Allow 0x77…0x77 to transfer NFT #5 of collection 0x1234…9abc"
    );
    // Wrong arity or a limb at 2^128 is opaque.
    assert_eq!(summary("transfer", &[0x77, 1], &known), opaque("transfer"));
    let wide = [
        Felt::from(0x77u8),
        Felt::from(u128::MAX) + Felt::ONE,
        Felt::ZERO,
    ];
    assert_eq!(
        call_summary(&Felt::from(CONTRACT), "transfer", &wide, &known),
        opaque("transfer")
    );
    // Selectors match exactly.
    assert_eq!(
        summary("Transfer", &[0x77, 1, 0], &known),
        opaque("Transfer")
    );
    assert_eq!(
        summary("decrease_allowance", &[0x77, 1, 0], &known),
        opaque("decrease_allowance")
    );
}

#[test]
fn transfer_from_reads_an_amount_for_a_token_and_an_id_otherwise() {
    let tokens = token();
    let known = Known {
        tokens: &tokens,
        nft_contracts: &[],
    };
    assert_eq!(
        summary("transfer_from", &[0x55, 0x77, 250, 0], &known),
        "Send 2.5 TST from 0x55…0x55 to 0x77…0x77"
    );
    assert_eq!(
        summary("transferFrom", &[0x55, 0x77, 9, 0], &Known::default()),
        "Send NFT #9 to 0x77…0x77"
    );
    assert_eq!(
        summary("transfer_from", &[0x55, 0x77, 9], &known),
        opaque("transfer_from")
    );
}

#[test]
fn safe_transfers_state_no_quantity_they_cannot_attribute() {
    let none = Known::default();
    // ERC721 with an empty data span.
    assert_eq!(
        summary("safe_transfer_from", &[0x55, 0x77, 9, 0, 0], &none),
        "Send NFT #9 to 0x77…0x77"
    );
    // ERC1155 with an empty data span.
    assert_eq!(
        summary("safeTransferFrom", &[0x55, 0x77, 9, 0, 3, 0, 0], &none),
        "Send 3 of NFT #9 to 0x77…0x77"
    );
    // Fits both readings: no count.
    assert_eq!(
        summary("safe_transfer_from", &[0x55, 0x77, 9, 0, 2, 4, 0], &none),
        "Safe transfer of NFT #9 to 0x77…0x77"
    );
    assert_eq!(
        summary("safe_transfer_from", &[0x55, 0x77, 9, 0, 5], &none),
        opaque("safe_transfer_from")
    );
}

#[test]
fn batch_transfers_walk_the_whole_span() {
    let none = Known::default();
    let line = |calldata: &[u64]| summary("safe_batch_transfer_from", calldata, &none);
    assert_eq!(
        line(&[0x55, 0x77, 1, 9, 0, 1, 3, 0, 0]),
        "Send 1 token id of collection 0x1234…9abc to 0x77…0x77"
    );
    assert_eq!(
        summary(
            "safeBatchTransferFrom",
            &[0x55, 0x77, 2, 9, 0, 8, 0, 2, 3, 0, 4, 0, 1, 7],
            &none
        ),
        "Send 2 token ids of collection 0x1234…9abc to 0x77…0x77"
    );
    // Mismatched counts, a trailing felt, or an id count past the span.
    assert_eq!(
        line(&[0x55, 0x77, 1, 9, 0, 2, 3, 0, 0]),
        opaque("safe_batch_transfer_from")
    );
    assert_eq!(
        line(&[0x55, 0x77, 1, 9, 0, 1, 3, 0, 0, 0]),
        opaque("safe_batch_transfer_from")
    );
    assert_eq!(
        line(&[0x55, 0x77, 9, 0, 0]),
        opaque("safe_batch_transfer_from")
    );
}

#[test]
fn operator_approvals_and_swaps() {
    let none = Known::default();
    assert_eq!(
        summary("set_approval_for_all", &[0x77, 1], &none),
        "Allow 0x77…0x77 to transfer EVERY item you own in collection 0x1234…9abc"
    );
    assert_eq!(
        summary("setApprovalForAll", &[0x77, 0], &none),
        "Revoke 0x77…0x77's permission to transfer every item in collection 0x1234…9abc"
    );
    assert_eq!(
        summary("setApprovalForAll", &[0x77], &none),
        opaque("setApprovalForAll")
    );
    let swap = [1, 5, 0, 2, 9, 0, 8, 0, 3, 0, 4, 1, 6, 7, 10, 11, 0];
    assert_eq!(
        summary(AVNU_MULTI_ROUTE_SWAP, &swap, &none),
        "Swap 5 units of 0x1…0x1 for at least 8 units of 0x2…0x2, paid to 0x3…0x3"
    );
    assert_eq!(
        summary(AVNU_MULTI_ROUTE_SWAP, &swap[..16], &none),
        opaque(AVNU_MULTI_ROUTE_SWAP)
    );
}

#[test]
fn recipients_come_from_the_selector_alone() {
    let calldata = felts(&[1, 2, 3, 4, 5, 6, 7, 8, 9]);
    assert_eq!(recipient_for_flags("transfer", &calldata), Some(Felt::ONE));
    for name in [
        "transfer_from",
        "safeTransferFrom",
        "safe_batch_transfer_from",
    ] {
        assert_eq!(recipient_for_flags(name, &calldata), Some(Felt::from(2u8)));
    }
    assert_eq!(
        recipient_for_flags(AVNU_MULTI_ROUTE_SWAP, &calldata),
        Some(Felt::from(9u8))
    );
    assert_eq!(recipient_for_flags("approve", &calldata), None);
    assert_eq!(
        recipient_for_flags(AVNU_MULTI_ROUTE_SWAP, &calldata[..8]),
        None
    );
}
