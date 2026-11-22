// `allowances.test.ts` (the package's `tests/allowances/`) and `allowance-recording.test.ts` (the
// extension's root `tests/background/`), over the log on in-memory storage, plus the guards they
// leave out.

use super::calls::*;
use super::record::AllowanceRecord;
use crate::catalog::TokenInfo;
use crate::send::call_decode::{call_summary, Known};
use crate::send::calls::Call;
use crate::send::u256::U256;
use crate::{Felt, NetworkId};

pub(super) const ADDRESS: &str =
    "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcde";
pub(super) const ACCOUNT: &str =
    "SN_MAIN:0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcde";
pub(super) const TOKEN: &str = "0x049d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7";
pub(super) const CANONICAL_TOKEN: &str =
    "0x49d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7";
pub(super) const SPENDER: &str =
    "0x0555aa7890abcdef1234567890abcdef1234567890abcdef1234567890aabbcc";
pub(super) const CANONICAL_SPENDER: &str =
    "0x555aa7890abcdef1234567890abcdef1234567890abcdef1234567890aabbcc";
// `0x0299${"8".repeat(59)}4` and `0x0777${"7".repeat(59)}1`.
pub(super) const COLLECTION: &str =
    "0x0299888888888888888888888888888888888888888888888888888888888884";
pub(super) const UNKNOWN: &str =
    "0x0777777777777777777777777777777777777777777777777777777777777771";

pub(super) fn f(hex: &str) -> Felt {
    Felt::from_hex(hex).unwrap()
}

pub(super) fn grant(
    contract: &str,
    entrypoint: &str,
    spender: &str,
    low: u128,
    high: u128,
) -> Call {
    Call {
        contract_address: f(contract),
        entrypoint: entrypoint.into(),
        calldata: vec![f(spender), Felt::from(low), Felt::from(high)],
    }
}

pub(super) fn owner() -> AllowanceOwner {
    AllowanceOwner::of(NetworkId::Mainnet, &f(ADDRESS)).unwrap()
}

/// What the account trusts: tokens (named ETH) and watched collections.
pub(super) struct Lists {
    tokens: Vec<TokenInfo>,
    nfts: Vec<String>,
}

impl Lists {
    pub(super) fn new(tokens: &[&str], nfts: &[&str]) -> Self {
        let token = |address: &str| TokenInfo {
            address: address.into(),
            decimals: 18,
            name: "Ether".into(),
            symbol: "ETH".into(),
        };
        Self {
            tokens: tokens.iter().map(|address| token(address)).collect(),
            nfts: nfts.iter().map(|nft| nft.to_string()).collect(),
        }
    }

    /// The extension tests' ETH and watched collection.
    pub(super) fn standard() -> Self {
        Self::new(&[TOKEN], &[COLLECTION])
    }

    pub(super) fn known(&self) -> Known<'_> {
        Known {
            tokens: &self.tokens,
            nft_contracts: &self.nfts,
        }
    }
}

pub(super) fn amount(text: &str) -> U256 {
    U256::from_decimal(text).unwrap()
}

#[test]
fn owners_are_bound_to_their_network() {
    assert_eq!(owner().account_id, ACCOUNT);
    assert_eq!(owner().network, NetworkId::Mainnet);
    let sepolia = AllowanceOwner::of(NetworkId::Sepolia, &f(ADDRESS)).unwrap();
    assert!(sepolia.account_id.starts_with("SN_SEPOLIA:"));
    assert_eq!(AllowanceOwner::of(NetworkId::Mainnet, &Felt::ZERO), None);
}

#[test]
fn decodes_an_approve_and_combines_the_limbs() {
    // "decodes an approve into spender, uint256 amount, and an absolute kind".
    let decoded = decode_allowance_call(&grant(TOKEN, "approve", "0x5", 1000, 0)).unwrap();
    assert_eq!(
        decoded,
        DecodedAllowanceCall {
            kind: ChangeKind::Set,
            spender: f("0x5"),
            amount: amount("1000"),
        }
    );
    // "combines uint256 high and low limbs".
    let decoded = decode_allowance_call(&grant(TOKEN, "approve", "0x5", 0, 1)).unwrap();
    assert_eq!(decoded.amount, U256 { high: 1, low: 0 });
}

#[test]
fn decodes_both_spellings_of_increase_and_decrease() {
    for (entrypoint, kind) in [
        ("increase_allowance", ChangeKind::Increase),
        ("increaseAllowance", ChangeKind::Increase),
        ("decrease_allowance", ChangeKind::Decrease),
        ("decreaseAllowance", ChangeKind::Decrease),
    ] {
        let decoded = decode_allowance_call(&grant(TOKEN, entrypoint, "0x5", 7, 0)).unwrap();
        assert_eq!((decoded.kind, decoded.amount), (kind, amount("7")));
    }
}

#[test]
fn ignores_non_allowance_case_variant_and_malformed_calls() {
    for entrypoint in [
        "transfer",
        "Approve",
        "INCREASE_ALLOWANCE",
        "increase_Allowance",
    ] {
        assert_eq!(
            decode_allowance_call(&grant(TOKEN, entrypoint, "0x5", 1, 0)),
            None
        );
    }
    let mut short = grant(TOKEN, "approve", "0x5", 1, 0);
    short.calldata.truncate(1);
    assert_eq!(decode_allowance_call(&short), None);
    short.calldata.clear();
    assert_eq!(decode_allowance_call(&short), None);
    // Beyond the extension's tests: a limb at 2^128 is not a uint256 half.
    let mut wide = grant(TOKEN, "approve", "0x5", 1, 0);
    wide.calldata[2] = f("0x100000000000000000000000000000000");
    assert_eq!(decode_allowance_call(&wide), None);
}

#[test]
fn every_grant_spelling_is_one_the_reviewer_summarises_as_a_grant() {
    // The bookkeeping and `call_decode`'s vocabulary must agree on what grants spend.
    let lists = Lists::standard();
    for entrypoint in ["approve", "increaseAllowance", "increase_allowance"] {
        let call = grant(TOKEN, entrypoint, "0x5", 1, 0);
        assert!(decode_allowance_call(&call).is_some());
        let summary = call_summary(
            &call.contract_address,
            entrypoint,
            &call.calldata,
            &lists.known(),
        );
        assert!(summary.starts_with("Allow "), "{entrypoint}: {summary}");
    }
}

#[test]
fn maps_allowance_calls_to_inputs_scoped_to_the_granting_owner() {
    let lists = Lists::standard();
    let changes = allowance_changes(
        &[
            grant(TOKEN, "approve", "0x5", 1000, 0),
            grant(TOKEN, "transfer", "0x9", 1, 0),
            grant(TOKEN, "increase_allowance", "0x9", 4, 0),
        ],
        &owner(),
        &lists.known(),
    );
    let shape: Vec<_> = changes
        .iter()
        .map(|c| {
            let i = &c.input;
            let at = (i.account_id.as_str(), i.network, i.token.as_str());
            (c.kind, at, i.spender.as_str(), i.amount, i.lower_bound)
        })
        .collect();
    let scope = (ACCOUNT, NetworkId::Mainnet, CANONICAL_TOKEN);
    assert_eq!(
        shape,
        [
            (ChangeKind::Set, scope, "0x5", amount("1000"), false),
            (ChangeKind::Increase, scope, "0x9", amount("4"), false),
        ]
    );
}

#[test]
fn drops_a_zero_delta_but_keeps_a_zero_approve() {
    let lists = Lists::standard();
    let zero_deltas = allowance_changes(
        &[
            grant(TOKEN, "increase_allowance", "0x5", 0, 0),
            grant(TOKEN, "decrease_allowance", "0x5", 0, 0),
        ],
        &owner(),
        &lists.known(),
    );
    assert!(zero_deltas.is_empty());
    let revoke = allowance_changes(
        &[grant(TOKEN, "approve", "0x5", 0, 0)],
        &owner(),
        &lists.known(),
    );
    assert_eq!(
        revoke.iter().map(|c| c.kind).collect::<Vec<_>>(),
        [ChangeKind::Set]
    );
}

#[test]
fn classifies_the_approved_contract_from_the_accounts_own_lists() {
    let lists = Lists::standard();
    let kind = |contract: &str| approved_asset_kind(&lists.known(), &f(contract));
    assert_eq!(kind(TOKEN), AssetKind::Fungible);
    assert_eq!(kind(CANONICAL_TOKEN), AssetKind::Fungible);
    assert_eq!(kind(COLLECTION), AssetKind::Collection);
    assert_eq!(kind(UNKNOWN), AssetKind::Unknown);
    // No lists at all is unknown, never fungible.
    assert_eq!(
        approved_asset_kind(&Known::default(), &f(TOKEN)),
        AssetKind::Unknown
    );
    // Beyond the extension's tests: a contract in both lists is a collection (checked first).
    let both = Lists::new(&[COLLECTION], &[COLLECTION]);
    assert_eq!(
        approved_asset_kind(&both.known(), &f(COLLECTION)),
        AssetKind::Collection
    );
}

#[test]
fn files_no_allowance_change_for_a_watched_collection() {
    let calls = [
        grant(COLLECTION, "approve", "0x5", 7, 0),
        grant(TOKEN, "approve", "0x5", 7, 0),
    ];
    let tokens = |lists: &Lists| {
        allowance_changes(&calls, &owner(), &lists.known())
            .into_iter()
            .map(|c| c.input.token)
            .collect::<Vec<_>>()
    };
    assert_eq!(tokens(&Lists::standard()), [CANONICAL_TOKEN]);
    // Without the watch entry the two cannot be told apart, so both are filed.
    assert_eq!(tokens(&Lists::new(&[TOKEN], &[])).len(), 2);
}

// --- `standingAllowanceTotals` ---

pub(super) fn stored(
    account_id: &str,
    network: NetworkId,
    token: &str,
    amount: U256,
) -> AllowanceRecord {
    AllowanceRecord {
        id: format!("{account_id}:{token}:0x5"),
        account_id: account_id.into(),
        network,
        token: token.into(),
        spender: "0x5".into(),
        amount,
        lower_bound: false,
        revoking: false,
        revoke_hash: None,
    }
}

#[test]
fn standing_totals_seed_only_the_owners_trusted_tokens() {
    let other = "SN_MAIN:0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdf";
    let mut bound = stored(ACCOUNT, NetworkId::Mainnet, CANONICAL_TOKEN, amount("9"));
    bound.lower_bound = true;
    let records = [
        bound,
        stored(other, NetworkId::Mainnet, "0x99", amount("1")),
        // An inconsistent record: the owner's id on another network.
        stored(ACCOUNT, NetworkId::Sepolia, "0x98", amount("1")),
        stored(ACCOUNT, NetworkId::Mainnet, COLLECTION, amount("1")),
        stored(ACCOUNT, NetworkId::Mainnet, UNKNOWN, amount("1")),
    ];
    // A watched collection is not seeded even when it is also a trusted token.
    let wide = Lists::new(&[TOKEN, "0x99", "0x98", UNKNOWN, COLLECTION], &[COLLECTION]);
    let mut totals: Vec<_> = standing_allowance_totals(&records, &owner(), &wide.known())
        .into_iter()
        .collect();
    totals.sort();
    assert_eq!(
        totals,
        [
            ((f(TOKEN), f("0x5")), amount("9")),
            ((f(UNKNOWN), f("0x5")), amount("1")),
        ]
    );
    // With only ETH trusted, the other contract is not seeded.
    let lists = Lists::standard();
    assert_eq!(
        standing_allowance_totals(&records, &owner(), &lists.known()).len(),
        1
    );
}
