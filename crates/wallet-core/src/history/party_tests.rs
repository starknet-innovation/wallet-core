//! The extension's own cases for the per-counterparty summary, ported with their inputs and
//! expected strings from `packages/presentation/wallet-ui/tests/sidepanel/journeys/portfolio/
//! activity-filter.test.ts` at Bramble `a01b240c` (each names its `describe` and case). Those run
//! over hand-written row facts (`summaryRow`), so these run `summarise` over the same facts; the
//! cases after them cover the branches they leave out and the path from entries. No generator
//! writes a fixture for this yet.

use super::*;
use crate::history::activity::ActivityKind;
use crate::NetworkId;

const CONTACT: &str = "0x0000000000000000000000000000000000000000000000000000000000000abc";
const CONTACT_SHORT: &str = "0xabc";
const OTHER_NETWORK: &str = "0x0000000000000000000000000000000000000000000000000000000000000def";
const JULY_14: u64 = 1_752_501_900_000;

// The test's `summaryRow`.
fn row() -> SummaryRow {
    SummaryRow {
        amount: "1".into(),
        counterparty: CONTACT_SHORT.into(),
        decimals: Some(18),
        direction: Some(ActivityDirection::Out),
        label: String::new(),
        status: TxStatus::Accepted,
        submitted_at: JULY_14,
        token: "STRK".into(),
        token_key: "symbol:strk".into(),
    }
}

fn amount(amount: &str, direction: ActivityDirection) -> SummaryRow {
    SummaryRow {
        amount: amount.into(),
        direction: Some(direction),
        ..row()
    }
}

fn total(received: &str, sent: &str, symbol: &str) -> ActivityTokenTotal {
    ActivityTokenTotal {
        received: received.into(),
        sent: sent.into(),
        symbol: symbol.into(),
    }
}

fn totals(rows: Vec<SummaryRow>) -> Vec<ActivityTokenTotal> {
    summarise(&rows).map(|s| s.totals).unwrap_or_default()
}

fn body(rows: Vec<SummaryRow>) -> String {
    summarise(&rows).map_or_else(String::new, |s| activity_party_summary_parts(&s).body)
}

// activityPartySummary: "summarises one counterparty's count, totals and date range".
#[test]
fn one_counterparty_is_summarised() {
    let summary = summarise(&[
        amount("1.5", ActivityDirection::Out),
        SummaryRow {
            label: "Ada".into(),
            submitted_at: 1_752_501_960_000,
            ..amount("2", ActivityDirection::In)
        },
    ]);
    let summary = summary.unwrap_or_else(|| panic!("summary missing"));
    assert_eq!(summary.count, 2);
    assert_eq!(summary.label, "Ada");
    assert_eq!(summary.totals, [total("2", "1.5", "STRK")]);
    assert_eq!(summary.first_at, Some(JULY_14));
    assert_eq!(summary.last_at, Some(1_752_501_960_000));
}

// activityPartySummary: "totals only what settled, while counting every visible row".
#[test]
fn only_settled_rows_add_to_the_totals() {
    let status = |amount: &str, status| SummaryRow {
        amount: amount.into(),
        status,
        ..row()
    };
    let rows = vec![
        status("1", TxStatus::Accepted),
        status("5", TxStatus::Reverted),
        status("7", TxStatus::Rejected),
        status("11", TxStatus::Pending),
        SummaryRow {
            direction: Some(ActivityDirection::In),
            ..status("2", TxStatus::Preconfirmed)
        },
    ];
    let summary = summarise(&rows).unwrap_or_else(|| panic!("summary missing"));
    assert_eq!(summary.count, 5);
    assert_eq!(summary.totals, [total("2", "1", "STRK")]);
    let failed = body(rows.into_iter().take(3).collect());
    assert!(
        failed.starts_with("3 transactions · Sent 1 STRK"),
        "{failed}"
    );
}

// activityPartySummary: "returns null unless the visible rows name exactly one counterparty".
#[test]
fn none_unless_exactly_one_counterparty() {
    assert_eq!(summarise(&[]), None);
    let other = SummaryRow {
        counterparty: OTHER_NETWORK.into(),
        ..row()
    };
    assert_eq!(summarise(&[row(), other]), None);
}

// activityPartySummary: "keeps one counterparty's totals per token, in arrival order".
#[test]
fn totals_keep_arrival_order_per_token() {
    let eth = SummaryRow {
        amount: "2".into(),
        token: "ETH".into(),
        token_key: "symbol:eth".into(),
        ..row()
    };
    let symbols: Vec<String> = totals(vec![row(), eth])
        .into_iter()
        .map(|t| t.symbol)
        .collect();
    assert_eq!(symbols, ["STRK", "ETH"]);
}

// activityPartySummary: "counts a row it cannot total instead of trusting a bad amount".
#[test]
fn a_row_that_cannot_be_totalled_is_still_counted() {
    let none: Vec<ActivityTokenTotal> = Vec::new();
    assert_eq!(
        totals(vec![amount("not-a-number", ActivityDirection::Out)]),
        none
    );
    assert_eq!(
        totals(vec![amount(
            "1.0000000000000000001",
            ActivityDirection::Out
        )]),
        none
    );
    let no_decimals = SummaryRow {
        decimals: None,
        ..row()
    };
    assert_eq!(totals(vec![no_decimals]), none);
    assert_eq!(totals(vec![amount("1", ActivityDirection::Internal)]), none);
    let no_token = SummaryRow {
        token: String::new(),
        token_key: String::new(),
        ..row()
    };
    assert_eq!(totals(vec![no_token]), none);
    assert_eq!(
        summarise(&[amount("abc", ActivityDirection::Out)]).map(|s| s.count),
        Some(1)
    );
}

// activityPartySummaryParts: "carries the name, the count, both directions and the date".
#[test]
fn the_parts_carry_name_count_directions_and_date() {
    let named = |row: SummaryRow| SummaryRow {
        label: "Ada".into(),
        ..row
    };
    let summary = summarise(&[
        named(amount("1.5", ActivityDirection::Out)),
        named(SummaryRow {
            submitted_at: 1_752_501_960_000,
            ..amount("2", ActivityDirection::In)
        }),
    ]);
    let parts = summary.map(|s| activity_party_summary_parts(&s));
    let parts = parts.unwrap_or_else(|| panic!("summary missing"));
    assert_eq!(parts.label, "Ada");
    assert_eq!(
        parts.body,
        "2 transactions · Sent 1.5 STRK · Received 2 STRK · 14 Jul 2025"
    );
}

// activityPartySummaryParts: "says one transaction in the singular and heads an unnamed row by
// its address".
#[test]
fn one_transaction_is_singular_and_an_unnamed_party_is_its_address() {
    let parts = summarise(&[row()]).map(|s| activity_party_summary_parts(&s));
    let parts = parts.unwrap_or_else(|| panic!("summary missing"));
    assert_eq!(parts.label, CONTACT_SHORT);
    assert_eq!(parts.body, "1 transaction · Sent 1 STRK · 14 Jul 2025");
}

// activityPartySummaryParts: "returns the name verbatim, leaving escaping to the writer".
#[test]
fn the_name_is_returned_verbatim() {
    let hostile = SummaryRow {
        label: "<img src=x>".into(),
        ..row()
    };
    let parts = summarise(&[hostile]).map(|s| activity_party_summary_parts(&s));
    assert_eq!(parts.map(|p| p.label).as_deref(), Some("<img src=x>"));
}

// token and counterparty identity: "one counterparty written with and without leading zeros is
// one counterparty".
#[test]
fn one_counterparty_in_two_spellings_is_one() {
    let padded = SummaryRow {
        counterparty: CONTACT.into(),
        ..amount("2", ActivityDirection::In)
    };
    let summary = summarise(&[row(), padded]).unwrap_or_else(|| panic!("summary missing"));
    assert_eq!(summary.count, 2);
    assert_eq!(summary.totals, [total("2", "1", "STRK")]);
}

// token and counterparty identity: "totals are kept apart per token key even when the symbol is
// shared".
#[test]
fn totals_are_kept_apart_per_token_key() {
    let usdc = |amount: &str, key: &str| SummaryRow {
        amount: amount.into(),
        token: "USDC".into(),
        token_key: key.into(),
        ..row()
    };
    assert_eq!(
        totals(vec![usdc("1", "0x1"), usdc("2", "0x2")]),
        [total("0", "1", "USDC"), total("0", "2", "USDC")]
    );
}

// Not in the extension's tests from here on.

#[test]
fn a_range_of_days_one_direction_and_an_undated_row() {
    let next_day = |row: SummaryRow| SummaryRow {
        submitted_at: JULY_14 + 86_400_000,
        ..row
    };
    // A zero sent total is left out; one day reads as that day.
    let rows = vec![
        next_day(amount("2", ActivityDirection::In)),
        next_day(amount("0", ActivityDirection::Out)),
    ];
    assert_eq!(body(rows), "2 transactions · Received 2 STRK · 15 Jul 2025");
    let range = vec![row(), row(), next_day(row())];
    assert_eq!(
        body(range),
        "3 transactions · Sent 3 STRK · 14 Jul 2025 to 15 Jul 2025"
    );
    let undated = SummaryRow {
        submitted_at: 0,
        ..row()
    };
    assert_eq!(body(vec![undated]), "1 transaction · Sent 1 STRK");
}

#[test]
fn amounts_follow_the_decimal_grammar_and_sum_without_bound() {
    for bad in ["1.", ".5", "+1", "1e3", "0", "0.000", "1,5"] {
        assert_eq!(
            totals(vec![amount(bad, ActivityDirection::Out)]),
            [],
            "{bad}"
        );
    }
    assert_eq!(
        totals(vec![amount(" 2.50 ", ActivityDirection::Out)]),
        [total("0", "2.5", "STRK")]
    );
    // Two u256-sized amounts in a token with no decimals: a `bigint` sum, not a wrapped one.
    let huge = "9".repeat(78);
    let big = |amount: &str| SummaryRow {
        amount: amount.into(),
        decimals: Some(0),
        ..row()
    };
    let expected = format!("1{}8", "9".repeat(77));
    assert_eq!(
        totals(vec![big(&huge), big(&huge)]),
        [total("0", &expected, "STRK")]
    );
}

fn entry(
    hash: &str,
    summary: &str,
    status: TxStatus,
    token_address: Option<&str>,
) -> ActivityEntry {
    ActivityEntry {
        account_id: "SN_MAIN:0x1234".into(),
        amount: Some("1.5".into()),
        counterparty: Some(CONTACT_SHORT.into()),
        direction: None,
        failure_notice: None,
        fee_paid: None,
        finalized_on_l1: None,
        hash: hash.into(),
        kind: ActivityKind::Transfer,
        network: NetworkId::Mainnet,
        not_found_count: None,
        origin: None,
        status,
        submitted_at: JULY_14,
        submitted_price: None,
        summary: summary.into(),
        token: Some("STRK".into()),
        token_address: token_address.map(str::to_string),
    }
}

#[test]
fn the_summary_reads_its_facts_off_the_entries() {
    let strk = crate::catalog::testing::STRK;
    let tracked = [TokenConfig {
        id: format!("SN_MAIN:STRK:{strk}"),
        network: NetworkId::Mainnet,
        address: strk.into(),
        symbol: "STRK".into(),
        name: "Starknet Token".into(),
        decimals: 18,
    }];
    let ada = [ActivityContact {
        address: CONTACT.into(),
        label: "Ada".into(),
    }];
    let rows = [
        entry("0x1", "Send 1.5 STRK", TxStatus::Accepted, None),
        entry("0x2", "Receive 1.5 STRK", TxStatus::Preconfirmed, None),
        // Another contract recorded under the tracked symbol has no decimals (Sherlock 249):
        // counted, never totalled.
        entry("0x3", "Send 1.5 STRK", TxStatus::Accepted, Some("0x5")),
    ];
    let summary = activity_party_summary(&rows, &tracked, &ada);
    let parts = summary.map(|s| activity_party_summary_parts(&s));
    let parts = parts.unwrap_or_else(|| panic!("summary missing"));
    assert_eq!(parts.label, "Ada");
    assert_eq!(
        parts.body,
        "3 transactions · Sent 1.5 STRK · Received 1.5 STRK · 14 Jul 2025"
    );
    // Without the tracked token no row has decimals, so nothing is totalled.
    let bare = activity_party_summary(&rows, &[], &[]).map(|s| activity_party_summary_parts(&s));
    let bare = bare.unwrap_or_else(|| panic!("summary missing"));
    assert_eq!(
        (bare.label.as_str(), bare.body.as_str()),
        (CONTACT_SHORT, "3 transactions · 14 Jul 2025")
    );
}
