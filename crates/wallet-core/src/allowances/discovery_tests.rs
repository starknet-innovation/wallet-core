// `allowance-discovery.test.ts`, `allowance-revoke-hash.test.ts`'s view cases and the listing
// rules of `allowance-ops.ts`, plus the branches they leave out.

use super::discovery::*;
use super::record::AllowanceRecord;
use super::tracked::ALLOWANCE_SELECTOR;
use crate::js::json::JsValue;
use crate::send::u256::U256;
use crate::NetworkId;

const ACCOUNT: &str = "SN_MAIN:0x1234567890abcdef";
const OWNER: &str = "0x1234567890abcdef";
const TOKEN: &str = "0x049d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7";
const CANONICAL_TOKEN: &str = "0x49d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7";
const SPENDER: &str = "0x5abcdef";
const OTHER_SPENDER: &str = "0x7feed";
// 2^128.
const UNLIMITED: U256 = U256 { high: 1, low: 0 };

fn krusty_selector(name: &str) -> String {
    crate::felt::canonical(&crate::kms::selector(name))
}

#[test]
fn selectors_are_starknet_keccak() {
    assert_eq!(krusty_selector("Approval"), APPROVAL_EVENT_KEY);
    assert_eq!(krusty_selector("allowance"), ALLOWANCE_SELECTOR);
}

fn strings(values: &[&str]) -> Vec<JsValue> {
    values
        .iter()
        .map(|v| JsValue::String((*v).into()))
        .collect()
}

fn amount(value: u128) -> U256 {
    U256::from_u128(value)
}

fn wallet_record() -> AllowanceRecord {
    AllowanceRecord {
        id: format!("{ACCOUNT}:{CANONICAL_TOKEN}:{SPENDER}"),
        account_id: ACCOUNT.into(),
        network: NetworkId::Mainnet,
        token: CANONICAL_TOKEN.into(),
        spender: SPENDER.into(),
        amount: amount(1000),
        lower_bound: false,
        revoking: false,
        revoke_hash: None,
    }
}

fn external(spender: &str, value: U256) -> ExternalAllowance {
    ExternalAllowance {
        token: TOKEN.into(),
        spender: spender.into(),
        amount: value,
    }
}

fn merge(
    in_wallet: &[AllowanceRecord],
    found: &[ExternalAllowance],
    multisig: &[AllowanceRecord],
) -> Vec<AllowanceView> {
    merge_allowance_views(ACCOUNT, NetworkId::Mainnet, in_wallet, multisig, found, &[])
}

#[test]
fn approval_events_decode_in_both_layouts() {
    let key = APPROVAL_EVENT_KEY;
    let expected = Some((OWNER.to_string(), SPENDER.to_string()));
    // "decodes the Cairo 1 layout with owner and spender as indexed keys".
    let keys = strings(&[key, OWNER, SPENDER]);
    assert_eq!(
        decode_approval_event(&keys, &strings(&["0x64", "0x0"])),
        expected
    );
    // "decodes the legacy layout with owner and spender in the data payload".
    let data = strings(&[OWNER, SPENDER, "0x64", "0x0"]);
    assert_eq!(decode_approval_event(&strings(&[key]), &data), expected);
    // "rejects an event whose first key is not the Approval selector".
    let other = strings(&["0x1", OWNER, SPENDER]);
    assert_eq!(decode_approval_event(&other, &[]), None);
    // "rejects a zero owner or spender".
    let zero_owner = strings(&[key, "0x0", SPENDER]);
    assert_eq!(decode_approval_event(&zero_owner, &[]), None);
    let zero_spender = strings(&[OWNER, "0x0", "0x64"]);
    assert_eq!(decode_approval_event(&strings(&[key]), &zero_spender), None);
    // Beyond the tests: a padded or decimal spelling is canonicalised, a non-string or missing
    // key is no approval, and a number past the field is refused.
    let padded_owner = "0x01234567890ABCDEF";
    let decimal = strings(&[key, padded_owner, "95145455"]);
    assert_eq!(decode_approval_event(&decimal, &[]), expected);
    let numeric = vec![
        JsValue::String(key.into()),
        JsValue::Number(1.0),
        JsValue::Number(2.0),
    ];
    assert_eq!(decode_approval_event(&numeric, &[]), None);
    assert_eq!(
        decode_approval_event(&strings(&[key]), &strings(&[OWNER])),
        None
    );
    assert_eq!(decode_approval_event(&[], &[]), None);
    let past_prime = format!("0x8{}", "0".repeat(63));
    let keys = strings(&[key, OWNER, &past_prime]);
    assert_eq!(decode_approval_event(&keys, &[]), None);
}

#[test]
fn allowance_replies_parse_as_uint256() {
    // "combines the uint256 low and high words into a decimal string".
    let parse = |words: &[&str]| parse_allowance_amount(&strings(words));
    assert_eq!(parse(&["0x64", "0x0"]), Some(amount(100)));
    assert_eq!(parse(&["0x0", "0x1"]), Some(UNLIMITED));
    // "returns null for a malformed response".
    assert_eq!(parse(&["0x64"]), None);
    assert_eq!(parse(&["nope", "0x0"]), None);
    // Beyond the tests: both words at their maximum are 2^256 - 1; a limb at 2^128 is malformed.
    let max_limb = format!("0x{}", "f".repeat(32));
    let max = U256 {
        high: u128::MAX,
        low: u128::MAX,
    };
    assert_eq!(parse(&[&max_limb, &max_limb]), Some(max));
    let over = format!("0x1{}", "0".repeat(32));
    assert_eq!(parse(&[&over, "0x0"]), None);
    assert_eq!(parse(&["-1", "0x0"]), None);
}

#[test]
fn merging_lists_tracked_records_then_new_discoveries() {
    // "labels tracked entries as wallet-sourced and passes an empty external list through".
    let merged = merge(&[wallet_record()], &[], &[]);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].source, AllowanceSource::Wallet);
    assert_eq!(merged[0].amount, amount(1000));
    // "appends externally discovered allowances labelled as external".
    let merged = merge(
        &[wallet_record()],
        &[external(OTHER_SPENDER, UNLIMITED)],
        &[],
    );
    assert_eq!(merged.len(), 2);
    assert_eq!(merged[1].source, AllowanceSource::External);
    assert_eq!(merged[1].amount, UNLIMITED);
    assert_eq!(merged[1].spender, OTHER_SPENDER);
    let id = format!("{ACCOUNT}:{CANONICAL_TOKEN}:{OTHER_SPENDER}");
    assert_eq!(merged[1].id, id);
    // "drops an external allowance that duplicates a tracked (token, spender)".
    let merged = merge(&[wallet_record()], &[external(SPENDER, amount(999))], &[]);
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].source, AllowanceSource::Wallet);
}

#[test]
fn an_on_chain_reading_never_understates_a_grant() {
    // MCW-V09-108 "keeps the larger on-chain amount for a tracked (token, spender)".
    let merged = merge(&[wallet_record()], &[external(SPENDER, UNLIMITED)], &[]);
    assert_eq!((merged.len(), merged[0].amount), (1, UNLIMITED));
    assert_eq!(merged[0].source, AllowanceSource::Wallet);
    // "never lets a smaller or malformed on-chain amount shrink a tracked grant" (a malformed
    // amount is not representable here).
    for smaller in [U256::ZERO, amount(1)] {
        let merged = merge(&[wallet_record()], &[external(SPENDER, smaller)], &[]);
        assert_eq!((merged.len(), merged[0].amount), (1, amount(1000)));
    }
    // A reading at least the figure resolves a lower bound; a smaller one leaves it marked.
    let bound = AllowanceRecord {
        lower_bound: true,
        ..wallet_record()
    };
    let merged = merge(
        std::slice::from_ref(&bound),
        &[external(SPENDER, amount(1000))],
        &[],
    );
    assert!(!merged[0].lower_bound);
    let merged = merge(&[bound], &[external(SPENDER, amount(5))], &[]);
    assert!(merged[0].lower_bound);
}

#[test]
fn a_multisig_record_keeps_its_own_figure() {
    // "labels a multisig-owned record as such and never merges it with another owner's reading".
    let multisig = AllowanceRecord {
        account_id: "SN_MAIN:0xabc".into(),
        id: format!("SN_MAIN:0xabc:{CANONICAL_TOKEN}:{SPENDER}"),
        ..wallet_record()
    };
    let merged = merge(&[], &[external(SPENDER, UNLIMITED)], &[multisig]);
    assert_eq!(merged[0].source, AllowanceSource::Multisig);
    assert_eq!(merged[0].amount, amount(1000));
    assert_eq!(merged[1].source, AllowanceSource::External);
    assert_eq!(merged[1].amount, UNLIMITED);
}

#[test]
fn zero_and_malformed_discoveries_are_omitted() {
    // "omits zero-amount and malformed external allowances".
    let found = [
        external(OTHER_SPENDER, U256::ZERO),
        external("0xnot-a-felt", amount(5)),
    ];
    assert!(merge(&[], &found, &[]).is_empty());
    // Beyond the tests: a zero felt, and an account id of another network.
    assert!(merge(&[], &[external("0x0", amount(5))], &[]).is_empty());
    let found = [external(SPENDER, amount(5))];
    let sepolia = merge_allowance_views(ACCOUNT, NetworkId::Sepolia, &[], &[], &found, &[]);
    assert!(sepolia.is_empty());
}

#[test]
fn pending_revokes_take_their_reading_as_it_stands() {
    let pending = AllowanceRecord {
        revoking: true,
        revoke_hash: Some("0xabc".into()),
        ..wallet_record()
    };
    let read = |value: U256| AllowanceReading {
        id: pending.id.clone(),
        amount: value,
    };
    let views = |readings: &[AllowanceReading]| {
        let records = [pending.clone()];
        merge_allowance_views(ACCOUNT, NetworkId::Mainnet, &records, &[], &[], readings)
    };
    // allowance-revoke-hash.test.ts "a display view never carries the hash": the view type has
    // no such field; it keeps the mark.
    assert!(views(&[])[0].revoking);
    // A smaller reading is adopted for a pending revoke and keeps the mark; zero clears it, and
    // the listing then drops the row.
    let smaller = views(&[read(amount(250))]);
    assert_eq!(
        (smaller[0].amount, smaller[0].revoking),
        (amount(250), true)
    );
    let zero = views(&[read(U256::ZERO)]);
    assert_eq!((zero[0].amount, zero[0].revoking), (U256::ZERO, false));
    assert!(outstanding_allowances(zero, |_| true, &[]).is_empty());
    // A per-record reading wins over the event scan's figure for the same id.
    let reading = AllowanceReading {
        id: wallet_record().id,
        amount: amount(2000),
    };
    let merged = merge_allowance_views(
        ACCOUNT,
        NetworkId::Mainnet,
        &[wallet_record()],
        &[],
        &[external(SPENDER, UNLIMITED)],
        &[reading],
    );
    assert_eq!(merged[0].amount, amount(2000));
}

#[test]
fn the_listing_marks_what_it_cannot_vouch_for() {
    let views = merge(
        &[wallet_record()],
        &[external(OTHER_SPENDER, amount(7))],
        &[],
    );
    // A tracked token is fungible by the user's say-so; elsewhere only a reading confirms it.
    let fungible = outstanding_allowances(views.clone(), |token| token == CANONICAL_TOKEN, &[]);
    assert!(fungible.iter().all(|view| !view.unconfirmed_token));
    let unknown = outstanding_allowances(views.clone(), |_| false, &[]);
    assert!(unknown.iter().all(|view| view.unconfirmed_token));
    let reading = AllowanceReading {
        id: wallet_record().id,
        amount: amount(1000),
    };
    let confirmed = outstanding_allowances(views, |_| false, &[reading]);
    let marks: Vec<bool> = confirmed.iter().map(|v| v.unconfirmed_token).collect();
    assert_eq!(marks, [false, true]);
    // A zero lower bound is still outstanding: the base it bounds may stand.
    let zero_bound = AllowanceRecord {
        amount: U256::ZERO,
        lower_bound: true,
        ..wallet_record()
    };
    let listed = outstanding_allowances(merge(&[zero_bound], &[], &[]), |_| true, &[]);
    assert_eq!(listed.len(), 1);
    // listingNotices, in the manager's order.
    assert_eq!(listing_notices(true, false, false), []);
    assert_eq!(
        listing_notices(false, true, true),
        [
            AllowanceNotice::ScanIncomplete,
            AllowanceNotice::MultisigUnread,
            AllowanceNotice::RetentionFull
        ]
    );
}
