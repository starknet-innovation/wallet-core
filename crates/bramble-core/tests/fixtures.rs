//! The golden fixtures are copied from Bramble, never edited here. `fixtures/README.md` lists each
//! one with its sha256; this test holds the directory to that table in both directions.

use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

#[test]
fn every_fixture_is_listed_with_its_sha256() {
    let dir = format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"));
    let manifest = std::fs::read_to_string(format!("{dir}/README.md")).unwrap();
    // Table rows: | `file` | generator | `sha256` |
    let listed: BTreeMap<String, String> = manifest
        .lines()
        .filter_map(|line| {
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            let [_, file, _, hash, _] = cells[..] else {
                return None;
            };
            let file = file.strip_prefix('`')?.strip_suffix('`')?;
            let hash = hash.strip_prefix('`')?.strip_suffix('`')?;
            Some((file.to_string(), hash.to_string()))
        })
        .collect();

    let mut present = BTreeMap::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_str().unwrap().to_string();
        if name == "README.md" {
            continue;
        }
        let digest = Sha256::digest(std::fs::read(&path).unwrap());
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        present.insert(name, hex);
    }

    assert!(!listed.is_empty(), "no rows parsed from fixtures/README.md");
    assert_eq!(
        present, listed,
        "fixtures/README.md and the directory disagree"
    );
}
