// `allowance-recording.test.ts` (the extension's root `tests/background/`): what a submitted
// batch files in the log, over in-memory storage, plus the guards it leaves out.

use std::cell::Cell;

use super::calls::record_submitted;
use super::calls_tests::*;
use super::retention::Ranking;
use super::revoke::allowance_target;
use super::store::AllowanceLog;
use super::TEST_POLICY;
use crate::host::testing::Memory;
use crate::host::Storage;
use crate::send::calls::Call;
use crate::send::u256::U256;
use crate::Error;

fn file(log: &mut AllowanceLog, calls: &[Call]) {
    let lists = Lists::standard();
    record_submitted(
        log,
        calls,
        &owner(),
        &lists.known(),
        "0x07ab",
        &Ranking::default(),
    );
}

fn on(entrypoint: &str, low: u128, high: u128) -> Call {
    grant(TOKEN, entrypoint, SPENDER, low, high)
}

type Row = (U256, bool, bool);

fn rows(log: &AllowanceLog) -> Vec<Row> {
    log.list()
        .unwrap()
        .into_iter()
        .map(|r| (r.amount, r.lower_bound, r.revoking))
        .collect()
}

fn row(value: &str, lower_bound: bool, revoking: bool) -> Row {
    (amount(value), lower_bound, revoking)
}

#[test]
fn tracks_both_increase_spellings_so_they_can_be_revoked() {
    for entrypoint in ["increase_allowance", "increaseAllowance"] {
        let storage = Memory::default();
        let mut log = AllowanceLog::new(&storage, TEST_POLICY);
        file(&mut log, &[on(entrypoint, 1000, 0)]);
        let records = log.list().unwrap();
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.amount, amount("1000"));
        assert_eq!(
            (record.token.as_str(), record.spender.as_str()),
            (CANONICAL_TOKEN, CANONICAL_SPENDER)
        );
        assert_eq!(
            allowance_target(&record.id, ACCOUNT),
            Some((f(TOKEN), f(SPENDER)))
        );
    }
}

#[test]
fn records_an_unlimited_increase_alongside_an_approve_in_one_transaction() {
    let storage = Memory::default();
    let mut log = AllowanceLog::new(&storage, TEST_POLICY);
    file(
        &mut log,
        &[
            on("approve", 5, 0),
            grant(TOKEN, "increase_allowance", "0x777", 0, 1),
            on("transfer", 1, 0),
        ],
    );
    let mut amounts: Vec<_> = log
        .list()
        .unwrap()
        .into_iter()
        .map(|r| (r.spender, r.amount))
        .collect();
    amounts.sort();
    assert_eq!(
        amounts,
        [
            (CANONICAL_SPENDER.to_string(), amount("5")),
            ("0x777".to_string(), U256 { high: 1, low: 0 }),
        ]
    );
}

#[test]
fn ercs_semantics_decide_the_stored_amount() {
    type Steps = &'static [(&'static str, u128)];
    let cases: [(Steps, Row); 9] = [
        // "an increase of zero never erases a tracked allowance".
        (
            &[("approve", 1000), ("increase_allowance", 0)],
            row("1000", false, false),
        ),
        // "a zero approve keeps the grant, marked as revoking, until the chain confirms it".
        (
            &[("approve", 1000), ("approve", 0)],
            row("1000", false, true),
        ),
        // "an approve after an increase still replaces the tracked amount".
        (
            &[("increase_allowance", 1000), ("approve", 7)],
            row("7", false, false),
        ),
        // "a decrease subtracts from the tracked total instead of being ignored".
        (
            &[
                ("approve", 1000),
                ("decrease_allowance", 400),
                ("decreaseAllowance", 400),
            ],
            row("200", false, false),
        ),
        // "a decrease that empties the allowance keeps the grant until the chain agrees".
        (
            &[("approve", 1000), ("decrease_allowance", 1000)],
            row("1000", false, true),
        ),
        // "a decrease against an untracked allowance keeps the pair as an unknown amount".
        (&[("decrease_allowance", 5)], row("0", true, false)),
        // "decreasing a lower bound to its floor does not erase the pair".
        (
            &[("increase_allowance", 100), ("decrease_allowance", 100)],
            row("0", true, false),
        ),
        // "an increase with no known base is recorded as a lower bound, not a total".
        (&[("increase_allowance", 1)], row("1", true, false)),
        // "an increase on a tracked exact base stays exact".
        (
            &[("approve", 10), ("increase_allowance", 5)],
            row("15", false, false),
        ),
    ];
    for (steps, expected) in cases {
        let storage = Memory::default();
        let mut log = AllowanceLog::new(&storage, TEST_POLICY);
        for &(entrypoint, low) in steps {
            file(&mut log, &[on(entrypoint, low, 0)]);
        }
        assert_eq!(rows(&log), [expected], "{steps:?}");
    }
}

#[test]
fn a_revoke_mark_carries_the_canonical_transaction_hash() {
    let storage = Memory::default();
    let mut log = AllowanceLog::new(&storage, TEST_POLICY);
    file(&mut log, &[on("approve", 1000, 0)]);
    file(&mut log, &[on("approve", 0, 0)]);
    assert_eq!(log.list().unwrap()[0].revoke_hash.as_deref(), Some("0x7ab"));
}

#[test]
fn increases_accumulate_and_never_reduce_the_tracked_allowance() {
    // "repeated increases accumulate instead of overwriting" (both in one transaction).
    let storage = Memory::default();
    let mut log = AllowanceLog::new(&storage, TEST_POLICY);
    file(
        &mut log,
        &[
            on("increase_allowance", 40, 0),
            on("increaseAllowance", 2, 0),
        ],
    );
    assert_eq!(rows(&log), [row("42", true, false)]);
    // "an increase never reduces the tracked allowance".
    let storage = Memory::default();
    let mut log = AllowanceLog::new(&storage, TEST_POLICY);
    file(&mut log, &[on("approve", 0, 1)]);
    file(&mut log, &[on("increase_allowance", 1, 0)]);
    assert_eq!(log.list().unwrap()[0].amount, U256 { high: 1, low: 1 });
}

#[test]
fn a_lower_bound_stays_one_through_deltas_and_an_approve_resolves_it() {
    let storage = Memory::default();
    let mut log = AllowanceLog::new(&storage, TEST_POLICY);
    file(&mut log, &[on("increase_allowance", 10, 0)]);
    file(&mut log, &[on("increase_allowance", 5, 0)]);
    assert_eq!(rows(&log), [row("15", true, false)]);
    file(&mut log, &[on("decrease_allowance", 5, 0)]);
    assert_eq!(rows(&log), [row("10", true, false)]);
    file(&mut log, &[on("approve", 3, 0)]);
    assert_eq!(rows(&log), [row("3", false, false)]);
}

#[test]
fn a_collection_approval_and_calls_that_change_no_allowance_are_filed_as_nothing() {
    let storage = Memory::default();
    let mut log = AllowanceLog::new(&storage, TEST_POLICY);
    file(&mut log, &[grant(COLLECTION, "approve", SPENDER, 5, 0)]);
    let mut short = on("increase_allowance", 1000, 0);
    short.calldata.truncate(1);
    file(
        &mut log,
        &[
            on("transfer", 1, 0),
            on("Approve", 1000, 0),
            on("increase_Allowance", 1000, 0),
            short,
        ],
    );
    assert!(log.list().unwrap().is_empty());
}

// Fails the first write it is asked for, then behaves as memory.
#[derive(Default)]
struct FailsOnce {
    memory: Memory,
    failed: Cell<bool>,
}

impl Storage for FailsOnce {
    fn read(&self, key: &str) -> Result<Option<Vec<u8>>, Error> {
        self.memory.read(key)
    }
    fn write(&self, key: &str, value: &[u8]) -> Result<(), Error> {
        if !self.failed.replace(true) {
            return Err(Error::storage("write"));
        }
        self.memory.write(key, value)
    }
    fn rename(&self, from: &str, to: &str) -> Result<(), Error> {
        self.memory.rename(from, to)
    }
}

#[test]
fn a_failing_write_skips_that_change_and_files_the_rest() {
    // "a failing write never throws back into the submitting path", and each change is tried.
    for entrypoint in ["approve", "increase_allowance"] {
        let storage = FailsOnce::default();
        let mut log = AllowanceLog::new(&storage, TEST_POLICY);
        file(
            &mut log,
            &[
                on(entrypoint, 1000, 0),
                grant(TOKEN, entrypoint, "0x9", 7, 0),
            ],
        );
        let spenders: Vec<_> = log.list().unwrap().into_iter().map(|r| r.spender).collect();
        assert_eq!(spenders, ["0x9"], "{entrypoint}");
    }
}
