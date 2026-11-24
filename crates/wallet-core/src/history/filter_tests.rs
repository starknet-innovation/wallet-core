//! The extension's own cases for the feed filter and sort, ported with their inputs and expected
//! values from `packages/presentation/wallet-ui/tests/sidepanel/journeys/portfolio/
//! activity-filter.test.ts` at Bramble `a01b240c` (each test names its `describe` and case). The
//! extension's `activityRowMatches` cases run over hand-written row facts; here each runs over an
//! entry whose facts are those. Its `activityContacts`, `reconcileActivityFilter` and
//! party-summary cases are not ported (not part of this port). The cases after them cover
//! branches the extension's tests leave out. No generator writes a fixture for this yet.

use super::*;
use crate::catalog::testing::STRK;
use crate::history::activity::ActivityKind;
use crate::NetworkId;

const ACCOUNT_ID: &str = "SN_MAIN:0x1234";
const CONTACT: &str = "0x0000000000000000000000000000000000000000000000000000000000000abc";
const CONTACT_SHORT: &str = "0xabc";
const STRK_KEY: &str = "0x4718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d";

fn strk() -> TokenConfig {
    TokenConfig {
        id: format!("SN_MAIN:STRK:{STRK}"),
        network: NetworkId::Mainnet,
        address: STRK.into(),
        symbol: "STRK".into(),
        name: "Starknet Token".into(),
        decimals: 18,
    }
}

pub(super) fn sent() -> ActivityEntry {
    ActivityEntry {
        account_id: ACCOUNT_ID.into(),
        amount: Some("1.5".into()),
        counterparty: Some(CONTACT_SHORT.into()),
        direction: None,
        failure_notice: None,
        fee_payer: None,
        user_fee: None,
        fee_paid: None,
        finalized_on_l1: None,
        hash: "0x1".into(),
        kind: ActivityKind::Transfer,
        network: NetworkId::Mainnet,
        not_found_count: None,
        origin: None,
        status: TxStatus::Accepted,
        submitted_at: 1_752_501_900_000,
        submitted_price: None,
        summary: "Send 1.5 STRK".into(),
        token: Some("STRK".into()),
        token_address: None,
    }
}

pub(super) fn received() -> ActivityEntry {
    ActivityEntry {
        amount: Some("2".into()),
        hash: "0x2".into(),
        status: TxStatus::Preconfirmed,
        submitted_at: 1_752_501_960_000,
        summary: "Receive 2 STRK".into(),
        ..sent()
    }
}

pub(super) fn swapped() -> ActivityEntry {
    ActivityEntry {
        amount: Some("5".into()),
        counterparty: None,
        hash: "0x3".into(),
        kind: ActivityKind::Invoke,
        status: TxStatus::Pending,
        submitted_at: 1_752_502_020_000,
        summary: "Swap 5 STRK".into(),
        token: None,
        ..sent()
    }
}

fn contact(address: &str, label: &str) -> ActivityContact {
    ActivityContact {
        address: address.into(),
        label: label.into(),
    }
}

fn ada() -> Vec<ActivityContact> {
    vec![contact(CONTACT, "Ada")]
}

pub(super) fn with_token(entry: ActivityEntry, symbol: &str) -> ActivityEntry {
    ActivityEntry {
        token: Some(symbol.into()),
        ..entry
    }
}

// Whether `entry` survives `filter`, with Ada in the contacts and no tracked token.
fn keeps(entry: ActivityEntry, filter: ActivityFilter) -> bool {
    !filter_activity(vec![entry], &filter, &[], &ada()).is_empty()
}

fn query(text: &str) -> ActivityFilter {
    ActivityFilter {
        query: text.into(),
        ..ActivityFilter::default()
    }
}

pub(super) fn by_amount_hashes(
    entries: Vec<ActivityEntry>,
    tracked: &[TokenConfig],
) -> Vec<String> {
    let filter = ActivityFilter {
        sort: ActivitySort::Amount,
        ..ActivityFilter::default()
    };
    filter_activity(entries, &filter, tracked, &[])
        .into_iter()
        .map(|e| e.hash)
        .collect()
}

fn symbol_option(key: &str, label: &str) -> ActivityTokenOption {
    ActivityTokenOption {
        key: key.into(),
        label: label.into(),
    }
}

// activityFilterOptions: "offers only the values the feed contains, in lifecycle and text order".
#[test]
fn options_offer_only_the_values_the_feed_contains() {
    let options = activity_filter_options(&[swapped(), sent(), received()], &[]);
    assert_eq!(options.actions, ["Received", "Sent", "Swapped"]);
    assert_eq!(
        options.statuses,
        [
            TxStatus::Accepted,
            TxStatus::Pending,
            TxStatus::Preconfirmed
        ]
    );
    assert_eq!(options.tokens, [symbol_option("symbol:strk", "STRK")]);
}

// activityFilterOptions: "omits a token axis the feed never records".
#[test]
fn options_omit_a_token_axis_the_feed_never_records() {
    assert_eq!(activity_filter_options(&[swapped()], &[]).tokens, []);
}

// activitySearchIndex: "carries the saved name and the address in every stored spelling".
#[test]
fn the_search_index_carries_the_name_and_every_spelling() {
    let index = search_index(Some(CONTACT_SHORT), Some("Ada"));
    assert!(index.starts_with("ada 0xabc "));
    assert!(index.contains(CONTACT));
    assert_eq!(
        search_index(Some(CONTACT), None),
        format!("{CONTACT} 0xabc")
    );
    assert_eq!(search_index(None, None), "");
    assert_eq!(search_index(Some("not-a-felt"), None), "not-a-felt");
}

// activitySearchIndex: "matches a padded address and a padded partial against a short-spelled
// record".
#[test]
fn a_padded_address_matches_a_short_spelled_record() {
    let recipient = "0x4567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
    let padded = canonical_address(recipient).unwrap_or_default();
    let contacts = [contact(recipient, "Research treasury")];
    let entry = ActivityEntry {
        counterparty: Some(recipient.into()),
        ..sent()
    };
    let matches =
        |text: &str| !filter_activity(vec![entry.clone()], &query(text), &[], &contacts).is_empty();
    assert_eq!(padded.len(), 66);
    assert_ne!(padded, recipient);
    assert!(matches(&padded));
    assert!(matches(&padded[..10]));
    assert!(matches("0x4567890a"));
    assert!(matches("Research Treasury"));
    assert!(!matches("0x9999"));
}

// activityRowAttributes: "describe the row for the binding without repeating the render model",
// the facts the filter reads (action, search, status); amount, decimals, direction and time feed
// only the party summary.
#[test]
fn a_row_carries_the_facts_the_filter_reads() {
    let tracked = [strk()];
    let model = activity_row_model(&sent(), &tracked);
    assert_eq!(model.title, "Sent");
    let contacts = ada();
    let label = contact_for(Some(CONTACT_SHORT), &contacts).map(|c| c.label.as_str());
    assert_eq!(
        search_index(Some(CONTACT_SHORT), label),
        format!("ada 0xabc {CONTACT}")
    );
    let filter = ActivityFilter {
        action: Some("Sent".into()),
        status: Some(TxStatus::Accepted),
        token: Some(STRK_KEY.into()),
        query: "ada".into(),
        ..ActivityFilter::default()
    };
    let kept = filter_activity(vec![sent()], &filter, &tracked, &ada());
    assert_eq!(kept, [sent()]);
}

// activityRowAttributes: "omits absent facts rather than writing empty attributes".
#[test]
fn a_sparse_row_has_no_token_or_counterparty_facts() {
    let entry = swapped();
    let model = activity_row_model(&entry, &[]);
    assert_eq!(model.title, "Swapped");
    assert_eq!(token_key(&entry, model.token_address), "");
    assert_eq!(search_index(entry.counterparty.as_deref(), None), "");
}

// activityRowMatches: "passes the unfiltered feed".
#[test]
fn the_unfiltered_feed_passes() {
    assert!(keeps(sent(), ActivityFilter::default()));
}

// activityRowMatches: "applies every axis conjunctively".
#[test]
fn every_axis_applies_conjunctively() {
    let axes = |action: Option<&str>, status, token: Option<&str>| ActivityFilter {
        action: action.map(str::to_string),
        status,
        token: token.map(str::to_string),
        ..ActivityFilter::default()
    };
    let (accepted, pending) = (Some(TxStatus::Accepted), Some(TxStatus::Pending));
    assert!(!keeps(sent(), axes(Some("Received"), None, None)));
    assert!(keeps(sent(), axes(None, accepted, None)));
    assert!(!keeps(sent(), axes(None, pending, None)));
    assert!(keeps(sent(), axes(None, None, Some("symbol:strk"))));
    assert!(!keeps(sent(), axes(None, None, Some("symbol:eth"))));
    // The axis compares the token's identity, never its symbol.
    assert!(!keeps(sent(), axes(None, None, Some("STRK"))));
    assert!(keeps(sent(), axes(Some("Sent"), None, Some("symbol:strk"))));
    assert!(!keeps(sent(), axes(Some("Sent"), pending, None)));
}

// activityRowMatches: "matches a full address, a partial one, and a saved name
// case-insensitively".
#[test]
fn a_search_matches_addresses_and_names_case_insensitively() {
    assert!(keeps(sent(), query("ADA")));
    assert!(keeps(sent(), query("0xabc")));
    assert!(keeps(sent(), query("0xab")));
    assert!(keeps(sent(), query("  ada  ")));
    assert!(!keeps(sent(), query("0xzzz")));
}

// activityRowMatches: "reads an absent attribute as matching only the all axis", over a row with
// no token and no counterparty.
#[test]
fn an_absent_fact_matches_only_the_all_axis() {
    assert!(keeps(swapped(), ActivityFilter::default()));
    let token = ActivityFilter {
        token: Some("symbol:strk".into()),
        ..ActivityFilter::default()
    };
    assert!(!keeps(swapped(), token));
    assert!(!keeps(swapped(), query("ada")));
}

// token and counterparty identity: "a tracked token is keyed, offered and sorted by its address,
// never its symbol".
#[test]
fn a_tracked_token_is_keyed_by_its_address() {
    let tracked = [strk()];
    let key = token_key(&sent(), activity_row_model(&sent(), &tracked).token_address);
    assert_eq!(key, STRK_KEY);
    assert_eq!(
        activity_filter_options(&[sent(), received()], &tracked).tokens,
        [symbol_option(STRK_KEY, "STRK")]
    );
    // A symbol no single tracked token answers to keeps a key of its own.
    let usdc = with_token(sent(), "USDC");
    assert_eq!(
        token_key(&usdc, activity_row_model(&usdc, &tracked).token_address),
        "symbol:usdc"
    );
    let fake = ActivityEntry {
        amount: Some("1000".into()),
        hash: "0xfa".into(),
        ..usdc
    };
    assert_eq!(
        by_amount_hashes(vec![fake, sent()], &tracked),
        ["0x1", "0xfa"]
    );
}

// token and counterparty identity: "tokens sharing a ticker are told apart by a short address,
// others are not".
#[test]
fn tokens_sharing_a_ticker_are_told_apart() {
    let native = "0x033068f6539f8e6e6b131e6b2b814e6c34a5224bc66947c47dab9dfee93b35fb";
    let bridged = "0x053c91253bc9682c04929ca02ed00b3e423f6710d2ee7e0d5ebb06f3ecf368a8";
    let usdc = |hash: &str, token_address: Option<&str>| ActivityEntry {
        hash: hash.into(),
        summary: "Send 1.5 USDC".into(),
        token: Some("USDC".into()),
        token_address: token_address.map(str::to_string),
        ..sent()
    };
    let options = activity_filter_options(
        &[
            usdc("0xb1", Some(bridged)),
            usdc("0xa1", Some(native)),
            sent(),
            usdc("0xa2", None),
        ],
        &[strk()],
    );
    let canonical = |a: &str| canonical_felt(a).unwrap_or_default();
    assert_eq!(
        options.tokens,
        [
            symbol_option(STRK_KEY, "STRK"),
            symbol_option(&canonical(native), "USDC · 0x0330…35fb"),
            symbol_option(&canonical(bridged), "USDC · 0x053c…68a8"),
            symbol_option("symbol:usdc", "USDC"),
        ]
    );
}

// Not in the extension's tests from here on.

#[test]
fn an_empty_feed_offers_nothing_and_keeps_nothing() {
    let options = activity_filter_options(&[], &[]);
    assert!(options.actions.is_empty() && options.statuses.is_empty() && options.tokens.is_empty());
    assert!(filter_activity(Vec::new(), &query("ada"), &[], &ada()).is_empty());
}

#[test]
fn every_status_is_offered_in_lifecycle_order_and_filters() {
    let mut entries: Vec<ActivityEntry> = STATUS_ORDER
        .iter()
        .map(|&status| ActivityEntry { status, ..sent() })
        .collect();
    entries.reverse();
    let offered = activity_filter_options(&entries, &[]).statuses;
    assert_eq!(offered, STATUS_ORDER);
    for status in STATUS_ORDER {
        let filter = ActivityFilter {
            status: Some(status),
            ..ActivityFilter::default()
        };
        let kept = filter_activity(entries.clone(), &filter, &[], &[]);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].status, status);
    }
}

#[test]
fn every_kind_offers_its_verb() {
    let kind = |kind, summary: &str| ActivityEntry {
        kind,
        summary: summary.into(),
        ..sent()
    };
    let options = activity_filter_options(
        &[
            kind(ActivityKind::Declare, "Declare class"),
            kind(ActivityKind::Deploy, "Deploy account contract"),
            kind(ActivityKind::Invoke, "Call contract"),
            kind(ActivityKind::Transfer, "Send 1.5 STRK"),
        ],
        &[],
    );
    assert_eq!(
        options.actions,
        ["Declared", "Deployed", "Interacted", "Sent"]
    );
    let deployed = ActivityFilter {
        action: Some("Deployed".into()),
        ..ActivityFilter::default()
    };
    let rows = [
        sent(),
        kind(ActivityKind::Deploy, "Deploy account contract"),
    ];
    let kept = filter_activity(rows.to_vec(), &deployed, &[], &[]);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].kind, ActivityKind::Deploy);
}

#[test]
fn a_contact_resolves_across_spellings_and_an_empty_label_adds_nothing() {
    // A contact stored short still names a row that recorded the padded spelling.
    let short = [contact(CONTACT_SHORT, "Ada")];
    let padded = ActivityEntry {
        counterparty: Some(CONTACT.into()),
        ..sent()
    };
    assert!(!filter_activity(vec![padded.clone()], &query("ada"), &[], &short).is_empty());
    let unnamed = [contact(CONTACT, "")];
    assert!(filter_activity(vec![padded], &query("ada"), &[], &unnamed).is_empty());
    // An empty counterparty resolves no contact (`BigInt("")` would be zero).
    assert_eq!(contact_for(Some(""), &short), None);
}

#[test]
fn the_search_is_case_folded_beyond_ascii() {
    let contacts = [contact(CONTACT, "Équipe Zürich")];
    assert!(!filter_activity(vec![sent()], &query("ÉQUIPE zÜRICH"), &[], &contacts).is_empty());
    assert!(filter_activity(vec![sent()], &query("equipe"), &[], &contacts).is_empty());
}

#[test]
fn the_date_sort_keeps_the_feed_order_and_the_amount_sort_is_stable() {
    let feed = vec![received(), swapped(), sent()];
    let kept = filter_activity(feed.clone(), &ActivityFilter::default(), &[], &[]);
    assert_eq!(kept, feed);
    // Equal amount and time in one token: the given order stands.
    let twin = ActivityEntry {
        hash: "0x9".into(),
        ..sent()
    };
    assert_eq!(by_amount_hashes(vec![twin, sent()], &[]), ["0x9", "0x1"]);
    // An amount that is not a plain number sorts as no amount.
    let words = ActivityEntry {
        amount: Some("lots".into()),
        hash: "0x8".into(),
        submitted_at: 2_000_000_000_000,
        ..sent()
    };
    assert_eq!(by_amount_hashes(vec![words, sent()], &[]), ["0x1", "0x8"]);
}

#[test]
fn options_sort_by_utf16_units_as_javascript_does() {
    // U+FF61 sorts after an astral symbol in UTF-16 (0xFF61 > 0xD83D), before it in UTF-8.
    let options =
        activity_filter_options(&[with_token(sent(), "｡A"), with_token(sent(), "😀")], &[]);
    let labels: Vec<&str> = options.tokens.iter().map(|t| t.label.as_str()).collect();
    assert_eq!(labels, ["😀", "｡A"]);
}
