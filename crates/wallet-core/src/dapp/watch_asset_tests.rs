//! Ported from the extension's `tests/provider/contract/{provider-surface,provider-watch-nft}
//! .test.ts` ("readProviderWatchAsset", "collectible watch-asset params").

use starknet_types_core::felt::Felt;

use super::{NftStandard, WatchAsset};
use crate::dapp::request::tests::POLICY;
use crate::dapp::{read_request, Request, WalletRpcError};

const TOKEN: &str = "0x053c91253bc9682c04929ca02ed00b3e423f6710d2ee7e0d5ebb06f3ecf368a8";
const CONTRACT: &str = "0x0511a2b3c4d5e6f";

fn watch(params: &str) -> Result<Request, WalletRpcError> {
    read_request("wallet_watchAsset", Some(params), &POLICY)
}

fn refused(params: &str) -> bool {
    watch(params) == Err(WalletRpcError::invalid_request_payload())
}

fn token() -> Felt {
    crate::felt::parse(TOKEN).unwrap()
}

fn erc20(decimals: Option<u8>, symbol: Option<&str>) -> Result<Request, WalletRpcError> {
    Ok(Request::WatchAsset(WatchAsset::Erc20 {
        address: token(),
        decimals,
        name: None,
        symbol: symbol.map(str::to_owned),
    }))
}

// "accepts SNIP ERC20 options with a non-zero address", the image and api_version cases.
#[test]
fn erc20_assets() {
    let usdc = format!(r#""address":"{TOKEN}","decimals":6,"symbol":"USDC""#);
    assert_eq!(
        watch(&format!(r#"{{"options":{{{usdc}}},"type":"ERC20"}}"#)),
        erc20(Some(6), Some("USDC"))
    );
    assert_eq!(
        watch(&format!(
            r#"{{"options":{{"address":"{TOKEN}"}},"type":"ERC20"}}"#
        )),
        erc20(None, None)
    );
    assert_eq!(
        watch(&format!(
            r#"{{"options":{{{usdc},"image":"https://example.com/usdc.png"}},"type":"ERC20"}}"#
        )),
        erc20(Some(6), Some("USDC"))
    );
    for version in ["0.10", "0.10.4", "0.10.5"] {
        assert_eq!(
            watch(&format!(
                r#"{{"options":{{{usdc}}},"type":"ERC20","api_version":"{version}"}}"#
            )),
            erc20(Some(6), Some("USDC"))
        );
    }
    assert_eq!(
        watch(&format!(
            r#"{{"options":{{"address":"{TOKEN}","decimals":0,"name":"Coin"}},"type":"ERC20"}}"#
        )),
        Ok(Request::WatchAsset(WatchAsset::Erc20 {
            address: token(),
            decimals: Some(0),
            name: Some("Coin".into()),
            symbol: None,
        }))
    );
}

// "rejects malformed watch-asset options", "rejects a malformed api_version, or one nested"
#[test]
fn malformed_erc20_assets_are_refused() {
    assert_eq!(
        read_request("wallet_watchAsset", None, &POLICY),
        Err(WalletRpcError::invalid_request_payload())
    );
    let options = |options: &str| format!(r#"{{"options":{{{options}}},"type":"ERC20"}}"#);
    for bad in [
        r#""address":"0x0""#.to_owned(),
        r#""address":"nope""#.to_owned(),
        format!(r#""address":"0x{}""#, "f".repeat(64)),
        r#""address":"0xABC""#.to_owned(),
        format!(r#""address":"{TOKEN}","decimals":99"#),
        format!(r#""address":"{TOKEN}","decimals":37"#),
        format!(r#""address":"{TOKEN}","decimals":1.5"#),
        format!(r#""address":"{TOKEN}","decimals":"6""#),
        format!(r#""address":"{TOKEN}","decimals":null"#),
        format!(r#""address":"{TOKEN}","image":7"#),
        format!(r#""address":"{TOKEN}","image":null"#),
        format!(r#""address":"{TOKEN}","symbol":1"#),
        format!(r#""address":"{TOKEN}","api_version":"0.10.4""#),
        format!(r#""address":"{TOKEN}","tokenId":"1""#),
    ] {
        assert!(refused(&options(&bad)), "{bad}");
    }
    assert!(refused(&format!(
        r#"{{"chainId":"0x1","options":{{"address":"{TOKEN}"}},"type":"ERC20"}}"#
    )));
    assert!(refused(&format!(
        r#"{{"options":{{"address":"{TOKEN}"}},"type":"ERC777"}}"#
    )));
    assert!(refused(r#"{"type":"ERC20"}"#));
    for bad in [
        r#""not.a.version.x""#,
        r#""""#,
        r#""v0.10.4""#,
        r#""0.10.4-rc.0""#,
        "10",
        "null",
    ] {
        assert!(refused(&format!(
            r#"{{"options":{{"address":"{TOKEN}"}},"type":"ERC20","api_version":{bad}}}"#
        )));
    }
}

fn nft(standard: &str, token_id: &str, extra: &str) -> Result<Request, WalletRpcError> {
    watch(&format!(
        r#"{{"options":{{"address":"{CONTRACT}","tokenId":{token_id}{extra}}},"type":"{standard}"}}"#
    ))
}

fn collectible(standard: NftStandard, token_id: &str) -> Result<Request, WalletRpcError> {
    Ok(Request::WatchAsset(WatchAsset::Nft {
        address: crate::felt::parse(CONTRACT).unwrap(),
        standard,
        token_id: token_id.into(),
    }))
}

// "reads ERC721 and ERC1155 assets with a decimal or hex token id"
#[test]
fn collectibles() {
    assert_eq!(
        nft("ERC721", r#""42""#, ""),
        collectible(NftStandard::Erc721, "42")
    );
    assert_eq!(
        nft(
            "ERC1155",
            r#""0x2a""#,
            r#","image":"https://x/y.png","name":"Hero""#
        ),
        collectible(NftStandard::Erc1155, "42")
    );
    assert_eq!(
        nft("ERC721", r#""0x2A""#, ""),
        collectible(NftStandard::Erc721, "42")
    );
    assert_eq!(
        nft("ERC721", r#""0""#, ""),
        collectible(NftStandard::Erc721, "0")
    );
    assert!(watch(&format!(
        r#"{{"options":{{"address":"{CONTRACT}","tokenId":"1"}},"type":"ERC721","api_version":"0.8"}}"#
    ))
    .is_ok());
}

// "refuses ids outside u256, other types, extra keys and bad options"
#[test]
fn malformed_collectibles_are_refused() {
    let max = "115792089237316195423570985008687907853269984665640564039457584007913129639935";
    assert_eq!(
        nft("ERC721", &format!(r#""{max}""#), ""),
        collectible(NftStandard::Erc721, max)
    );
    let over = "115792089237316195423570985008687907853269984665640564039457584007913129639936";
    for (standard, token_id, extra) in [
        ("ERC721", format!(r#""{over}""#), ""),
        ("ERC721", r#""-1""#.into(), ""),
        ("ERC721", r#""1.5""#.into(), ""),
        ("ERC721", "7".into(), ""),
        ("ERC721", r#""""#.into(), ""),
        ("ERC721", r#""042""#.into(), ""),
        ("ERC721", r#""0X2a""#.into(), ""),
        ("ERC721", r#""x""#.into(), ""),
        ("ERC721", format!(r#""0x{}""#, "f".repeat(65)), ""),
        ("ERC721", r#""1""#.into(), r#","image":3"#),
        ("ERC721", r#""1""#.into(), r#","name":null"#),
        ("ERC721", r#""1""#.into(), r#","decimals":0"#),
        ("ERC777", r#""1""#.into(), ""),
    ] {
        assert_eq!(
            nft(standard, &token_id, extra),
            Err(WalletRpcError::invalid_request_payload()),
            "{standard} {token_id} {extra}"
        );
    }
    assert!(refused(&format!(
        r#"{{"options":{{"address":"{CONTRACT}","tokenId":"1"}},"type":"ERC721","extra":true}}"#
    )));
    assert!(refused(
        r#"{"options":{"address":"0x0","tokenId":"1"},"type":"ERC721"}"#
    ));
    assert!(refused(r#"{"type":"ERC721"}"#));
    assert!(refused(&format!(
        r#"{{"options":{{"address":"{CONTRACT}"}},"type":"ERC721"}}"#
    )));
}
