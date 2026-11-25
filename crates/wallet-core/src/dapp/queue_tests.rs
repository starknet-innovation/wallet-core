//! Ported from the extension's `tests/background/approvals/{approvals,approval-queue-audit}
//! .test.ts` and `tests/background/approval-expiry-waiter-release.test.ts`, the queue's own rules.
//! The payload stands for a card; the tests use its site's name.

use super::*;

const LIMITS: QueueLimits = QueueLimits {
    max_pending: 50,
    max_per_origin: 5,
    ttl_ms: 5 * 60 * 1_000,
    max_waiters: 8,
};

const VICTIM: &str = "https://victim.example";
const NEWCOMER: &str = "https://newcomer.example";
const T0: u64 = 1_000_000;

fn site(index: usize) -> String {
    format!("https://app-{index}.example")
}

/// A connect request: deduped per site.
fn connect(
    queue: &mut ApprovalQueue<&'static str>,
    origin: &str,
    now: u64,
) -> Result<Queued, Refusal> {
    queue
        .enqueue(origin, Some("connect".into()), "connect", now, None)
        .unwrap()
}

/// An invoke request: never deduped.
fn invoke(
    queue: &mut ApprovalQueue<&'static str>,
    origin: &str,
    now: u64,
) -> Result<Queued, Refusal> {
    queue.enqueue(origin, None, "invoke", now, None).unwrap()
}

// "deduplicates requests by origin"
#[test]
fn a_sites_second_connect_joins_its_first() {
    let mut queue = ApprovalQueue::new(LIMITS);
    let first = connect(&mut queue, VICTIM, T0).unwrap();
    let second = connect(&mut queue, VICTIM, T0).unwrap();
    assert!(!first.joined);
    assert!(second.joined);
    assert_eq!(second.id, first.id);
    assert_eq!(queue.list(T0).len(), 1);
    // Another site, or another key, is its own approval.
    assert_ne!(connect(&mut queue, NEWCOMER, T0).unwrap().id, first.id);
    let switch = queue
        .enqueue(VICTIM, Some("switch:0x1".into()), "switch", T0, None)
        .unwrap()
        .unwrap();
    assert_ne!(switch.id, first.id);
    // Invokes never dedupe.
    let a = invoke(&mut queue, VICTIM, T0).unwrap();
    let b = invoke(&mut queue, VICTIM, T0).unwrap();
    assert_ne!(a.id, b.id);
}

// `MAX_WAITERS_PER_APPROVAL`: at most eight requests wait on one approval.
#[test]
fn at_most_eight_requests_wait_on_one_approval() {
    let mut queue = ApprovalQueue::new(LIMITS);
    for _ in 0..LIMITS.max_waiters {
        connect(&mut queue, VICTIM, T0).unwrap();
    }
    assert_eq!(connect(&mut queue, VICTIM, T0), Err(Refusal::QueueFull));
}

// "claims requests exactly once"
#[test]
fn an_approval_is_claimed_once() {
    let mut queue = ApprovalQueue::new(LIMITS);
    let approved = connect(&mut queue, "https://approve.example", T0).unwrap();
    let denied = connect(&mut queue, "https://deny.example", T0).unwrap();
    let claimed = queue.claim(&approved.id, T0).unwrap();
    assert_eq!(claimed.origin, "https://approve.example");
    assert_eq!(claimed.queued_at, T0);
    assert_eq!(queue.claim(&approved.id, T0), None);
    assert!(queue.claim(&denied.id, T0).is_some());
    assert!(queue.list(T0).is_empty());
    assert_eq!(queue.claim(&ApprovalId("unknown".into()), T0), None);
}

#[test]
fn ids_are_random_and_distinct() {
    let mut queue = ApprovalQueue::new(LIMITS);
    let a = invoke(&mut queue, VICTIM, T0).unwrap().id;
    let b = invoke(&mut queue, VICTIM, T0).unwrap().id;
    assert_ne!(a, b);
    assert_eq!(a.as_str().len(), 32);
    assert!(a.as_str().bytes().all(|b| b.is_ascii_hexdigit()));
}

// "count against the per-origin cap", per-origin refusal.
#[test]
fn a_site_holds_at_most_five() {
    let mut queue = ApprovalQueue::new(LIMITS);
    for _ in 0..LIMITS.max_per_origin {
        invoke(&mut queue, VICTIM, T0).unwrap();
    }
    assert_eq!(invoke(&mut queue, VICTIM, T0), Err(Refusal::SiteFull));
    assert_eq!(queue.can_admit(VICTIM, T0), Err(Refusal::SiteFull));
    assert_eq!(queue.can_admit(NEWCOMER, T0), Ok(()));
}

// "caps pending approvals without breaking origin deduplication", and "colluding origins holding
// one prompt each cannot drop another site's prompt" (Sherlock 128).
#[test]
fn a_full_queue_of_single_prompts_refuses_a_newcomer() {
    let mut queue = ApprovalQueue::new(LIMITS);
    let reviewing = connect(&mut queue, VICTIM, T0).unwrap();
    for index in 1..LIMITS.max_pending {
        connect(&mut queue, &site(index), T0).unwrap();
    }
    assert_eq!(connect(&mut queue, VICTIM, T0).unwrap().id, reviewing.id);
    assert_eq!(queue.can_admit(NEWCOMER, T0), Err(Refusal::QueueFull));
    assert_eq!(connect(&mut queue, NEWCOMER, T0), Err(Refusal::QueueFull));
    assert_eq!(queue.list(T0).len(), LIMITS.max_pending);
    assert!(queue.get(&reviewing.id, T0).is_some());
    assert!(queue.take_dropped().is_empty());
}

// "a new origin still displaces the oldest prompt of an origin holding several"
#[test]
fn a_newcomer_displaces_the_oldest_prompt_of_the_largest_holder() {
    let mut queue = ApprovalQueue::new(LIMITS);
    let reviewing = connect(&mut queue, VICTIM, T0).unwrap();
    let heavy = "https://heavy.example";
    let heavy_first = invoke(&mut queue, heavy, T0).unwrap();
    let heavy_second = invoke(&mut queue, heavy, T0).unwrap();
    for index in 3..LIMITS.max_pending {
        connect(&mut queue, &site(index), T0).unwrap();
    }
    assert_eq!(queue.can_admit(NEWCOMER, T0), Ok(()));
    let admitted = connect(&mut queue, NEWCOMER, T0).unwrap();
    assert!(queue.get(&admitted.id, T0).is_some());
    assert!(queue.get(&heavy_first.id, T0).is_none());
    assert!(queue.get(&heavy_second.id, T0).is_some());
    assert!(queue.get(&reviewing.id, T0).is_some());
    assert_eq!(queue.take_dropped(), vec![heavy_first.id]);
    // A site already in the full queue does not evict on its own behalf.
    assert!(connect(&mut queue, &site(3), T0).unwrap().joined);
    assert_eq!(invoke(&mut queue, &site(3), T0), Err(Refusal::QueueFull));
}

// "admission reservations (Sherlock 151)"
#[test]
fn reservations_count_until_released() {
    let mut queue: ApprovalQueue<&str> = ApprovalQueue::new(LIMITS);
    let mut held: Vec<Reservation> = (0..LIMITS.max_per_origin)
        .map(|_| queue.reserve(VICTIM, T0).unwrap())
        .collect();
    assert_eq!(queue.can_admit(VICTIM, T0), Err(Refusal::SiteFull));
    assert_eq!(queue.reserve(VICTIM, T0).unwrap_err(), Refusal::SiteFull);
    assert_eq!(queue.can_admit(NEWCOMER, T0), Ok(()));
    // A reservation is released once: release takes it.
    queue.release(held.pop().unwrap());
    assert_eq!(queue.can_admit(VICTIM, T0), Ok(()));
    let again = queue.reserve(VICTIM, T0).unwrap();
    assert_eq!(queue.reserve(VICTIM, T0).unwrap_err(), Refusal::SiteFull);
    queue.release(again);
    for reservation in held {
        queue.release(reservation);
    }
    assert!(queue.list(T0).is_empty());
    assert_eq!(queue.can_admit(VICTIM, T0), Ok(()));
}

// "hand over to the queued prompt with no moment counted twice": enqueue does not count
// reservations, since the request being queued holds one.
#[test]
fn a_reserved_request_queues_without_counting_twice() {
    let mut queue = ApprovalQueue::new(LIMITS);
    for _ in 0..LIMITS.max_per_origin - 2 {
        invoke(&mut queue, VICTIM, T0).unwrap();
    }
    let reservation = queue.reserve(VICTIM, T0).unwrap();
    invoke(&mut queue, VICTIM, T0).unwrap();
    queue.release(reservation);
    assert!(queue.reserve(VICTIM, T0).is_ok());
}

// Reservations count towards the largest share at the global cap, as `admissionView` lists them.
#[test]
fn a_reserving_site_counts_towards_the_largest_share() {
    let mut queue = ApprovalQueue::new(LIMITS);
    for index in 0..LIMITS.max_pending - 2 {
        connect(&mut queue, &site(index), T0).unwrap();
    }
    let _first = queue.reserve(NEWCOMER, T0).unwrap();
    let _second = queue.reserve(NEWCOMER, T0).unwrap();
    // 48 single prompts and two reservations from one site: a third site may still get in.
    assert_eq!(queue.can_admit("https://third.example", T0), Ok(()));
}

// Expiry (`APPROVAL_TTL_MS`, `approval-expiry-waiter-release.test.ts`).
#[test]
fn approvals_expire_after_five_minutes() {
    let mut queue = ApprovalQueue::new(LIMITS);
    let old = connect(&mut queue, VICTIM, T0).unwrap();
    let late = T0 + 60_000;
    let newer = connect(&mut queue, NEWCOMER, late).unwrap();
    // Still live a millisecond before the TTL.
    assert!(queue.get(&old.id, T0 + LIMITS.ttl_ms - 1).is_some());
    assert!(queue.take_dropped().is_empty());
    // Gone at the TTL: never seen, never claimed, and reported once for the host to answer.
    assert!(queue.get(&old.id, T0 + LIMITS.ttl_ms).is_none());
    assert_eq!(queue.take_dropped(), vec![old.id.clone()]);
    assert!(queue.take_dropped().is_empty());
    assert_eq!(queue.claim(&old.id, T0 + LIMITS.ttl_ms), None);
    assert!(queue.get(&newer.id, T0 + LIMITS.ttl_ms).is_some());
    // An expired approval no longer holds its site's place or dedupe key.
    let fresh = connect(&mut queue, VICTIM, T0 + LIMITS.ttl_ms).unwrap();
    assert!(!fresh.joined);
    assert_ne!(fresh.id, old.id);
}

// `queuedAt: Math.min(arrivedAt ?? at, at)`: a request dated by its arrival expires on time.
#[test]
fn an_arrival_time_dates_the_approval_never_later_than_now() {
    let mut queue = ApprovalQueue::new(LIMITS);
    let early = queue
        .enqueue(VICTIM, None, "invoke", T0 + 10, Some(T0))
        .unwrap()
        .unwrap();
    assert_eq!(queue.get(&early.id, T0 + 10).unwrap().queued_at, T0);
    let future = queue
        .enqueue(VICTIM, None, "invoke", T0 + 10, Some(T0 + 99))
        .unwrap()
        .unwrap();
    assert_eq!(queue.get(&future.id, T0 + 10).unwrap().queued_at, T0 + 10);
}

// `beginFeeEstimate` and `updateFee`: only the newest estimate is written.
#[test]
fn only_the_newest_fee_estimate_is_written() {
    let mut queue: ApprovalQueue<String> = ApprovalQueue::new(LIMITS);
    let id = queue
        .enqueue(VICTIM, None, "no fee".into(), T0, None)
        .unwrap()
        .unwrap()
        .id;
    let first = queue.begin_fee_estimate(&id, T0).unwrap();
    let second = queue.begin_fee_estimate(&id, T0).unwrap();
    assert!(!queue.update_fee(&first, T0, |card| *card = "stale".into()));
    assert!(queue.update_fee(&second, T0, |card| *card = "fresh".into()));
    assert_eq!(queue.get(&id, T0).unwrap().payload, "fresh");
    queue.claim(&id, T0);
    assert!(!queue.update_fee(&second, T0, |card| *card = "late".into()));
    assert_eq!(queue.begin_fee_estimate(&id, T0), None);
}
