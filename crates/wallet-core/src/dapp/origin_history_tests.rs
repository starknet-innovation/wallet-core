//! Ported from the extension's `packages/features/permissions/tests/origin-history.test.ts` and
//! `origin-history-prune.test.ts`.

use super::*;
use crate::host::testing::Memory;

const FILE: &str = "origin-history.json";

fn history(storage: &Memory) -> OriginHistory<'_> {
    OriginHistory {
        storage,
        file: FILE,
        max_origins: 500,
    }
}

#[test]
fn records_the_first_seen_time_once_and_keeps_it() {
    let storage = Memory::default();
    let h = history(&storage);
    assert_eq!(h.first_seen("https://dapp.example").ok(), Some(None));
    assert_eq!(
        h.record("https://dapp.example", 5_000.0).ok(),
        Some(5_000.0)
    );
    assert_eq!(
        h.record("https://dapp.example", 15_000.0).ok(),
        Some(5_000.0)
    );
    // Another instance over the same file reads it back.
    assert_eq!(
        history(&storage).first_seen("https://dapp.example").ok(),
        Some(Some(5_000.0))
    );
    let stored = storage.0.borrow().get(FILE).cloned().unwrap_or_default();
    assert_eq!(stored, br#"{"https://dapp.example":5000}"#);
}

#[test]
fn ignores_invalid_origins_without_writing() {
    let storage = Memory::default();
    let h = history(&storage);
    assert_eq!(h.record("not-an-origin", 42.0).ok(), Some(42.0));
    assert_eq!(h.first_seen("not-an-origin").ok(), Some(None));
    assert!(storage.0.borrow().is_empty());
}

#[test]
fn drops_corrupt_entries_and_reads_a_corrupt_file_as_empty() {
    let storage = Memory::default();
    let entries = br#"{"bad key":1,"https://neg.example":-5,"https://half.example":1.5,"https://ok.example":42}"#;
    storage.0.borrow_mut().insert(FILE.into(), entries.to_vec());
    let h = history(&storage);
    assert_eq!(h.first_seen("https://ok.example").ok(), Some(Some(42.0)));
    assert_eq!(h.first_seen("https://neg.example").ok(), Some(None));
    assert_eq!(h.first_seen("https://half.example").ok(), Some(None));
    storage.0.borrow_mut().insert(FILE.into(), b"[1]".to_vec());
    assert_eq!(h.first_seen("https://ok.example").ok(), Some(None));
}

#[test]
fn keeps_the_newest_five_hundred() {
    let storage = Memory::default();
    let h = history(&storage);
    for index in 0..502_u32 {
        let origin = format!("https://d{index}.example");
        assert!(h.record(&origin, f64::from(index + 1)).is_ok());
    }
    let stored = storage.0.borrow().get(FILE).cloned().unwrap_or_default();
    let map: Map<String, Value> = serde_json::from_slice(&stored).unwrap_or_default();
    assert_eq!(map.len(), 500);
    assert_eq!(h.first_seen("https://d0.example").ok(), Some(None));
    assert_eq!(h.first_seen("https://d1.example").ok(), Some(None));
    assert_eq!(h.first_seen("https://d2.example").ok(), Some(Some(3.0)));
    assert_eq!(h.first_seen("https://d501.example").ok(), Some(Some(502.0)));
}
