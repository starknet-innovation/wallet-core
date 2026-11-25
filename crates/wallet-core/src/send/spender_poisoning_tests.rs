//! `verify-spender-poisoning.test.ts` (Bramble #1038, head `52013b8f`), through the one refusal
//! every batch takes (`refuse_poisoned`). Calldata is felts here, so the extension's decimal
//! spellings of a spender are the same case as their hex.

use super::*;
use crate::catalog::TokenInfo;
use crate::send::call_decode::spender_felt_index;

const ACCOUNT: &str = "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcde";
const ETH: &str = "0x049d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7";
const NFT: &str = "0x0511a2b3c4d5e6f7a8b9c0d1e2f30411223344556677889900aabbccddeeff00";
const UNKNOWN: &str = "0x0456000000000000000000000000000000000000000000000000000000000456";
const ROUTER: &str = "0x04270219d365d6b017231b52e92b3fb5d7c8378b05e9abc97724537a80e93b0f";
const CONTACT: &str = "0x777aa7890abcdef1234567890abcdef1234567890abcdef1234567890aabbcc";
// Same visible `0x` + four and last four as the contact / the account.
const CONTACT_TWIN: &str = "0x777a9999999999999999999999999999999999999999999999999999999bbcc";
const ACCOUNT_TWIN: &str = "0x1234000000000000000000000000000000000000000000000000000000bcde";

fn felt(value: &str) -> Felt {
    Felt::from_hex(value).unwrap()
}

fn call(contract: &str, entrypoint: &str, calldata: &[&str]) -> Call {
    Call {
        contract_address: felt(contract),
        entrypoint: entrypoint.into(),
        calldata: calldata.iter().map(|value| felt(value)).collect(),
    }
}

fn refused(calls: &[Call]) -> Result<(), CallReviewError> {
    let tokens = [TokenInfo {
        address: ETH.into(),
        decimals: 18,
        name: "Ether".into(),
        symbol: "ETH".into(),
    }];
    let nft_contracts = [NFT.to_string()];
    let known = Known {
        tokens: &tokens,
        nft_contracts: &nft_contracts,
    };
    refuse_poisoned(&felt(ACCOUNT), calls, &known, &[felt(CONTACT)])
}

const SPENDER: Result<(), CallReviewError> = Err(CallReviewError::PoisonedSpender);

#[test]
fn spender_felt_index_reads_felt_zero_for_every_grant_spelling_and_nothing_else() {
    for entrypoint in [
        "approve",
        "increase_allowance",
        "increaseAllowance",
        "set_approval_for_all",
        "setApprovalForAll",
    ] {
        assert_eq!(spender_felt_index(entrypoint), Some(0), "{entrypoint}");
    }
    for entrypoint in ["transfer", "decrease_allowance", "Approve", "transfer_from"] {
        assert_eq!(spender_felt_index(entrypoint), None, "{entrypoint}");
    }
}

#[test]
fn a_lookalike_spender_is_refused_with_the_spender_sentence() {
    assert_eq!(
        refused(&[call(ETH, "approve", &[CONTACT_TWIN, "0x64", "0x0"])]),
        SPENDER
    );
    assert_eq!(
        SPENDER.unwrap_err().to_string(),
        "The address this allows closely mimics an address you saved or paid but is a different \
         account. Re-check every character before approving."
    );
    // Both increase spellings, an increase by zero included.
    for entrypoint in ["increase_allowance", "increaseAllowance"] {
        assert_eq!(
            refused(&[call(ETH, entrypoint, &[CONTACT_TWIN, "0x5", "0x0"])]),
            SPENDER
        );
        assert_eq!(
            refused(&[call(ETH, entrypoint, &[CONTACT_TWIN, "0x0", "0x0"])]),
            SPENDER
        );
    }
    // A twin of the account itself.
    assert_eq!(
        refused(&[call(ETH, "approve", &[ACCOUNT_TWIN, "0x1", "0x0"])]),
        SPENDER
    );
    // ERC-721 approve, item #0 included, and both operator spellings.
    assert_eq!(
        refused(&[call(NFT, "approve", &[CONTACT_TWIN, "0x7", "0x0"])]),
        SPENDER
    );
    assert_eq!(
        refused(&[call(NFT, "approve", &[CONTACT_TWIN, "0x0", "0x0"])]),
        SPENDER
    );
    for entrypoint in ["set_approval_for_all", "setApprovalForAll"] {
        assert_eq!(
            refused(&[call(NFT, entrypoint, &[CONTACT_TWIN, "0x1"])]),
            SPENDER
        );
    }
    // A zero approve on a contract that is not a tracked token may be ERC-721 item #0.
    assert_eq!(
        refused(&[call(UNKNOWN, "approve", &[CONTACT_TWIN, "0x0", "0x0"])]),
        SPENDER
    );
    // A span the decoder cannot describe is still checked.
    assert_eq!(
        refused(&[call(ETH, "approve", &[CONTACT_TWIN, "0x1", "0x0", "0x0"])]),
        SPENDER
    );
    assert_eq!(
        refused(&[call(NFT, "set_approval_for_all", &[CONTACT_TWIN])]),
        SPENDER
    );
    // One lookalike grant in a multicall refuses the whole batch.
    let batch = [
        call(ETH, "approve", &[ROUTER, "0x1", "0x0"]),
        call(ETH, "approve", &[CONTACT_TWIN, "0x1", "0x0"]),
    ];
    assert_eq!(refused(&batch), SPENDER);
}

#[test]
fn grants_that_are_not_poisoned_are_unchanged() {
    for spender in [CONTACT, ACCOUNT, ROUTER, ETH] {
        assert_eq!(
            refused(&[call(ETH, "approve", &[spender, "0x1", "0x0"])]),
            Ok(())
        );
    }
}

// Bramble #1038's exemption: refusing these would leave a poisoned grant unrevokable.
#[test]
fn revoking_a_grant_to_a_lookalike_still_works() {
    assert_eq!(
        refused(&[call(ETH, "approve", &[CONTACT_TWIN, "0x0", "0x0"])]),
        Ok(())
    );
    for entrypoint in ["set_approval_for_all", "setApprovalForAll"] {
        assert_eq!(
            refused(&[call(NFT, entrypoint, &[CONTACT_TWIN, "0x0"])]),
            Ok(())
        );
    }
    assert_eq!(
        refused(&[call(
            ETH,
            "decrease_allowance",
            &[CONTACT_TWIN, "0x5", "0x0"]
        )]),
        Ok(())
    );
}
