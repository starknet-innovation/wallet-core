//! `nft-indexer.test.ts` and `normalizeIndexerNft`'s cases in `nft-metadata.test.ts`.

use super::*;
use crate::nft::TEST_POLICY;

const ADDRESS: &str = "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcde";

fn ok(body: &str) -> HttpOutcome {
    HttpOutcome {
        status: 200,
        retry_after: None,
        body: Some(body.as_bytes().to_vec()),
        timed_out: false,
    }
}

fn record(token_id: &str) -> String {
    format!(
        r#"{{"collection_name":"Punks","contract_address":"0x0511a2b3","image_url":"https://cdn.example/a.png","name":"Punk #{token_id}","token_id":"{token_id}"}}"#
    )
}

fn token_ids(discovery: &NftDiscovery) -> Vec<String> {
    discovery
        .nfts
        .iter()
        .map(|nft| nft.id.token_id.to_decimal())
        .collect()
}

#[test]
fn targets_the_documented_path() {
    let request = request(
        "https://indexer.example/api/",
        ADDRESS,
        NetworkId::Mainnet,
        &TEST_POLICY,
    );
    assert_eq!(
        request.url,
        format!("https://indexer.example/api/accounts/{ADDRESS}/nfts?network=SN_MAIN")
    );
    assert_eq!(request.method, HttpMethod::Get);
    assert!(request.headers.is_empty());
    assert_eq!(request.max_body_bytes, 4 * 1024 * 1024);
    assert_eq!(request.deadline_ms, 15_000);
}

#[test]
fn normalizes_an_adversarial_trailing_slash_run() {
    let url = format!("https://indexer.example/api{}", "/".repeat(8_192));
    let request = request(&url, ADDRESS, NetworkId::Sepolia, &TEST_POLICY);
    assert_eq!(
        request.url,
        format!("https://indexer.example/api/accounts/{ADDRESS}/nfts?network=SN_SEPOLIA")
    );
}

#[test]
fn parses_a_bare_array_body() {
    let body = format!("[{},{}]", record("1"), record("2"));
    let discovery = judge(&ok(&body), &TEST_POLICY).unwrap();
    assert_eq!(discovery.coverage, NftCoverage::Complete);
    assert_eq!(token_ids(&discovery), ["1", "2"]);
    let first = &discovery.nfts[0];
    assert_eq!(crate::felt::canonical(&first.id.contract), "0x511a2b3");
    assert_eq!(first.name, "Punk #1");
    assert_eq!(first.collection, "Punks");
    assert_eq!(first.image, "https://cdn.example/a.png");
}

#[test]
fn parses_an_envelope_dedupes_and_drops_malformed_records() {
    let body = format!(
        r#"{{"nfts":[{},{},{{"token_id":"3"}}]}}"#,
        record("1"),
        record("1")
    );
    let discovery = judge(&ok(&body), &TEST_POLICY).unwrap();
    assert_eq!(token_ids(&discovery), ["1"]);
    // A record the wallet could not read means the list may be short.
    assert_eq!(discovery.coverage, NftCoverage::Incomplete);
}

#[test]
fn only_unreadable_records_is_incomplete_not_empty() {
    let discovery = judge(&ok("[null]"), &TEST_POLICY).unwrap();
    assert_eq!(discovery, NftDiscovery::empty(NftCoverage::Incomplete));
}

#[test]
fn a_failed_exchange_or_bad_body_is_unavailable() {
    let body = format!("[{}]", record("1"));
    let mut not_ok = ok(&body);
    not_ok.status = 500;
    assert_eq!(judge(&not_ok, &TEST_POLICY), None);
    assert_eq!(judge(&ok(r#""not-json-shaped""#), &TEST_POLICY), None);
    assert_eq!(judge(&ok(r#"{"nfts":{}}"#), &TEST_POLICY), None);
    assert_eq!(judge(&ok("not json"), &TEST_POLICY), None);
    let transport = HttpOutcome {
        status: 0,
        retry_after: None,
        body: None,
        timed_out: true,
    };
    assert_eq!(judge(&transport, &TEST_POLICY), None);
    let small = NftPolicy {
        indexer_body_limit: 8,
        ..TEST_POLICY
    };
    assert_eq!(judge(&ok(&body), &small), None);
}

#[test]
fn caps_an_oversized_response() {
    let records: Vec<String> = (1..=TEST_POLICY.max_discovered + 25)
        .map(|index| record(&index.to_string()))
        .collect();
    let discovery = judge(&ok(&format!("[{}]", records.join(","))), &TEST_POLICY).unwrap();
    assert_eq!(discovery.nfts.len(), TEST_POLICY.max_discovered);
    // A trimmed list is never claimed to be whole.
    assert_eq!(discovery.coverage, NftCoverage::Incomplete);
}

fn one(record: &str) -> Option<NftCandidate> {
    candidate(&json::parse(record).unwrap())
}

#[test]
fn a_numeric_token_id_and_absent_text() {
    let nft = one(r#"{"contract_address":"0x0511a2b3","token_id":9}"#).unwrap();
    assert_eq!(nft.id.token_id, U256::from_u128(9));
    assert_eq!((nft.name.as_str(), nft.collection.as_str()), ("", ""));
    assert_eq!(nft.image, "");
    let trimmed = one(r#"{"contract_address":"0x0511a2b3","token_id":" 7 ","name":"  A  "}"#);
    assert_eq!(trimmed.unwrap().name, "A");
}

#[test]
fn refuses_an_unusable_contract_or_token_id() {
    for record in [
        r#"{"token_id":"1"}"#,
        r#"{"contract_address":"0x0","token_id":"1"}"#,
        r#"{"contract_address":"0x0511a2b3","token_id":"-1"}"#,
        r#"{"contract_address":"0x0511a2b3","token_id":"0x5"}"#,
        r#"{"contract_address":"0x0511a2b3","token_id":"01"}"#,
        r#"{"contract_address":"0x0511a2b3","token_id":1e21}"#,
        r#"{"contract_address":"0x0511a2b3","token_id":1.5}"#,
        r#"{"contract_address":"0x0511a2b3","token_id":true}"#,
        r#""nope""#,
        "[]",
    ] {
        assert_eq!(one(record), None, "{record}");
    }
    // 78 digits past u256 (a departure: the extension keeps it, its re-proof then fails).
    let past = format!(
        r#"{{"contract_address":"0x1","token_id":"{}"}}"#,
        "9".repeat(78)
    );
    assert_eq!(one(&past), None);
}

#[test]
fn bounds_names_and_the_raw_image() {
    let long = "x".repeat(200);
    let nft = one(&format!(
        r#"{{"contract_address":"0x1","token_id":"0","name":"{long}","image_url":"{}"}}"#,
        "y".repeat(MAX_IMAGE_LENGTH + 1)
    ))
    .unwrap();
    assert_eq!(nft.name.len(), MAX_TEXT_LENGTH);
    assert_eq!(nft.image, "");
}
