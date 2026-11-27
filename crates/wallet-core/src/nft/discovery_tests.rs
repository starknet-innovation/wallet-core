//! `createHttpNftIndexer`'s source choice (`nft-indexer.test.ts`), the Starkscan source's gates
//! (`starkscan-nfts.test.ts`), and `setNftIndexerUrl`.

use super::*;
use crate::nft::TEST_POLICY;

const OWNER: &str = "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcde";
const INDEXER: &str = "https://indexer.example/api/";

fn input(indexer_url: Option<&str>) -> DiscoveryInput<'_> {
    DiscoveryInput {
        owner: OWNER,
        network: NetworkId::Mainnet,
        indexer_url,
        nfts_shown: true,
        explorer_allowed: true,
    }
}

fn settled(start: DiscoveryStart) -> Option<NftCoverage> {
    match start {
        DiscoveryStart::Settled(discovery) => {
            assert!(discovery.nfts.is_empty());
            Some(discovery.coverage)
        }
        DiscoveryStart::Send(..) => None,
    }
}

fn sent(start: DiscoveryStart) -> (PendingDiscovery, HttpRequest) {
    match start {
        DiscoveryStart::Send(pending, request) => (pending, request),
        DiscoveryStart::Settled(discovery) => panic!("settled: {discovery:?}"),
    }
}

fn ok(body: &str) -> HttpOutcome {
    HttpOutcome {
        status: 200,
        retry_after: None,
        body: Some(body.as_bytes().to_vec()),
        timed_out: false,
    }
}

#[test]
fn with_nfts_off_nothing_runs() {
    for url in [None, Some(INDEXER)] {
        let off = DiscoveryInput {
            nfts_shown: false,
            ..input(url)
        };
        assert_eq!(
            settled(PendingDiscovery::start(&off, &TEST_POLICY)),
            Some(NftCoverage::None)
        );
    }
}

#[test]
fn a_saved_url_replaces_starkscan() {
    for (network, explorer_allowed) in [(NetworkId::Mainnet, true), (NetworkId::Sepolia, false)] {
        let with_url = DiscoveryInput {
            network,
            explorer_allowed,
            ..input(Some(INDEXER))
        };
        let (pending, request) = sent(PendingDiscovery::start(&with_url, &TEST_POLICY));
        assert!(request
            .url
            .starts_with("https://indexer.example/api/accounts/"));
        let body = r#"[{"contract_address":"0x1","token_id":"1"}]"#;
        let discovery = pending.judge(&ok(body));
        assert_eq!(discovery.coverage, NftCoverage::Complete);
        assert_eq!(discovery.nfts.len(), 1);
    }
}

#[test]
fn starkscan_runs_on_mainnet_when_the_explorer_is_allowed() {
    let (pending, request) = sent(PendingDiscovery::start(&input(None), &TEST_POLICY));
    assert!(request
        .url
        .starts_with("https://starkscan.test/v1/SN_MAIN/address/"));
    let mut failed = ok("{}");
    failed.status = 429;
    assert_eq!(
        pending.judge(&failed),
        NftDiscovery::empty(NftCoverage::Unavailable)
    );
}

#[test]
fn starkscan_sends_nothing_off_mainnet_or_against_the_explorer_choice() {
    let sepolia = DiscoveryInput {
        network: NetworkId::Sepolia,
        ..input(None)
    };
    let kept_off = DiscoveryInput {
        explorer_allowed: false,
        ..input(None)
    };
    for case in [sepolia, kept_off] {
        assert_eq!(
            settled(PendingDiscovery::start(&case, &TEST_POLICY)),
            Some(NftCoverage::None)
        );
    }
}

#[test]
fn a_bad_owner_or_saved_url_sends_nothing() {
    let bad_owner = DiscoveryInput {
        owner: "0x0",
        ..input(Some(INDEXER))
    };
    let bad_url = input(Some("http://indexer.example/api"));
    let bad_starkscan_owner = DiscoveryInput {
        owner: "me",
        ..input(None)
    };
    for case in [bad_owner, bad_url, bad_starkscan_owner] {
        assert_eq!(
            settled(PendingDiscovery::start(&case, &TEST_POLICY)),
            Some(NftCoverage::Unavailable)
        );
    }
}

#[test]
fn the_indexer_url_setting_follows_the_custom_rpc_rule() {
    assert_eq!(indexer_url_setting("   "), Some(IndexerUrlSetting::Clear));
    assert_eq!(
        indexer_url_setting(" https://indexer.example/api "),
        Some(IndexerUrlSetting::Set("https://indexer.example/api".into()))
    );
    assert_eq!(
        indexer_url_setting("http://127.0.0.1:8080/nfts"),
        Some(IndexerUrlSetting::Set("http://127.0.0.1:8080/nfts".into()))
    );
    for refused in [
        "http://indexer.example/api",
        "https://user:pass@indexer.example",
        "ftp://indexer.example",
        "not a url",
    ] {
        assert_eq!(indexer_url_setting(refused), None, "{refused}");
    }
    let long = format!("https://indexer.example/{}", "a".repeat(2048));
    assert_eq!(indexer_url_setting(&long), None);
}
