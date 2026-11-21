//! The extension's own cases for `reconcileActivityFilter`, from `packages/presentation/
//! wallet-ui/tests/sidepanel/journeys/portfolio/activity-filter.test.ts` at Bramble `a01b240c`
//! ("reconcileActivityFilter"), over the feed `filter_tests.rs` builds.

use super::tests::{received, sent, swapped};
use super::*;

fn options() -> ActivityFilterOptions {
    activity_filter_options(&[swapped(), sent(), received()], &[])
}

fn axes(action: &str, status: TxStatus, token: &str) -> ActivityFilter {
    ActivityFilter {
        action: Some(action.into()),
        status: Some(status),
        token: Some(token.into()),
        ..ActivityFilter::default()
    }
}

// "passes a filter whose values the feed still offers".
#[test]
fn a_filter_the_feed_still_offers_passes() {
    let kept = axes("Sent", TxStatus::Accepted, "symbol:strk");
    assert_eq!(reconcile_activity_filter(kept.clone(), &options()), kept);
}

// "resets only the stale axes, keeping the query and the sort".
#[test]
fn only_stale_axes_reset() {
    let stale = ActivityFilter {
        query: "ada".into(),
        sort: ActivitySort::Amount,
        ..axes("Approved", TxStatus::Reverted, "symbol:eth")
    };
    let expected = ActivityFilter {
        query: "ada".into(),
        sort: ActivitySort::Amount,
        ..ActivityFilter::default()
    };
    assert_eq!(reconcile_activity_filter(stale, &options()), expected);
}

// "clears every axis when the feed offers no options".
#[test]
fn every_axis_clears_without_options() {
    let stale = ActivityFilter {
        query: "x".into(),
        ..axes("Sent", TxStatus::Accepted, "STRK")
    };
    let empty = ActivityFilterOptions {
        actions: Vec::new(),
        statuses: Vec::new(),
        tokens: Vec::new(),
    };
    let expected = ActivityFilter {
        query: "x".into(),
        ..ActivityFilter::default()
    };
    assert_eq!(reconcile_activity_filter(stale, &empty), expected);
}

// Not in the extension's tests: an axis already at all stays there, and a token kept by key.
#[test]
fn all_stays_all_and_tokens_match_by_key() {
    assert_eq!(
        reconcile_activity_filter(ActivityFilter::default(), &options()),
        ActivityFilter::default()
    );
    let by_label = ActivityFilter {
        token: Some("STRK".into()),
        ..ActivityFilter::default()
    };
    assert_eq!(reconcile_activity_filter(by_label, &options()).token, None);
}
