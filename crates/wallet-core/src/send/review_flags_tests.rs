//! `verifyCalls`'s flags against the extension's own cases (`verify.test.ts`,
//! `verify-token-contract-recipient.test.ts`, `verify-spoof-token.test.ts`, and the root
//! `tests/starknet/verify-{increase-allowance,operator-approval,dapp-avnu-swap}.test.ts`), each
//! named with its source.

use super::*;
use crate::catalog::testing::STRK;
use crate::catalog::TokenInfo;
use crate::kms::NetworkId;
use crate::send::call_decode::decode_call;
use crate::send::types::Authorization;

const ACCOUNT: &str = "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcde";
const ETH: &str = "0x049d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7";
const SPENDER: &str = "0x0555aa7890abcdef1234567890abcdef1234567890abcdef1234567890aabbcc";
const UNKNOWN: &str = "0x6b17d1f2e12c4247f8bce6e563a40f2275d5e88540f0e9899d9ecfeeb8e0aff";
const COLLECTION: &str = "0x03ba1f0dd8b9b6f7c8d1d1d7f0a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4";
const ROUTER: &str = "0x04270219d365d6b017231b52e92b3fb5d7c8378b05e9abc97724537a80e93b0f";
const USDC: &str = "0x033068f6539f8e6e6b131e6b2b814e6c34a5224bc66947c47dab9dfee93b35fb";
const MAX_U128: &str = "0xffffffffffffffffffffffffffffffff";
const HALF: &str = "0x80000000000000000000000000000000";
/// `UNLIMITED_ALLOWANCE_THRESHOLD - 1`.
const JUST_UNDER: &str = "0xfffffffffffffffffffffffffffffffe";

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

fn eth() -> Vec<TokenInfo> {
    vec![TokenInfo {
        address: ETH.into(),
        decimals: 18,
        name: "Ether".into(),
        symbol: "ETH".into(),
    }]
}

fn call(contract: &str, entrypoint: &str, calldata: &[&str]) -> Call {
    Call {
        contract_address: felt(contract),
        entrypoint: entrypoint.into(),
        calldata: calldata.iter().map(|value| felt(value)).collect(),
    }
}

fn increase(amount: &str) -> Call {
    call(ETH, "increase_allowance", &[SPENDER, amount, "0x0"])
}

/// Each call's flags, from `actor`, under `context`.
fn flags_from(actor: &Account, calls: &[Call], context: &ReviewContext) -> Vec<Vec<RiskFlag>> {
    let decoded: Vec<_> = calls
        .iter()
        .map(|c| {
            decode_call(
                &c.contract_address,
                &c.entrypoint,
                &c.calldata,
                &context.known,
            )
        })
        .collect();
    call_flags(calls, &decoded, actor, context)
}

/// Each call's flag codes, under `context`.
fn codes(calls: &[Call], context: &ReviewContext) -> Vec<Vec<&'static str>> {
    flags_from(&account(), calls, context)
        .iter()
        .map(|flags| flags.iter().map(RiskFlag::code).collect())
        .collect()
}

fn all_codes(calls: &[Call], context: &ReviewContext) -> Vec<&'static str> {
    codes(calls, context).concat()
}

const NONE: [&str; 0] = [];

fn with_eth<T>(test: impl FnOnce(&ReviewContext) -> T) -> T {
    let tokens = eth();
    test(&ReviewContext {
        known: Known {
            tokens: &tokens,
            nft_contracts: &[],
        },
        ..ReviewContext::default()
    })
}

#[test]
fn a_known_transfer_raises_nothing() {
    // verify.test.ts, "decodes known ERC20 transfers into a human summary".
    with_eth(|context| {
        let transfer = call(ETH, "transfer", &[SPENDER, "0x14d1120d7b160000", "0x0"]);
        assert_eq!(all_codes(&[transfer], context), NONE);
    });
}

#[test]
fn an_unlimited_approval_on_an_unknown_contract_raises_both() {
    // verify.test.ts, "flags unlimited approvals and unknown contracts".
    with_eth(|context| {
        let approve = call(UNKNOWN, "approve", &["0x999", MAX_U128, MAX_U128]);
        assert_eq!(
            codes(&[approve], context),
            [["unknown-contract", "unlimited-approval"]]
        );
        // "does not read a non-canonical uint256 as an approval amount".
        let wide = Call {
            calldata: vec![Felt::from(0x999u64), Felt::TWO.pow(251u32), Felt::ZERO],
            ..call(ETH, "approve", &[])
        };
        assert_eq!(all_codes(&[wide], context), NONE);
    });
}

#[test]
fn the_unknown_contract_message_names_the_contract() {
    let flag = RiskFlag::UnknownContract {
        contract: felt("0x7aaa00000000000000000000000000000000000000000000000000000000001"),
    };
    assert_eq!(
        flag.message(),
        "0x07aa…0001 is not a token or contact this wallet knows. Verify the contract before continuing."
    );
    assert_eq!(flag.to_string(), flag.message());
}

#[test]
fn distinct_lines_survive_the_dedupe_and_identical_ones_collapse() {
    // verify.test.ts (with addresses inside the field), "keeps a distinct unknown-contract flag for each contract in a multicall"
    // and "still collapses identical risk lines from a multicall".
    let first = RiskFlag::UnknownContract {
        contract: felt("0x7aaa00000000000000000000000000000000000000000000000000000000001"),
    };
    let second = RiskFlag::UnknownContract {
        contract: felt("0x7bbb00000000000000000000000000000000000000000000000000000000002"),
    };
    let flags = [
        first.clone(),
        RiskFlag::UnlimitedApproval,
        second.clone(),
        first.clone(),
        RiskFlag::UnlimitedApproval,
    ];
    assert_eq!(
        dedupe(flags.iter()),
        [&first, &RiskFlag::UnlimitedApproval, &second]
    );
}

#[test]
fn a_payment_to_the_account_itself_is_noted() {
    // verify.test.ts, "flags transfers back to the sender's own account", and the transfer_from
    // cases (the recipient is calldata[1], not the from field).
    with_eth(|context| {
        let transfer = call(ETH, "transfer", &[ACCOUNT, "0x1", "0x0"]);
        assert_eq!(codes(&[transfer], context), [["self-transfer"]]);
        let from = call(ETH, "transferFrom", &[SPENDER, ACCOUNT, "0x1", "0x0"]);
        assert_eq!(codes(&[from], context), [["self-transfer"]]);
        let away = call(ETH, "transfer_from", &[ACCOUNT, SPENDER, "0x1", "0x0"]);
        assert_eq!(all_codes(&[away], context), NONE);
    });
}

#[test]
fn a_payment_to_a_token_contract_is_warned_about() {
    // verify-token-contract-recipient.test.ts: STRK, a curated token and a tracked token, by
    // transfer and transfer_from; an ordinary wallet is not flagged.
    let tokens = eth();
    let contracts = [felt(STRK), felt(USDC)];
    let context = ReviewContext {
        known: Known {
            tokens: &tokens,
            nft_contracts: &[],
        },
        token_contracts: &contracts,
        ..ReviewContext::default()
    };
    for recipient in [STRK, USDC, ETH] {
        let transfer = call(ETH, "transfer", &[recipient, "0x1", "0x0"]);
        assert_eq!(codes(&[transfer], &context), [["token-contract-recipient"]]);
    }
    let from = call(ETH, "transfer_from", &[ACCOUNT, STRK, "0x1", "0x0"]);
    assert_eq!(codes(&[from], &context), [["token-contract-recipient"]]);
    let wallet = call(ETH, "transfer", &[SPENDER, "0x1", "0x0"]);
    assert_eq!(all_codes(&[wallet], &context), NONE);
    // Without the caller's list only the trusted tokens count.
    let bare = ReviewContext {
        token_contracts: &[],
        ..context
    };
    let strk = call(ETH, "transfer", &[STRK, "0x1", "0x0"]);
    assert_eq!(all_codes(&[strk], &bare), NONE);
    let eth = call(ETH, "transfer", &[ETH, "0x1", "0x0"]);
    assert_eq!(all_codes(&[eth], &bare), ["token-contract-recipient"]);
}

#[test]
fn a_token_outside_the_trusted_set_is_an_unknown_contract() {
    // verify-spoof-token.test.ts: the caller leaves a spoof out of `known`, so it stays unknown;
    // a contract the caller pinned (a router, a pool) does not.
    let spoof = "0x00bad00000000000000000000000000000000000000000000000000000000001";
    let transfer = call(spoof, "transfer", &[SPENDER, "0x3b9aca00", "0x0"]);
    with_eth(|context| {
        assert_eq!(
            all_codes(std::slice::from_ref(&transfer), context),
            ["unknown-contract"]
        );
        let pinned = [felt(spoof)];
        let context = ReviewContext {
            known_contracts: &pinned,
            ..*context
        };
        assert_eq!(all_codes(&[transfer], &context), NONE);
    });
}

#[test]
fn operator_grants_are_flagged_and_revocations_are_not() {
    // verify-operator-approval.test.ts.
    let watched = [COLLECTION.to_string()];
    let context = ReviewContext {
        known: Known {
            tokens: &[],
            nft_contracts: &watched,
        },
        ..ReviewContext::default()
    };
    let operator = "0x0111111111111111111111111111111111111111111111111111111111111111";
    for entrypoint in ["set_approval_for_all", "setApprovalForAll"] {
        let grant = call(COLLECTION, entrypoint, &[operator, "0x1"]);
        assert_eq!(
            codes(&[grant], &context),
            [["unknown-contract", "operator-approval"]]
        );
    }
    let revoke = call(COLLECTION, "set_approval_for_all", &[operator, "0x0"]);
    assert_eq!(all_codes(&[revoke], &context), ["unknown-contract"]);
    let folded = call(COLLECTION, "Set_Approval_For_All", &[operator, "0x1"]);
    assert_eq!(all_codes(&[folded], &context), ["unknown-contract"]);
    // A watched collection's item approval is known, and its id is not an amount.
    let item = call(COLLECTION, "approve", &[operator, "0x0", "0x2"]);
    assert_eq!(all_codes(&[item], &context), NONE);
}

#[test]
fn increases_are_judged_on_their_running_total() {
    // verify-increase-allowance.test.ts, the network-free cases.
    with_eth(|context| {
        let high = call(ETH, "increaseAllowance", &[SPENDER, "0x0", "0x1"]);
        assert_eq!(codes(&[high], context), [["unlimited-approval"]]);
        assert_eq!(all_codes(&[increase("0x3e8")], context), NONE);
        // Three halves: the second call completes the grant.
        let split = [increase(HALF), increase(HALF), increase(HALF)];
        assert_eq!(
            codes(&split, context),
            [
                vec![],
                vec!["unlimited-approval"],
                vec!["unlimited-approval"]
            ]
        );
        // Per spender and per token, not across them.
        let other_spender = call(ETH, "increase_allowance", &["0x777", HALF, "0x0"]);
        assert_eq!(all_codes(&[increase(HALF), other_spender], context), NONE);
        // An approve resets the total instead of adding to it.
        let reset = call(ETH, "approve", &[SPENDER, "0x1", "0x0"]);
        assert_eq!(
            all_codes(&[increase(HALF), reset, increase(HALF)], context),
            NONE
        );
        let approve = call(ETH, "approve", &[SPENDER, HALF, "0x0"]);
        assert_eq!(all_codes(&[approve.clone(), approve], context), NONE);
        // A decrease is not a grant.
        let decrease = call(ETH, "decrease_allowance", &[SPENDER, "0x0", "0x1"]);
        assert_eq!(all_codes(&[decrease], context), NONE);
    });
}

fn record(account_id: &str, network: NetworkId, token: &str, amount: &str) -> AllowanceRecord {
    AllowanceRecord {
        id: String::new(),
        account_id: account_id.into(),
        network,
        token: token.into(),
        spender: SPENDER.into(),
        amount: U256 {
            high: 0,
            low: u128::from_str_radix(&amount[2..], 16).unwrap(),
        },
        lower_bound: false,
        revoking: false,
        revoke_hash: None,
    }
}

fn own_id() -> String {
    format!("SN_MAIN:{ACCOUNT}")
}

/// `all_codes` with one standing record.
fn seeded(calls: &[Call], standing: AllowanceRecord, watched: &[String]) -> Vec<&'static str> {
    let tokens = eth();
    let records = [standing];
    let context = ReviewContext {
        known: Known {
            tokens: &tokens,
            nft_contracts: watched,
        },
        standing: &records,
        ..ReviewContext::default()
    };
    all_codes(calls, &context)
}

#[test]
fn the_wallets_own_standing_allowance_seeds_the_total() {
    // verify-increase-allowance.test.ts, "standing allowances seed the unlimited-approval total".
    let standing = || record(&own_id(), NetworkId::Mainnet, ETH, JUST_UNDER);
    assert_eq!(
        seeded(&[increase("0x1")], standing(), &[]),
        ["unlimited-approval"]
    );
    // Another spelling of the same account and token is the same pair.
    let padded = record(
        "SN_MAIN:0x01234567890abcdef1234567890abcdef1234567890abcdef1234567890abcde",
        NetworkId::Mainnet,
        "0x49d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7",
        JUST_UNDER,
    );
    assert_eq!(
        seeded(&[increase("0x1")], padded, &[]),
        ["unlimited-approval"]
    );
    // Another account's, another network's, an untrusted contract's or a watched collection's
    // record seeds nothing.
    let other = record("SN_MAIN:0x9999", NetworkId::Mainnet, ETH, JUST_UNDER);
    assert_eq!(seeded(&[increase("0x1")], other, &[]), NONE);
    let sepolia = record(&own_id(), NetworkId::Sepolia, ETH, JUST_UNDER);
    assert_eq!(seeded(&[increase("0x1")], sepolia, &[]), NONE);
    let unknown = record(&own_id(), NetworkId::Mainnet, "0xabc123", JUST_UNDER);
    let on_unknown = call("0xabc123", "increase_allowance", &[SPENDER, "0x1", "0x0"]);
    assert_eq!(seeded(&[on_unknown], unknown, &[]), ["unknown-contract"]);
    let watched = [ETH.to_string()];
    assert_eq!(seeded(&[increase("0x1")], standing(), &watched), NONE);
}

#[test]
fn a_decrease_lowers_the_seed_and_an_approve_replaces_it() {
    // verify-increase-allowance.test.ts, the decrease and replace cases.
    let standing = || record(&own_id(), NetworkId::Mainnet, ETH, JUST_UNDER);
    for entrypoint in ["decrease_allowance", "decreaseAllowance"] {
        let decrease = call(ETH, entrypoint, &[SPENDER, JUST_UNDER, "0x0"]);
        assert_eq!(seeded(&[decrease, increase("0x1")], standing(), &[]), NONE);
    }
    let one = call(ETH, "decrease_allowance", &[SPENDER, "0x1", "0x0"]);
    assert_eq!(
        seeded(&[one, increase("0x2")], standing(), &[]),
        ["unlimited-approval"]
    );
    // A decrease past the standing amount clamps at zero.
    let past = call(ETH, "decrease_allowance", &[SPENDER, "0x0", "0x1"]);
    assert_eq!(seeded(&[past, increase("0x1")], standing(), &[]), NONE);
    let alone = call(ETH, "decrease_allowance", &[SPENDER, "0x1", "0x0"]);
    assert_eq!(seeded(&[alone], standing(), &[]), NONE);
    let approve = call(ETH, "approve", &[SPENDER, "0x1", "0x0"]);
    assert_eq!(seeded(&[approve], standing(), &[]), NONE);
}

#[test]
fn a_total_past_uint256_saturates_instead_of_dropping_the_flag() {
    let full = AllowanceRecord {
        amount: U256 {
            high: u128::MAX,
            low: u128::MAX,
        },
        ..record(&own_id(), NetworkId::Mainnet, ETH, "0x0")
    };
    assert_eq!(
        seeded(&[increase("0x1")], full.clone(), &[]),
        ["unlimited-approval"]
    );
    // A decrease that leaves the total past the threshold is still not a grant.
    let decrease = call(ETH, "decrease_allowance", &[SPENDER, "0x1", "0x0"]);
    assert_eq!(seeded(&[decrease], full, &[]), NONE);
    // Past UINT256_MAX the total stays there through a decrease: the extension's bigint holds
    // 2^257 - 2, less 2^256 - 1, plus one, and flags the last call; a capped total read it as one.
    let max = call(ETH, "increase_allowance", &[SPENDER, MAX_U128, MAX_U128]);
    let down = call(ETH, "decrease_allowance", &[SPENDER, MAX_U128, MAX_U128]);
    with_eth(|context| {
        let calls = [max.clone(), max, down, increase("0x1")];
        let unlimited = vec!["unlimited-approval"];
        assert_eq!(
            codes(&calls, context),
            [unlimited.clone(), unlimited.clone(), vec![], unlimited]
        );
    });
}

/// `avnuRouteCalldata`'s header (sell, its amount, buy, its quote, the floor, the beneficiary, the
/// integrator fee and its recipient, one route), then one leg with no parameters.
fn swap(beneficiary: &str) -> Call {
    let header = [
        USDC,
        "0x1dcd6500",
        "0x0",
        ETH,
        "0x2c68af0bb140000",
        "0x0",
        "0x2a303fe4b530000",
        "0x0",
        beneficiary,
        "0x0",
        "0x0",
        "0x1",
    ];
    let route = [USDC, ETH, "0x1", "0x0", "0x0"];
    call(
        ROUTER,
        "multi_route_swap",
        &[&header[..], &route[..]].concat(),
    )
}

#[test]
fn a_swap_is_warned_about_when_it_pays_elsewhere_or_cannot_be_read() {
    // verify-dapp-avnu-swap.test.ts. The router is pinned, as swap-ops.ts pins it.
    let pinned = [felt(ROUTER)];
    let tokens = eth();
    let context = ReviewContext {
        known: Known {
            tokens: &tokens,
            nft_contracts: &[],
        },
        known_contracts: &pinned,
        ..ReviewContext::default()
    };
    // Into the account: no warning, and no self-transfer note.
    let own = swap(ACCOUNT);
    let decoded = decode_call(
        &own.contract_address,
        &own.entrypoint,
        &own.calldata,
        &context.known,
    );
    assert_eq!(decoded.kind, CallKind::Swap);
    assert_eq!(all_codes(&[own], &context), NONE);
    let elsewhere = flags_from(&account(), &[swap(SPENDER)], &context);
    assert_eq!(
        elsewhere[0].iter().map(RiskFlag::message).collect::<Vec<_>>(),
        ["This swap pays its proceeds to 0x0555…bbcc, not to this account. Reject it unless you meant to send them there."]
    );
    // "malformed calldata falls back to the opaque line, still shows the call, and is flagged".
    let mut truncated = swap(SPENDER);
    truncated.calldata.truncate(11);
    assert_eq!(
        codes(&[truncated], &context),
        [["undecoded-swap", "foreign-beneficiary"]]
    );
    let mut stub = swap(SPENDER);
    stub.calldata.truncate(3);
    assert_eq!(codes(&[stub], &context), [["undecoded-swap"]]);
    // On the swap selector the payee rules are the swap's: a token contract as beneficiary is
    // foreign, not a token-contract recipient.
    assert_eq!(codes(&[swap(ETH)], &context), [["foreign-beneficiary"]]);
    // Unpinned, the router is an unknown contract.
    assert_eq!(
        all_codes(
            &[swap(ACCOUNT)],
            &ReviewContext {
                known_contracts: &[],
                ..context
            }
        ),
        ["unknown-contract"]
    );
}
