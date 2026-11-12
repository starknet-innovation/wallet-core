//! Unit tests for the activity store (kept apart to hold `store.rs` under 500 lines), over an
//! in-memory `Storage`.

use super::*;
use crate::host::testing::Memory;

const SCOPE: &str = "SN_SEPOLIA:0x208361e3911b1d93a704400b88e7e53a89171fce64dbb48f8479e4f44b398d9";

fn entry(hash: &str, at: u64) -> Entry {
    Entry {
        account_id: SCOPE.into(),
        amount: Some("1.5".into()),
        counterparty: Some("0x1".into()),
        direction: None,
        failure_notice: None,
        finalized_on_l1: None,
        hash: hash.into(),
        kind: "transfer".into(),
        network_id: "SN_SEPOLIA".into(),
        not_found_count: None,
        status: "pending".into(),
        submitted_at: at,
        summary: "Send 1.5 STRK to 0x1…0x1".into(),
        token: Some("STRK".into()),
        token_address: None,
    }
}

fn found(status: TxStatus) -> Observation {
    Observation::Found {
        status,
        accepted_finality: false,
        finalized_on_l1: false,
    }
}

#[test]
fn round_trip_dedup_and_cap() {
    let storage = Memory::default();
    let mut log = ActivityLog::new(&storage);
    assert!(log.record(entry("0xab", 1)));
    assert!(log.record(entry("0x0ab", 2)), "same felt is kept once");
    assert_eq!(log.list(SCOPE).len(), 1);
    for n in 0..MAX_ENTRIES_PER_SCOPE as u64 + 5 {
        log.record(entry(&format!("{:#x}", n + 0x1000), n + 10));
    }
    let listed = log.list(SCOPE);
    assert_eq!(listed.len(), MAX_ENTRIES_PER_SCOPE);
    assert!(listed
        .windows(2)
        .all(|w| w[0].submitted_at >= w[1].submitted_at));
    assert!(listed.iter().all(|e| e.hash != "0xab"), "oldest dropped");
}

#[test]
fn status_moves_forward_and_not_found_rejects() {
    let storage = Memory::default();
    let mut log = ActivityLog::new(&storage);
    let hash = Felt::from(0xabu8);
    log.record(entry("0xab", 1));
    assert_eq!(
        log.observe(SCOPE, &hash, found(TxStatus::Accepted)),
        Some(TxStatus::Accepted)
    );
    assert_eq!(
        log.observe(SCOPE, &hash, found(TxStatus::Pending)),
        Some(TxStatus::Accepted)
    );
    log.record(entry("0xcd", 2));
    let other = Felt::from(0xcdu8);
    assert_eq!(
        log.observe(SCOPE, &other, Observation::NotFound),
        Some(TxStatus::Pending)
    );
    assert_eq!(
        log.observe(SCOPE, &other, found(TxStatus::Pending)),
        Some(TxStatus::Pending)
    );
    for expected in [TxStatus::Pending, TxStatus::Pending, TxStatus::Rejected] {
        assert_eq!(
            log.observe(SCOPE, &other, Observation::NotFound),
            Some(expected)
        );
    }
    assert_eq!(
        log.observe(SCOPE, &other, found(TxStatus::Accepted)),
        Some(TxStatus::Rejected)
    );
    assert_eq!(
        log.observe(SCOPE, &Felt::from(9u8), found(TxStatus::Accepted)),
        None
    );
}

// Mobile's version of this test also checks that wallet deletion removes every activity file;
// deletion is the host's (`Storage` has no delete), so that half stays with mobile's `remove`.
#[test]
fn corrupt_file_is_quarantined_not_overwritten() {
    let storage = Memory::default();
    let mut log = ActivityLog::new(&storage);
    let keys = |storage: &Memory| storage.0.borrow().keys().cloned().collect::<Vec<_>>();
    storage.write(ACTIVITY_FILE, b"{not json").unwrap();
    assert!(log.list(SCOPE).is_empty());
    assert_eq!(
        storage
            .read("activity.v1.corrupt-0.json")
            .unwrap()
            .as_deref(),
        Some(&b"{not json"[..])
    );
    storage
        .write(ACTIVITY_FILE, br#"{"version":2,"scopes":{}}"#)
        .unwrap();
    assert!(log.record(entry("0xab", 1)));
    assert!(storage
        .read("activity.v1.corrupt-1.json")
        .unwrap()
        .is_some());
    assert_eq!(log.list(SCOPE).len(), 1);
    assert_eq!(
        keys(&storage),
        [
            "activity.v1.corrupt-0.json",
            "activity.v1.corrupt-1.json",
            ACTIVITY_FILE
        ]
    );
    // Over the size bound, even when it parses.
    let mut big = br#"{"version":1,"scopes":{}}"#.to_vec();
    big.resize(MAX_FILE_BYTES as usize + 1, b' ');
    storage.write(ACTIVITY_FILE, &big).unwrap();
    assert!(log.list(SCOPE).is_empty());
    assert!(storage
        .read("activity.v1.corrupt-2.json")
        .unwrap()
        .is_some());
    assert!(storage.read(ACTIVITY_FILE).unwrap().is_none());
}

// Storage that cannot move files and, when `.1`, cannot read them either.
struct Stuck(Memory, bool);
impl Storage for Stuck {
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, crate::Error> {
        if self.1 {
            return Err(crate::Error::storage("read"));
        }
        self.0.read(key)
    }
    fn write(&self, key: &str, value: &[u8]) -> Result<(), crate::Error> {
        self.0.write(key, value)
    }
    fn rename(&self, _: &str, _: &str) -> Result<(), crate::Error> {
        Err(crate::Error::storage("rename"))
    }
}

#[test]
fn a_file_that_cannot_be_set_aside_is_never_overwritten() {
    for failing_reads in [false, true] {
        let storage = Stuck(Memory::default(), failing_reads);
        storage.0.write(ACTIVITY_FILE, b"{not json").unwrap();
        let mut log = ActivityLog::new(&storage);
        assert!(log.list(SCOPE).is_empty());
        assert!(!log.record(entry("0xab", 1)));
        log.mark_unavailable(SCOPE, 5);
        assert_eq!(
            storage.0.read(ACTIVITY_FILE).unwrap().as_deref(),
            Some(&b"{not json"[..])
        );
    }
}

#[test]
fn invalid_entries_are_dropped() {
    let storage = Memory::default();
    let mut log = ActivityLog::new(&storage);
    let mut bad = entry("0xab", 1);
    bad.summary = String::new();
    assert!(!log.record(bad));
    let mut wrong_scope = entry("0xab", 1);
    wrong_scope.network_id = "SN_MAIN".into();
    assert!(!log.record(wrong_scope));
    assert!(storage.read(ACTIVITY_FILE).unwrap().is_none());
}

#[test]
fn an_expired_unresolved_entry_is_retired_once_and_its_notice_delivered_once() {
    use super::observation::UNRESOLVED_WINDOW_MS as HOUR;
    let storage = Memory::default();
    let mut log = ActivityLog::new(&storage);
    log.record(entry("0xab", 1_000));
    log.record(entry("0xcd", 1_000 + HOUR));
    let mut accepted = entry("0xef", 1_000);
    accepted.status = "accepted".into();
    log.record(accepted);

    assert_eq!(
        log.retire_expired(SCOPE, 1_000 + HOUR - 1),
        0,
        "inside the hour"
    );
    assert!(log.take_failure_notices(SCOPE).is_empty());
    assert_eq!(log.retire_expired(SCOPE, 1_000 + HOUR), 1);
    assert_eq!(log.retire_expired(SCOPE, 1_000 + HOUR), 0, "retired once");

    let retired = log.list(SCOPE);
    let retired = retired.iter().find(|e| e.hash == "0xab").unwrap();
    assert_eq!(retired.status, "pending", "the status is never invented");
    assert_eq!(retired.failure_notice.as_deref(), Some("pending"));

    let taken = log.take_failure_notices(SCOPE);
    assert_eq!(taken.len(), 1);
    assert_eq!(taken[0].hash, "0xab");
    assert!(log.take_failure_notices(SCOPE).is_empty(), "delivered once");
    assert_eq!(
        log.retire_expired(SCOPE, u64::MAX),
        1,
        "only 0xcd is left to retire"
    );
    assert!(log.take_failure_notices("SN_SEPOLIA:0x1").is_empty());
}

#[test]
fn notices_are_drained_at_most_five_at_a_time() {
    let storage = Memory::default();
    let mut log = ActivityLog::new(&storage);
    for n in 1..=7u8 {
        log.record(entry(&format!("0x{n:x}"), 1));
    }
    assert_eq!(log.retire_expired(SCOPE, u64::MAX), 7);
    assert_eq!(log.take_failure_notices(SCOPE).len(), 5);
    assert_eq!(log.take_failure_notices(SCOPE).len(), 2);
    assert!(log.take_failure_notices(SCOPE).is_empty());
}

#[test]
fn an_observation_reaches_every_leg_of_a_transaction() {
    use crate::history::activity::{ActivityEntry, ActivityKind};
    let storage = Memory::default();
    let mut log = ActivityLog::new(&storage);
    let leg = |token: &str| ActivityEntry {
        account_id: SCOPE.into(),
        amount: Some("1".into()),
        counterparty: Some("0x5".into()),
        direction: None,
        failure_notice: None,
        finalized_on_l1: None,
        hash: "0xab".into(),
        kind: ActivityKind::Transfer,
        network: crate::NetworkId::Sepolia,
        not_found_count: None,
        status: TxStatus::Accepted,
        submitted_at: 1,
        summary: format!("Received 1 {token} from 0x5"),
        token: Some(token.into()),
        token_address: None,
    };
    // A swap's two legs share one hash and are stored as two rows.
    log.merge(SCOPE, &[leg("STRK"), leg("ETH")], 1);
    assert_eq!(log.list(SCOPE).len(), 2);
    let settled = Observation::Found {
        status: TxStatus::Accepted,
        accepted_finality: true,
        finalized_on_l1: true,
    };
    assert_eq!(
        log.observe(SCOPE, &Felt::from(0xabu8), settled),
        Some(TxStatus::Accepted)
    );
    assert!(log
        .list(SCOPE)
        .iter()
        .all(|e| e.finalized_on_l1 == Some(true)));
}
