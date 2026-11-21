// `allowance-store.test.ts`, `allowance-store-branches.test.ts`, `allowance-store-delta.test.ts`
// and `allowance-revoke-hash.test.ts` (the extension's root `tests/storage/` and the package's
// `tests/allowances/`), over the log on in-memory storage, plus what they leave out. The
// extension's concurrency cases have no counterpart: `&mut self` serialises every write.

use serde_json::{json, Value};

use super::discovery::AllowanceReading;
use super::record::{AllowanceInput, AllowanceRecord};
use super::retention::Ranking;
use super::store::*;
use super::TEST_POLICY;
use crate::host::testing::Memory;
use crate::host::Storage;
use crate::send::u256::U256;
use crate::{Error, NetworkId};

const ACCOUNT: &str = "SN_MAIN:0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcde";
const TOKEN: &str = "0x049d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7";
const CANONICAL_TOKEN: &str = "0x49d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7";
const REVOKE: &str = "0xabc";
const MAX: &str = "115792089237316195423570985008687907853269984665640564039457584007913129639935";

type Shape = (String, bool, bool, Option<String>);

fn id(spender: &str) -> String {
    format!("{ACCOUNT}:{CANONICAL_TOKEN}:{spender}")
}

fn amount(text: &str) -> U256 {
    U256::from_decimal(text).unwrap()
}

fn input(value: &str) -> AllowanceInput {
    AllowanceInput {
        account_id: ACCOUNT.into(),
        network: NetworkId::Mainnet,
        token: TOKEN.into(),
        spender: "0x05".into(),
        amount: amount(value),
        lower_bound: false,
    }
}

fn record(log: &mut AllowanceLog, value: &str, hash: Option<&str>) {
    log.record(&input(value), hash, &Ranking::default())
        .unwrap();
}

fn delta(log: &mut AllowanceLog, direction: DeltaDirection, value: &str, hash: Option<&str>) {
    let change = AllowanceDelta {
        direction,
        input: input(value),
        transaction_hash: hash.map(str::to_string),
    };
    log.apply_delta(&change, &Ranking::default()).unwrap();
}

fn reading(value: &str) -> AllowanceReading {
    AllowanceReading {
        id: id("0x5"),
        amount: amount(value),
    }
}

fn outcome(status: RevokeStatus) -> RevokeOutcome {
    RevokeOutcome {
        hash: REVOKE.into(),
        status,
    }
}

fn stored_value(storage: &Memory) -> Value {
    serde_json::from_slice(&storage.read("allowances.json").unwrap().unwrap()).unwrap()
}

// (amount, lower bound, revoking, revoke hash) of each record, newest first.
fn shape(log: &AllowanceLog) -> Vec<Shape> {
    let entries = log.list().unwrap();
    entries
        .into_iter()
        .map(|r| {
            (
                r.amount.to_decimal(),
                r.lower_bound,
                r.revoking,
                r.revoke_hash,
            )
        })
        .collect()
}

fn plain(value: &str) -> Shape {
    (value.into(), false, false, None)
}

fn pending(value: &str, hash: Option<&str>) -> Shape {
    (value.into(), false, true, hash.map(str::to_string))
}

fn bound(value: &str) -> Shape {
    (value.into(), true, false, None)
}

#[test]
fn records_upsert_canonically_and_list_newest_first() {
    let storage = Memory::default();
    let mut log = AllowanceLog::new(&storage, TEST_POLICY);
    // "records, canonicalizes spender, and lists newest first".
    record(&mut log, "1000", None);
    let other = AllowanceInput {
        spender: "0x0a".into(),
        ..input("50")
    };
    log.record(&other, None, &Ranking::default()).unwrap();
    let spenders: Vec<String> = log.list().unwrap().into_iter().map(|r| r.spender).collect();
    assert_eq!(spenders, ["0xa", "0x5"]);
    // The stored value is the extension's shape: an array, oldest first.
    let expected = json!([
        { "accountId": ACCOUNT, "amount": "1000", "id": id("0x5"), "networkId": "SN_MAIN",
          "spender": "0x5", "token": CANONICAL_TOKEN },
        { "accountId": ACCOUNT, "amount": "50", "id": id("0xa"), "networkId": "SN_MAIN",
          "spender": "0xa", "token": CANONICAL_TOKEN },
    ]);
    assert_eq!(stored_value(&storage), expected);
    // "upserts the same spender/token instead of duplicating".
    record(&mut log, "2000", None);
    assert_eq!(log.list().unwrap().len(), 2);
    assert_eq!(log.list().unwrap()[0].amount, amount("2000"));
}

#[test]
fn invalid_inputs_write_nothing_and_remove_misses_nothing() {
    let storage = Memory::default();
    let mut log = AllowanceLog::new(&storage, TEST_POLICY);
    // allowance-store-branches.test.ts: a token or spender that is not a felt is dropped; so are
    // a zero spender and an account of another network.
    let bad = [
        AllowanceInput {
            token: "not-a-felt".into(),
            ..input("1000")
        },
        AllowanceInput {
            spender: String::new(),
            ..input("1000")
        },
        AllowanceInput {
            spender: "0x0".into(),
            ..input("1000")
        },
        AllowanceInput {
            network: NetworkId::Sepolia,
            ..input("1000")
        },
    ];
    for entry in &bad {
        log.record(entry, None, &Ranking::default()).unwrap();
    }
    assert!(log.list().unwrap().is_empty());
    // "a zero-amount record with no prior entry stays empty".
    record(&mut log, "0", None);
    assert!(log.list().unwrap().is_empty());
    // "remove of an absent id is a no-op"; then removes by id.
    record(&mut log, "1000", None);
    log.remove(&format!("{ACCOUNT}:0xdead:0xbeef")).unwrap();
    assert_eq!(log.list().unwrap().len(), 1);
    log.remove(&id("0x5")).unwrap();
    assert!(log.list().unwrap().is_empty());
}

#[test]
fn a_stored_record_that_fails_validation_is_dropped_on_load() {
    let storage = Memory::default();
    let good = json!({ "accountId": ACCOUNT, "amount": "1000", "id": id("0x5"),
        "networkId": "SN_MAIN", "spender": "0x5", "token": CANONICAL_TOKEN });
    let wrong_network = json!({ "accountId": ACCOUNT, "amount": "1000", "id": id("0x6"),
        "networkId": "SN_SEPOLIA", "spender": "0x6", "token": CANONICAL_TOKEN });
    let bad_hash = json!({ "accountId": ACCOUNT, "amount": "1000", "id": id("0x7"),
        "networkId": "SN_MAIN", "revokeHash": "0x0abc", "revoking": true, "spender": "0x7",
        "token": CANONICAL_TOKEN });
    let value = json!([good, { "bogus": true }, wrong_network, bad_hash, "text", null]);
    storage
        .write("allowances.json", value.to_string().as_bytes())
        .unwrap();
    let mut log = AllowanceLog::new(&storage, TEST_POLICY);
    let ids: Vec<String> = log.list().unwrap().into_iter().map(|r| r.id).collect();
    assert_eq!(ids, [id("0x5")]);
    // The next write keeps only what loaded.
    log.remove("nothing").unwrap();
    assert_eq!(stored_value(&storage), json!([good]));
    // "ignores malformed stored data": a bogus row, an object, a non-JSON value read as empty.
    for bytes in [&b"[{\"bogus\":true}]"[..], b"{}", b"not json", b""] {
        storage.write("allowances.json", bytes).unwrap();
        assert!(log.list().unwrap().is_empty());
    }
}

struct Broken;

impl Storage for Broken {
    fn read(&self, _: &str) -> Result<Option<Vec<u8>>, Error> {
        Err(Error::storage("read"))
    }
    fn write(&self, _: &str, _: &[u8]) -> Result<(), Error> {
        Err(Error::storage("write"))
    }
    fn rename(&self, _: &str, _: &str) -> Result<(), Error> {
        Err(Error::storage("rename"))
    }
}

#[test]
fn a_storage_failure_is_an_error_not_an_empty_log() {
    let mut log = AllowanceLog::new(&Broken, TEST_POLICY);
    assert!(log.list().is_err());
    assert!(log.record(&input("1"), None, &Ranking::default()).is_err());
}

#[test]
fn a_zero_over_a_grant_marks_it_revoking_with_its_hash() {
    let storage = Memory::default();
    let mut log = AllowanceLog::new(&storage, TEST_POLICY);
    // allowance-revoke-hash.test.ts "a zero approve over a tracked grant keeps the canonical
    // hash of its transaction".
    record(&mut log, "1000", Some("0x1"));
    record(&mut log, "0", Some("0x0ABC"));
    assert_eq!(shape(&log), [pending("1000", Some(REVOKE))]);
    // "a later revoke without a hash drops the earlier revoke's hash".
    record(&mut log, "0", None);
    assert_eq!(shape(&log), [pending("1000", None)]);
    // A zero hash is not kept either.
    record(&mut log, "0", Some("0x0"));
    assert_eq!(shape(&log), [pending("1000", None)]);
    // "a new grant replaces the pending revoke and its hash".
    record(&mut log, "0", Some(REVOKE));
    record(&mut log, "7", Some("0xdef"));
    assert_eq!(shape(&log), [plain("7")]);
}

#[test]
fn deltas_apply_erc20_arithmetic_inside_the_log() {
    let storage = Memory::default();
    let mut log = AllowanceLog::new(&storage, TEST_POLICY);
    // allowance-store-delta.test.ts "two concurrent increases both land" and "concurrent
    // increases and decreases settle on the net total", sequentially.
    record(&mut log, "1000", None);
    delta(&mut log, DeltaDirection::Add, "500", None);
    delta(&mut log, DeltaDirection::Subtract, "200", None);
    delta(&mut log, DeltaDirection::Add, "700", None);
    assert_eq!(shape(&log), [plain("2000")]);
    // "an increase saturates at the uint256 ceiling instead of overflowing".
    record(&mut log, MAX, None);
    delta(&mut log, DeltaDirection::Add, "1000", None);
    assert_eq!(shape(&log), [plain(MAX)]);
    // "a decrease that floors an EXACT entry keeps the grant until the chain agrees", with its
    // hash (allowance-revoke-hash.test.ts "a decrease to zero from an exact base").
    record(&mut log, "100", None);
    delta(&mut log, DeltaDirection::Subtract, "400", Some(REVOKE));
    assert_eq!(shape(&log), [pending("100", Some(REVOKE))]);
}

#[test]
fn a_delta_over_an_unseen_base_is_a_lower_bound() {
    let fresh = Memory::default();
    let mut log = AllowanceLog::new(&fresh, TEST_POLICY);
    // "an increase with no tracked base is a lower bound, not a total".
    delta(&mut log, DeltaDirection::Add, "1", None);
    assert_eq!(shape(&log), [bound("1")]);
    // "a decrease that floors a LOWER BOUND keeps the pair as an unknown amount".
    delta(&mut log, DeltaDirection::Subtract, "100", None);
    assert_eq!(shape(&log), [bound("0")]);
    // "an unknown remainder is corrected upward by a later increase".
    delta(&mut log, DeltaDirection::Add, "25", None);
    assert_eq!(shape(&log), [bound("25")]);
    // "a decrease with no tracked base records an unknown remainder, not nothing".
    let other = Memory::default();
    let mut log = AllowanceLog::new(&other, TEST_POLICY);
    delta(&mut log, DeltaDirection::Subtract, "400", None);
    assert_eq!(shape(&log), [bound("0")]);
    // "the persistent and in-memory stores apply identical semantics": 7 + 3 - 4 over nothing.
    let third = Memory::default();
    let mut log = AllowanceLog::new(&third, TEST_POLICY);
    delta(&mut log, DeltaDirection::Add, "7", None);
    delta(&mut log, DeltaDirection::Add, "3", None);
    delta(&mut log, DeltaDirection::Subtract, "4", None);
    assert_eq!(shape(&log), [bound("6")]);
    // "a malformed delta is ignored rather than stored" (a bad amount is not representable).
    let fourth = Memory::default();
    let mut log = AllowanceLog::new(&fourth, TEST_POLICY);
    let change = AllowanceDelta {
        direction: DeltaDirection::Add,
        input: AllowanceInput {
            token: "nope".into(),
            ..input("5")
        },
        transaction_hash: None,
    };
    log.apply_delta(&change, &Ranking::default()).unwrap();
    assert!(log.list().unwrap().is_empty());
}

#[test]
fn settling_on_readings_drops_only_what_a_zero_confirms() {
    let storage = Memory::default();
    let mut log = AllowanceLog::new(&storage, TEST_POLICY);
    // allowance-store.test.ts "keeps a submitted revoke as a zero bound, and drops it once the
    // chain reads zero".
    record(&mut log, "1000", None);
    record(&mut log, "0", None);
    log.settle_revocations(&[reading("0")], &[]).unwrap();
    assert!(log.list().unwrap().is_empty());
    // allowance-store-retention-risk.test.ts "settling leaves a record holding a figure alone".
    record(&mut log, "1000", None);
    log.settle_revocations(&[reading("0")], &[]).unwrap();
    assert_eq!(shape(&log), [plain("1000")]);
    // "The non-zero reading neither drops nor rewrites its pending revoke".
    record(&mut log, "0", None);
    log.settle_revocations(&[reading("42")], &[]).unwrap();
    assert_eq!(shape(&log), [pending("1000", None)]);
    // A zero lower bound goes on a zero reading; a non-zero lower bound does not.
    let bounds = Memory::default();
    let mut log = AllowanceLog::new(&bounds, TEST_POLICY);
    delta(&mut log, DeltaDirection::Subtract, "5", None);
    log.settle_revocations(&[reading("0")], &[]).unwrap();
    assert!(log.list().unwrap().is_empty());
    delta(&mut log, DeltaDirection::Add, "5", None);
    log.settle_revocations(&[reading("0")], &[]).unwrap();
    assert_eq!(shape(&log), [bound("5")]);
}

fn pending_log(storage: &Memory) -> AllowanceLog<'_> {
    let mut log = AllowanceLog::new(storage, TEST_POLICY);
    record(&mut log, "1000", None);
    record(&mut log, "0", Some(REVOKE));
    log
}

#[test]
fn settling_on_a_revoke_outcome() {
    let accepted = [outcome(RevokeStatus::Accepted)];
    // allowance-revoke-hash.test.ts "a failed revoke clears the mark and keeps the grant".
    let storage = Memory::default();
    let mut log = pending_log(&storage);
    let failed = [outcome(RevokeStatus::Failed)];
    log.settle_revocations(&[reading("1000")], &failed).unwrap();
    assert_eq!(shape(&log), [plain("1000")]);
    // "an accepted revoke with no reading keeps the record and its mark".
    let storage = Memory::default();
    let mut log = pending_log(&storage);
    log.settle_revocations(&[], &accepted).unwrap();
    assert_eq!(shape(&log), [pending("1000", Some(REVOKE))]);
    // "an accepted revoke with a zero reading drops the record".
    log.settle_revocations(&[reading("0")], &accepted).unwrap();
    assert!(log.list().unwrap().is_empty());
    // "an accepted revoke with a non-zero reading adopts it as the exact amount", and "an
    // accepted revoke read stale as non-zero is dropped by the next zero reading".
    let storage = Memory::default();
    let mut log = pending_log(&storage);
    log.settle_revocations(&[reading("5")], &accepted).unwrap();
    assert_eq!(shape(&log), [pending("5", None)]);
    log.settle_revocations(&[reading("0")], &[]).unwrap();
    assert!(log.list().unwrap().is_empty());
    // "a genuine re-grant is kept at its amount, through a later failed read".
    let storage = Memory::default();
    let mut log = pending_log(&storage);
    log.settle_revocations(&[reading("250")], &accepted)
        .unwrap();
    log.settle_revocations(&[], &accepted).unwrap();
    assert_eq!(shape(&log), [pending("250", None)]);
}

#[test]
fn settling_on_conflicting_or_foreign_outcomes() {
    // allowance-revoke-hash.test.ts "conflicting outcomes for one hash resolve to failed".
    for statuses in [
        [RevokeStatus::Accepted, RevokeStatus::Failed],
        [RevokeStatus::Failed, RevokeStatus::Accepted],
    ] {
        let storage = Memory::default();
        let mut log = pending_log(&storage);
        log.settle_revocations(&[reading("5")], &statuses.map(outcome))
            .unwrap();
        assert_eq!(shape(&log), [plain("1000")]);
    }
    // "an outcome for another transaction changes nothing"; nor does one reach a record that
    // never named its transaction ("a legacy pending revoke without a hash").
    let storage = Memory::default();
    let mut log = pending_log(&storage);
    let elsewhere = [RevokeOutcome {
        hash: "0xdef".into(),
        status: RevokeStatus::Failed,
    }];
    log.settle_revocations(&[], &elsewhere).unwrap();
    assert_eq!(shape(&log), [pending("1000", Some(REVOKE))]);
    record(&mut log, "0", None);
    let failed = [outcome(RevokeStatus::Failed)];
    log.settle_revocations(&[], &failed).unwrap();
    assert_eq!(shape(&log), [pending("1000", None)]);
}

#[test]
fn settling_that_changes_nothing_writes_nothing() {
    let storage = Memory::default();
    let raw = b"[{\"bogus\":true}]";
    storage.write("allowances.json", raw).unwrap();
    let mut log = AllowanceLog::new(&storage, TEST_POLICY);
    log.settle_revocations(&[reading("0")], &[]).unwrap();
    assert_eq!(storage.read("allowances.json").unwrap().unwrap(), raw);
}

#[test]
fn the_retention_notice_reads_both_caps() {
    let record = |account: &str, spender: u32| AllowanceRecord {
        id: format!("{account}:{CANONICAL_TOKEN}:0x{spender:x}"),
        account_id: account.into(),
        network: NetworkId::Mainnet,
        token: CANONICAL_TOKEN.into(),
        spender: format!("0x{spender:x}"),
        amount: U256::from_u128(1),
        lower_bound: false,
        revoking: false,
        revoke_hash: None,
    };
    let below: Vec<AllowanceRecord> = (1..50).map(|s| record(ACCOUNT, s)).collect();
    assert!(!retention_full(&below, ACCOUNT, &TEST_POLICY));
    let at_scope: Vec<AllowanceRecord> = (1..=50).map(|s| record(ACCOUNT, s)).collect();
    assert!(retention_full(&at_scope, ACCOUNT, &TEST_POLICY));
    assert!(!retention_full(&at_scope, "SN_MAIN:0x1", &TEST_POLICY));
    let at_total: Vec<AllowanceRecord> = (1..=200).map(|s| record("SN_MAIN:0x2", s)).collect();
    assert!(retention_full(&at_total, "SN_MAIN:0x1", &TEST_POLICY));
}
