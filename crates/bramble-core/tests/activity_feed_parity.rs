//! The Activity feed's CSV export, filter, amount sort, stale-axis reset and per-counterparty
//! summary (P10) against what the extension's own code produced over one fixed feed
//! (`fixtures/extension-activity-feed.jsonl`, `bun run mobile:parity`): `activityToCsv`,
//! `activityFilterOptions`, `reconcileActivityFilter`, `activityAmountComparator` with
//! `activityRowMatches`, and `activityPartySummary` with its parts (and, in the mobile build,
//! bramble-core's `activity_party_summary` with the feed's blank-search gate).

use crate::portfolio_support;

use bramble_core::catalogs::networks;
use portfolio_support::{at, list, text, token};
use starknet_wallet_core::activity::csv::activity_to_csv;
use starknet_wallet_core::activity::store::{parse_status, status_text};
use starknet_wallet_core::history::activity::{entry_from_json, ActivityEntry};
use starknet_wallet_core::history::filter::{
    activity_filter_options, filter_activity, reconcile_activity_filter, ActivityContact,
    ActivityFilter, ActivityFilterOptions, ActivitySort,
};
use starknet_wallet_core::history::party::{
    activity_party_summary, activity_party_summary_parts, ActivityPartySummary,
};
use starknet_wallet_core::js::json::{parse, JsValue};
#[cfg(feature = "mobile")]
use starknet_wallet_core::js::num::js_trim;
use starknet_wallet_core::portfolio::model::TokenConfig;

fn cases(block: &str) -> Vec<JsValue> {
    let found: Vec<JsValue> = include_str!("fixtures/extension-activity-feed.jsonl")
        .lines()
        .map(|line| parse(line).expect("a JSON line"))
        .filter(|case| case.get("block").and_then(JsValue::as_str) == Some(block))
        .collect();
    assert!(!found.is_empty(), "no {block} cases");
    found
}

struct Feed {
    entries: Vec<ActivityEntry>,
    tracked: Vec<TokenConfig>,
    contacts: Vec<ActivityContact>,
}

fn feed() -> Feed {
    let feed = cases("feed").remove(0);
    Feed {
        entries: list(&feed, "entries")
            .iter()
            .map(|entry| entry_from_json(entry).expect("a stored row"))
            .collect(),
        tracked: list(&feed, "tracked").iter().map(token).collect(),
        contacts: list(&feed, "contacts")
            .iter()
            .map(|contact| ActivityContact {
                address: text(contact, "address"),
                label: text(contact, "label"),
            })
            .collect(),
    }
}

fn hashes(value: &JsValue) -> Vec<String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|hash| hash.as_str().unwrap().to_string())
        .collect()
}

impl Feed {
    // The case's rows (`hashes`, in the feed's order), or the whole feed.
    fn rows(&self, case: &JsValue) -> Vec<ActivityEntry> {
        let Some(wanted) = case.get("hashes").map(hashes) else {
            return self.entries.clone();
        };
        let rows: Vec<ActivityEntry> = self
            .entries
            .iter()
            .filter(|entry| wanted.contains(&entry.hash))
            .cloned()
            .collect();
        assert_eq!(rows.len(), wanted.len(), "{}", text(case, "name"));
        rows
    }

    // The extension's `pricing` is either the tracked tokens or absent.
    fn tracked(&self, case: &JsValue) -> &[TokenConfig] {
        if at(case, "tracked") == &JsValue::Bool(true) {
            &self.tracked
        } else {
            &[]
        }
    }
}

// `"all"` is `None`.
fn axis(filter: &JsValue, key: &str) -> Option<String> {
    Some(text(filter, key)).filter(|value| value != "all")
}

fn filter(value: &JsValue) -> ActivityFilter {
    ActivityFilter {
        action: axis(value, "action"),
        query: text(value, "query"),
        sort: match text(value, "sort").as_str() {
            "amount" => ActivitySort::Amount,
            "date" => ActivitySort::Date,
            other => panic!("unknown sort {other:?}"),
        },
        status: axis(value, "status").map(|status| parse_status(&status).unwrap()),
        token: axis(value, "token"),
    }
}

fn options_json(options: &ActivityFilterOptions) -> serde_json::Value {
    serde_json::json!({
        "actions": options.actions,
        "statuses": options.statuses.iter().map(|s| status_text(*s)).collect::<Vec<_>>(),
        "tokens": options
            .tokens
            .iter()
            .map(|t| serde_json::json!({"key": t.key, "label": t.label}))
            .collect::<Vec<_>>(),
    })
}

fn summary_json(summary: &ActivityPartySummary) -> serde_json::Value {
    serde_json::json!({
        "count": summary.count,
        "firstAt": summary.first_at,
        "label": summary.label,
        "lastAt": summary.last_at,
        "totals": summary
            .totals
            .iter()
            .map(|t| serde_json::json!({"received": t.received, "sent": t.sent, "symbol": t.symbol}))
            .collect::<Vec<_>>(),
    })
}

fn json(value: &JsValue) -> serde_json::Value {
    serde_json::from_str(&value.to_json()).unwrap()
}

#[test]
fn the_export_is_activity_to_csv_cell_for_cell() {
    let feed = feed();
    for case in cases("csv") {
        assert_eq!(
            activity_to_csv(&feed.rows(&case), networks::all()),
            text(&case, "csv"),
            "{}",
            text(&case, "name")
        );
    }
}

#[test]
fn the_controls_offer_what_activity_filter_options_offers() {
    let feed = feed();
    for case in cases("options") {
        let options = activity_filter_options(&feed.rows(&case), feed.tracked(&case));
        assert_eq!(
            options_json(&options),
            json(at(&case, "options")),
            "{}",
            text(&case, "name")
        );
    }
}

#[test]
fn each_filter_keeps_sorts_and_summarises_as_the_feed_does() {
    let feed = feed();
    for case in cases("view") {
        let name = text(&case, "name");
        let rows = feed.rows(&case);
        let tracked = feed.tracked(&case);
        let options = activity_filter_options(&rows, tracked);
        let reconciled = reconcile_activity_filter(filter(at(&case, "filter")), &options);
        assert_eq!(
            reconciled,
            filter(at(&case, "reconciled")),
            "{name}: reconciled"
        );
        let visible = filter_activity(rows, &reconciled, tracked, &feed.contacts);
        #[cfg(feature = "mobile")]
        let exported = bramble_core::activity_party_summary(
            visible.clone(),
            reconciled.clone(),
            tracked.to_vec(),
            feed.contacts.clone(),
        );
        let kept: Vec<String> = visible.iter().map(|entry| entry.hash.clone()).collect();
        assert_eq!(kept, hashes(at(&case, "visible")), "{name}: visible");
        let summary = activity_party_summary(&visible, tracked, &feed.contacts);
        assert_eq!(
            summary.as_ref().map(summary_json),
            Some(json(at(&case, "summary"))).filter(|s| !s.is_null()),
            "{name}: summary"
        );
        // bramble-core's export adds `activity-ui.ts`'s gate: no summary while the search is blank.
        #[cfg(feature = "mobile")]
        assert_eq!(
            exported,
            summary
                .clone()
                .filter(|_| !js_trim(&reconciled.query).is_empty()),
            "{name}: exported summary"
        );
        let parts = summary.as_ref().map(activity_party_summary_parts);
        let expected = at(&case, "parts");
        assert_eq!(
            parts.map(|p| (p.label, p.body)),
            (expected != &JsValue::Null).then(|| (text(expected, "label"), text(expected, "body"))),
            "{name}: parts"
        );
    }
}
