// `allowance-scan.test.ts`, `allowance-scan-edges.test.ts`, `allowance-scan-owner-filter.test.ts`
// and `allowance-scan-legacy-window.test.ts`, against `scan_node::Node`, plus the reply and bound
// branches they leave out.

use serde_json::{json, Value};

use super::discovery::{ExternalAllowance, APPROVAL_EVENT_KEY};
use super::scan::*;
use super::scan_node::*;
use super::{AllowancePolicy, TEST_POLICY};
use crate::portfolio::wire::RpcReply;
use crate::send::u256::U256;

// Drives a scan to its end; `None` when it reported a transport failure.
fn drive(scan: &mut ExternalAllowanceScan, node: &Node) -> Option<ExternalAllowanceScanResult> {
    loop {
        let calls = scan.next_calls();
        if calls.is_empty() {
            return scan.finish().ok();
        }
        for call in calls {
            if scan.accept(node.answer(&call)) == ScanStep::Transport {
                return None;
            }
        }
    }
}

fn scan_with(
    node: &Node,
    tokens: &[&str],
    policy: AllowancePolicy,
) -> Option<ExternalAllowanceScanResult> {
    let tokens: Vec<String> = tokens.iter().map(|t| t.to_string()).collect();
    drive(
        &mut ExternalAllowanceScan::new(OWNER, &tokens, policy),
        node,
    )
}

fn run(node: &Node) -> ExternalAllowanceScanResult {
    scan_with(node, &[TOKEN], TEST_POLICY).unwrap()
}

fn found(spender: &str, amount: U256) -> ExternalAllowance {
    ExternalAllowance {
        token: canonical(TOKEN),
        spender: canonical(spender),
        amount,
    }
}

#[test]
fn discovers_spenders_across_pages_and_reads_live_amounts() {
    // allowance-scan.test.ts "discovers spenders across pages, reads live amounts, and filters
    // owner and zero" (its 100-block chain; one Cairo 1 and one legacy approval read live).
    let (spender, zero, legacy_spender) = ("0x05abcdef", "0x07feed", "0x09aa");
    let mut chain = node(
        100,
        vec![
            cairo1(OWNER, spender, 1),
            cairo1(OWNER, zero, 2),
            cairo1("0x9999", spender, 3),
            legacy(OWNER, legacy_spender, 4),
        ],
    );
    chain.amounts = vec![
        (canonical(spender), ["0x3e8".into(), "0x0".into()]),
        (canonical(legacy_spender), ["0x0".into(), "0x1".into()]),
        (canonical(zero), ["0x0".into(), "0x0".into()]),
    ];
    let result = run(&chain);
    let expected = vec![
        found(spender, U256::from_u128(1000)),
        found(legacy_spender, U256 { high: 1, low: 0 }),
    ];
    assert_eq!(
        result,
        ExternalAllowanceScanResult {
            allowances: expected,
            complete: true
        }
    );
}

#[test]
fn nothing_to_scan_is_empty_and_complete_without_a_call() {
    // "returns nothing for an invalid owner"; "short-circuits to an empty list when no tokens are
    // tracked"; beyond the tests, a token list of non-felts.
    let empty = ExternalAllowanceScanResult {
        allowances: vec![],
        complete: true,
    };
    let tokens = vec![TOKEN.to_string()];
    for (owner, tokens) in [
        ("0x0", tokens.clone()),
        (OWNER, vec![]),
        (OWNER, vec!["0x0".into(), "nope".into()]),
    ] {
        let mut scan = ExternalAllowanceScan::new(owner, &tokens, TEST_POLICY);
        assert!(scan.next_calls().is_empty());
        assert_eq!(scan.finish().unwrap(), empty);
    }
}

#[test]
fn the_owner_read_reaches_past_a_busy_log() {
    // owner-filter.test.ts "finds the owner's approval behind 600 earlier approvals by others".
    let mut log = strangers(600, 1, cairo1);
    log.push(cairo1(OWNER, &drainer(), 700));
    let chain = node(1_000, log);
    let result = run(&chain);
    assert_eq!(result.allowances, [found(&drainer(), unlimited_low())]);
    assert!(result.complete);
    let first = chain.filters.borrow()[0].clone();
    assert_eq!(
        first["keys"],
        json!([[APPROVAL_EVENT_KEY], [canonical(OWNER)]])
    );
    assert!(first.get("from_block").is_none());
    // "is incomplete when the owner's own approvals outrun the budget".
    let owned: Vec<Event> = (0..600)
        .map(|i| cairo1(OWNER, &format!("0x{:x}", 0x7000 + i), i as u64))
        .collect();
    assert!(!run(&node(1_000, owned)).complete);
}

#[test]
fn a_legacy_token_is_read_in_windows_back_from_the_head() {
    // owner-filter.test.ts "reads a legacy token's log in full when it fits the budget".
    let mut log = strangers(50, 1, legacy);
    log.push(legacy(OWNER, &drainer(), 60));
    let result = run(&node(100, log));
    assert_eq!(result.allowances, [found(&drainer(), unlimited_low())]);
    assert!(result.complete);
    // "is incomplete when a legacy token's log is longer than the budget".
    let mut log = strangers(600, 1, legacy);
    log.push(legacy(OWNER, &drainer(), 700));
    let result = run(&node(1_000, log));
    assert_eq!((result.allowances.len(), result.complete), (0, false));
}

const HEAD: u64 = 1_000_000;
const WINDOW: u64 = 200_000;

#[test]
fn legacy_windows_end_where_the_token_turned_cairo1() {
    // legacy-window.test.ts "Cairo 0 approvals before an upgrade no longer mark every scan
    // incomplete": the ETH shape.
    let mut log = strangers(600, 0, legacy);
    log.extend(strangers(400, HEAD - 50_000, cairo1));
    log.push(cairo1(OWNER, &drainer(), HEAD - 10));
    let chain = node(HEAD, log);
    let result = run(&chain);
    assert_eq!(result.allowances, [found(&drainer(), unlimited_low())]);
    assert!(result.complete);
    let filters = chain.filters.borrow();
    let unfiltered: Vec<&Value> = filters
        .iter()
        .filter(|f| f["keys"].as_array().unwrap().len() == 1)
        .collect();
    assert!(!unfiltered.is_empty());
    for filter in unfiltered {
        assert_eq!(
            filter["from_block"]["block_number"],
            json!(HEAD - WINDOW + 1)
        );
        assert_eq!(filter["to_block"]["block_number"], json!(HEAD));
    }
}

#[test]
fn legacy_windows_find_recent_approvals_and_say_what_they_missed() {
    // "finds a recent legacy approval behind 600 older ones": incomplete, since the rest of the
    // log from block 0 ran out of pages on legacy events.
    let mut log = strangers(600, 0, legacy);
    log.push(legacy(OWNER, &drainer(), HEAD - 10));
    let chain = node(HEAD, log);
    let result = run(&chain);
    assert_eq!(result.allowances, [found(&drainer(), unlimited_low())]);
    assert!(!result.complete);
    // Three windows walked back, then the rest from block 0 to just below the last window.
    let ranges: Vec<(u64, u64)> = chain
        .filters
        .borrow()
        .iter()
        .filter(|f| {
            f["keys"].as_array().unwrap().len() == 1 && f.get("continuation_token").is_none()
        })
        .map(|f| {
            (
                f["from_block"]["block_number"].as_u64().unwrap(),
                f["to_block"]["block_number"].as_u64().unwrap(),
            )
        })
        .collect();
    let edge = HEAD - 3 * WINDOW;
    assert_eq!(
        ranges,
        [
            (HEAD - WINDOW + 1, HEAD),
            (HEAD - 2 * WINDOW + 1, HEAD - WINDOW),
            (edge + 1, HEAD - 2 * WINDOW),
            (0, edge),
        ]
    );
    // "reads a quiet legacy token to its first block".
    let mut log = vec![legacy(OWNER, &drainer(), 5)];
    log.extend(strangers(20, 100, legacy));
    let result = run(&node(HEAD, log));
    assert_eq!((result.allowances.len(), result.complete), (1, true));
    // "is incomplete when a recent window holds more legacy events than the budget".
    assert!(!run(&node(HEAD, strangers(400, HEAD - 1_000, legacy))).complete);
    // Beyond the tests: a head inside the first window reads from block 0 and stops there.
    let chain = node(150_000, vec![legacy(OWNER, &drainer(), 3)]);
    let result = run(&chain);
    assert_eq!((result.allowances.len(), result.complete), (1, true));
    let last = chain.filters.borrow().last().cloned().unwrap();
    assert_eq!(last["from_block"]["block_number"], json!(0));
}

#[test]
fn a_node_error_skips_the_token_as_incomplete() {
    // legacy-window.test.ts "is incomplete when a window read fails": the owner read found the
    // drainer, but the token's whole result goes.
    let mut chain = node(HEAD, vec![cairo1(OWNER, &drainer(), HEAD - 10)]);
    chain.fault = Some(|body| {
        body["params"]["filter"]
            .get("to_block")
            .map(|_| semantic(0))
    });
    assert_eq!(run(&chain), ExternalAllowanceScanResult::unavailable());
    // allowance-scan.test.ts "skips a token whose event query fails without a transport error";
    // edges.test.ts "accumulates per token in token order, skipping a token that yields no
    // spenders", with the failing token first: the next token still counts.
    let mut chain = node(100, vec![cairo1(OWNER, "0x05abcdef", 1)]);
    chain.amounts = vec![(canonical("0x05abcdef"), ["0x64".into(), "0x0".into()])];
    chain.fault = Some(|body| {
        (body["params"]["filter"]["address"] == json!("0x511a2b3")).then(|| semantic(0))
    });
    let result = scan_with(&chain, &["0x0511a2b3", TOKEN], TEST_POLICY).unwrap();
    assert_eq!(
        result.allowances,
        [found("0x05abcdef", U256::from_u128(100))]
    );
    assert!(!result.complete);
    // A node error on the allowance read drops the token too.
    let mut chain = node(100, vec![cairo1(OWNER, &drainer(), 1)]);
    chain.fault = Some(|body| (body["method"] == json!("starknet_call")).then(|| semantic(0)));
    assert_eq!(run(&chain), ExternalAllowanceScanResult::unavailable());
}

#[test]
fn the_spender_cap_bounds_the_reads_and_marks_the_rest() {
    // edges.test.ts "stops discovering spenders at the per-token cap of 50, as incomplete".
    let spenders: Vec<String> = (0..60).map(|i| format!("0x{:x}", 0x1000 + i)).collect();
    let mut chain = node(100, spenders.iter().map(|s| cairo1(OWNER, s, 1)).collect());
    chain.amounts = spenders
        .iter()
        .map(|s| (canonical(s), ["0x64".into(), "0x0".into()]))
        .collect();
    let result = run(&chain);
    assert_eq!((result.allowances.len(), result.complete), (50, false));
    // Beyond the tests: exactly at the cap is complete, and a spender approved twice is one.
    let mut chain = node(
        100,
        spenders[..50].iter().map(|s| cairo1(OWNER, s, 1)).collect(),
    );
    chain.log.push(cairo1(OWNER, &spenders[0], 2));
    chain.amounts = spenders
        .iter()
        .map(|s| (canonical(s), ["0x64".into(), "0x0".into()]))
        .collect();
    let result = run(&chain);
    assert_eq!((result.allowances.len(), result.complete), (50, true));
}

#[test]
fn a_page_past_the_chunk_size_stops_at_the_cap() {
    // A node that ignores `chunk_size` answers the owner read with far more distinct spenders
    // than the cap in one page: the reads stop at the cap, and the scan is incomplete.
    let mut chain = node(100, vec![]);
    chain.fault = Some(|b| {
        let owner_read = b["params"]["filter"]["keys"][1].is_array();
        (b["method"] == json!("starknet_getEvents") && owner_read).then(|| {
            let events: Vec<Value> = (0..1_000u64)
                .map(|i| {
                    let spender = format!("0x{:x}", 0x10_0000 + i);
                    json!({ "keys": [APPROVAL_EVENT_KEY, OWNER, spender], "data": [], "block_number": 1 })
                })
                .collect();
            ok(0, json!({ "events": events }))
        })
    });
    assert!(!run(&chain).complete);
}

#[test]
fn de_duplicating_spenders_stops_one_past_the_cap() {
    // Such a page can hold hundreds of thousands of spenders, and de-duplicating all of them was
    // quadratic (about 30 s for 100,000 in a debug build). What bounds the work is that nothing
    // past the cap is read, so that is counted here. A wall-clock limit said the same until the
    // machine was busy, and then failed.
    let cap = TEST_POLICY.max_spenders_per_token;
    let spenders: Vec<String> = (0..100_000).map(|i| format!("0x{i:x}")).collect();
    let read = std::cell::Cell::new(0usize);
    let counted = spenders.iter().inspect(|_| read.set(read.get() + 1));
    let distinct = distinct_spenders(counted, cap);
    // One past the cap is kept, which is how `start_reads` knows the cap was passed.
    assert_eq!(distinct, spenders[..=cap]);
    assert!(read.get() <= cap + 2, "{} spenders read", read.get());
    // A repeat is neither kept twice nor counted towards the cap, in first-seen order.
    let repeated: Vec<String> = ["0x2", "0x1", "0x2", "0x1", "0x3"]
        .map(String::from)
        .to_vec();
    assert_eq!(
        distinct_spenders(repeated.iter(), cap),
        ["0x2", "0x1", "0x3"]
    );
    assert_eq!(distinct_spenders(repeated.iter(), 1), ["0x2", "0x1"]);
}

#[test]
fn a_transport_failure_restarts_the_whole_scan() {
    // edges.test.ts "propagates a transport error rather than skipping the token".
    let mut chain = node(100, vec![cairo1(OWNER, &drainer(), 1)]);
    chain.fault = Some(|_| {
        Some(RpcReply {
            id: 0,
            status: 0,
            body: None,
        })
    });
    let mut scan = ExternalAllowanceScan::new(OWNER, &[TOKEN.into()], TEST_POLICY);
    assert!(drive(&mut scan, &chain).is_none());
    // `restart` replays it on a healthy endpoint, from the first call.
    scan.restart();
    chain.fault = None;
    let result = drive(&mut scan, &chain).unwrap();
    assert_eq!(result.allowances, [found(&drainer(), unlimited_low())]);
    assert!(scan.finish().is_ok());
}

#[test]
fn allowance_scan_ignores_a_late_reply_after_restart() {
    let mut scan = ExternalAllowanceScan::new(OWNER, &[TOKEN.into()], TEST_POLICY);
    let old = scan.next_calls().remove(0);
    assert_eq!(
        scan.accept(RpcReply {
            id: old.id,
            status: 0,
            body: None
        }),
        ScanStep::Transport
    );
    assert!(scan.next_calls().is_empty());
    scan.restart();
    let current = scan.next_calls().remove(0);
    assert!(current.id > old.id);
    let chain = node(100, vec![]);
    assert_eq!(scan.accept(chain.answer(&old)), ScanStep::Continue);
    assert!(scan.next_calls().is_empty());
    assert_eq!(scan.accept(chain.answer(&current)), ScanStep::Continue);
    assert!(drive(&mut scan, &chain).is_some());
}

#[test]
fn malformed_replies_are_transport_failures() {
    let faults: [Fault; 7] = [
        // A null page, events that are not a list, an event without keys or data, a block number
        // that is not one, a null allowance reply, an oversized continuation.
        |b| (b["method"] == json!("starknet_getEvents")).then(|| ok(0, Value::Null)),
        |b| (b["method"] == json!("starknet_getEvents")).then(|| ok(0, json!({ "events": "x" }))),
        |b| {
            (b["method"] == json!("starknet_getEvents"))
                .then(|| ok(0, json!({ "events": [{ "keys": [] }] })))
        },
        |b| {
            (b["method"] == json!("starknet_getEvents")).then(|| ok(0, json!({ "events": [null] })))
        },
        |b| (b["method"] == json!("starknet_blockNumber")).then(|| ok(0, json!("0x10"))),
        |b| (b["method"] == json!("starknet_call")).then(|| ok(0, Value::Null)),
        |b| {
            (b["method"] == json!("starknet_getEvents")).then(|| {
                ok(
                    0,
                    json!({ "events": [], "continuation_token": "x".repeat(5_000) }),
                )
            })
        },
    ];
    for (index, fault) in faults.into_iter().enumerate() {
        let mut chain = node(100, vec![cairo1(OWNER, &drainer(), 1)]);
        chain.fault = Some(fault);
        assert!(
            scan_with(&chain, &[TOKEN], TEST_POLICY).is_none(),
            "fault {index}"
        );
    }
    // A reply for another call, or one after the end, is refused too.
    let mut scan = ExternalAllowanceScan::new(OWNER, &[TOKEN.into()], TEST_POLICY);
    let call = scan.next_calls().remove(0);
    assert_eq!(
        scan.accept(ok(call.id + 1, json!({ "events": [] }))),
        ScanStep::Transport
    );
    assert!(scan.finish().is_err());
    let mut done = ExternalAllowanceScan::new(OWNER, &[], TEST_POLICY);
    assert_eq!(done.accept(ok(1, json!(1))), ScanStep::Transport);
    // An allowance reply that is not a uint256 pair is no allowance, not a failure.
    let mut chain = node(100, vec![cairo1(OWNER, &drainer(), 1)]);
    chain.fault = Some(|b| (b["method"] == json!("starknet_call")).then(|| ok(0, json!(["0x1"]))));
    assert_eq!(
        run(&chain),
        ExternalAllowanceScanResult {
            allowances: vec![],
            complete: true
        }
    );
}

#[test]
fn page_budgets_follow_the_policy() {
    // One page of owner events per token: a second page is left unread, so incomplete.
    let tight = AllowancePolicy {
        owner_event_pages: 1,
        event_chunk_size: 2,
        ..TEST_POLICY
    };
    let owned: Vec<Event> = (0..3)
        .map(|i| cairo1(OWNER, &format!("0x{:x}", 0x7000 + i), i))
        .collect();
    let chain = node(100, owned.clone());
    let result = scan_with(&chain, &[TOKEN], tight).unwrap();
    assert!(!result.complete);
    let events_reads = chain
        .filters
        .borrow()
        .iter()
        .filter(|f| f["keys"].as_array().unwrap().len() == 2)
        .count();
    assert_eq!(events_reads, 1);
    // Two pages read it all.
    let roomy = AllowancePolicy {
        owner_event_pages: 2,
        ..tight
    };
    assert!(
        scan_with(&node(100, owned), &[TOKEN], roomy)
            .unwrap()
            .complete
    );
}
