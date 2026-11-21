//! The amount sort: the extension's `compareActivityByAmount` cases from `activity-filter.test.ts`
//! at Bramble `a01b240c`, over the feed `filter_tests.rs` builds, then a feed the extension's
//! comparator gives no consistent order.

use super::tests::{by_amount_hashes, received, sent, swapped, with_token};
use super::*;

// `compareActivityByAmount` / `activityAmountComparator(pricing)` on two rows.
fn compare(left: ActivityEntry, right: ActivityEntry, tracked: &[TokenConfig]) -> Ordering {
    let keyed = |e: ActivityEntry| -> Keyed {
        let key = token_key(&e, activity_row_model(&e, tracked).token_address);
        (key, trimmed_token(&e).to_lowercase(), e)
    };
    by_amount(&keyed(left), &keyed(right))
}

// compareActivityByAmount: "orders one token's rows by recorded amount descending, then newest
// first".
#[test]
fn one_tokens_rows_order_by_amount_then_newest() {
    let ordered = by_amount_hashes(vec![sent(), with_token(swapped(), "STRK"), received()], &[]);
    assert_eq!(ordered, ["0x3", "0x2", "0x1"]);
    let tied = ActivityEntry {
        amount: Some("1.5".into()),
        ..received()
    };
    assert_eq!(compare(tied, sent(), &[]), Ordering::Less);
}

// compareActivityByAmount: "groups by token so a raw amount is never compared across tokens".
#[test]
fn rows_group_by_token_before_amount() {
    let row = |symbol, amount: &str, hash: &str| ActivityEntry {
        amount: Some(amount.into()),
        hash: hash.into(),
        ..with_token(sent(), symbol)
    };
    let rows = vec![row("USDC", "2", "0xf"), row("ETH", "1", "0xe"), sent()];
    // ETH, STRK, USDC: 2 USDC never outranks 1 ETH.
    assert_eq!(by_amount_hashes(rows, &[]), ["0xe", "0x1", "0xf"]);
}

// compareActivityByAmount: "sorts a token-less row, and a row with no numeric amount, last".
#[test]
fn token_less_and_amountless_rows_sort_last() {
    let undated = ActivityEntry {
        amount: None,
        hash: "0x0".into(),
        summary: "Send 1 STRK".into(),
        ..sent()
    };
    assert_eq!(compare(undated.clone(), sent(), &[]), Ordering::Greater);
    assert_eq!(compare(sent(), undated.clone(), &[]), Ordering::Less);
    // Two rows with no amount, in one token, keep the feed's newest-first order.
    let newer = ActivityEntry {
        submitted_at: undated.submitted_at + 1000,
        ..undated.clone()
    };
    assert_eq!(compare(undated, newer, &[]), Ordering::Greater);
    // A row with no token at all sorts after every token-bearing row.
    assert_eq!(compare(swapped(), sent(), &[]), Ordering::Greater);
    assert_eq!(compare(sent(), swapped(), &[]), Ordering::Less);
}

fn at(address: &str, symbol: &str, amount: &str, hash: &str) -> ActivityEntry {
    ActivityEntry {
        amount: Some(amount.into()),
        hash: hash.into(),
        token_address: Some(address.into()),
        ..with_token(sent(), symbol)
    }
}

// One address recorded under two symbols (a renamed token, or a spam contract) with a third token
// between them: the extension's comparator puts B < C < A and A < B. Each address groups under its
// first row's symbol, so the sort is total (Rust's sort panics otherwise).
#[test]
fn an_address_under_two_symbols_groups_under_its_first() {
    let rows = vec![
        at("0xa", "zzz", "1", "0x1"),
        at("0xa", "aaa", "2", "0x2"),
        at("0xb", "mmm", "3", "0x3"),
    ];
    assert_eq!(by_amount_hashes(rows, &[]), ["0x3", "0x2", "0x1"]);
    // Many such rows: no panic, and each address's rows stay together.
    let symbols = ["USDC", "usdc.e", "ETH", "mmm", "aaa"];
    let rows: Vec<ActivityEntry> = (0..64)
        .map(|i| {
            let address = format!("0x{}", i % 3 + 1);
            at(
                &address,
                symbols[i % 5],
                &(i % 7).to_string(),
                &format!("0x{i:x}"),
            )
        })
        .collect();
    let addresses: Vec<Option<String>> = filter_activity(
        rows,
        &ActivityFilter {
            sort: ActivitySort::Amount,
            ..ActivityFilter::default()
        },
        &[],
        &[],
    )
    .into_iter()
    .map(|e| e.token_address)
    .collect();
    let mut groups = addresses.clone();
    groups.dedup();
    assert_eq!(groups.len(), 3, "{addresses:?}");
}
