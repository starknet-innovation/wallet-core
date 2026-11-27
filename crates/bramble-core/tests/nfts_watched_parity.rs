//! P17's watched NFTs against what the extension's own code did (G-NFT,
//! `fixtures/extension-nfts.jsonl`): the `watched` block (`withNftId`, `isWatchedNft`, the
//! preference writes and the gallery's candidates), and what `wallet.addNft` and an approved
//! `wallet_watchAsset` card leave in the store. The ownership read is the host's: each case's
//! recorded node replies decide `owned`, and a case whose reads all faulted stores nothing.
//!
//! A `watchAsset` case's params go through the page's reader (`read_page_request`): a refused one
//! has no card, and an admitted one's card names the NFT the store is asked to watch.
//!
//! Not read here: `gallery candidates`' `indexed` and `merged` (`mergeNftCandidates`, discovery's).

use bramble_core::dapp::{read_page_request, PageRequest};
use serde_json::{json, Value};
use starknet_wallet_core::dapp::{NftStandard, Request, WalletRpcError, WatchAsset};
use starknet_wallet_core::nft::{NftTarget, WatchedNft, WatchedNfts};
use starknet_wallet_core::send::u256::U256;
use starknet_wallet_core::{Error, Felt, NetworkId};

use crate::portfolio_support::Memory;

const KEY: &str = "nfts";

fn cases(block: &str, count: usize) -> Vec<Value> {
    let found: Vec<Value> = include_str!("fixtures/extension-nfts.jsonl")
        .lines()
        .map(|line| serde_json::from_str(line).expect("a JSON line"))
        .filter(|case: &Value| case["block"] == block)
        .collect();
    assert_eq!(found.len(), count, "{block} lines");
    found
}

fn watched_case(name: &str) -> Value {
    cases("watched", 4)
        .into_iter()
        .find(|case| case["name"] == name)
        .expect(name)
}

fn text(value: &Value) -> &str {
    value.as_str().expect("a string")
}

fn felt_of(value: &Value) -> Felt {
    Felt::from_hex(text(value)).expect("a felt")
}

fn network(value: &Value) -> NetworkId {
    NetworkId::from_catalog_id(text(value)).expect("a network")
}

// `BigInt(tokenId)`, as `withNftId` reads it: hex or decimal.
fn big(value: &str) -> Option<U256> {
    match value.strip_prefix("0x") {
        Some(hex) => U256::from_hex(hex),
        None => U256::from_decimal(value),
    }
}

fn stored(store: &WatchedNfts) -> Value {
    Value::Array(
        store
            .list()
            .unwrap()
            .iter()
            .map(WatchedNft::to_json)
            .collect(),
    )
}

fn refusal<T: std::fmt::Debug>(result: Result<T, Error>) -> Value {
    match result {
        Ok(_) => json!({ "ok": true }),
        Err(Error::InvalidInput { reason }) => json!({ "error": reason }),
        Err(other) => panic!("{other:?}"),
    }
}

/// What the recorded node said about `account` and the case's NFT: the last read that answered,
/// an `owner_of` (two calldata words: the token id) naming the account or a `balance_of` (three:
/// the account, then the token id) above zero. `None` when every read faulted.
fn owned(case: &Value, account: &Felt) -> Option<bool> {
    let answered = case["exchanges"]
        .as_array()
        .unwrap()
        .iter()
        .rfind(|exchange| exchange["reply"].get("result").is_some())?;
    let result = answered["reply"]["result"].as_array().unwrap();
    Some(
        match answered["params"]["request"]["calldata"]
            .as_array()
            .unwrap()
            .len()
        {
            2 => felt_of(&result[0]) == *account,
            3 => result.iter().any(|word| felt_of(word) != Felt::ZERO),
            other => panic!("calldata of {other} words"),
        },
    )
}

#[test]
fn with_nft_id_spells_a_record_as_the_extension_does() {
    for pair in watched_case("withNftId")["pairs"].as_array().unwrap() {
        let input = &pair["input"];
        let target = NftTarget {
            contract: felt_of(&input["contract"]),
            network: network(&input["networkId"]),
            token_id: big(text(&input["tokenId"])).unwrap(),
            label: input["label"].as_str().map(str::to_string),
        };
        let owner = input.get("owner").map(felt_of);
        let nft = WatchedNft::with_nft_id(&target, owner.as_ref(), input["spam"].as_bool());
        assert_eq!(nft.unwrap().to_json(), pair["output"], "{input}");
    }
}

#[test]
fn is_watched_nft_accepts_what_the_extension_accepts() {
    for pair in watched_case("isWatchedNft")["pairs"].as_array().unwrap() {
        let label = text(&pair["label"]);
        let valid = pair["valid"].as_bool().unwrap();
        assert_eq!(
            WatchedNft::from_json(&pair["value"]).is_some(),
            valid,
            "{label}"
        );
    }
}

#[test]
fn preference_writes_leave_the_list_the_extension_left() {
    let memory = Memory::default();
    let store = WatchedNfts::new(&memory, KEY);
    for step in watched_case("preference writes")["steps"]
        .as_array()
        .unwrap()
    {
        let input = &step["input"];
        let outcome = match text(&step["op"]) {
            "addNft" => match big(text(&input["tokenId"])) {
                // `-1` is no u256: the typed input refuses it before the store does.
                None => json!({ "error": "Invalid NFT" }),
                Some(token_id) => {
                    let target = NftTarget {
                        contract: felt_of(&input["contract"]),
                        network: network(&input["networkId"]),
                        token_id,
                        label: input["label"].as_str().map(str::to_string),
                    };
                    let account = felt_of(&input["owner"]);
                    refusal(store.watch_owned(&target, target.network, &account, true))
                }
            },
            "setNftSpam" => {
                refusal(store.set_spam(text(&input["id"]), input["spam"].as_bool().unwrap()))
            }
            "removeNft" => refusal(store.remove(text(input))),
            op => panic!("{op}"),
        };
        let expected = match step.get("error") {
            Some(error) => json!({ "error": error }),
            None => json!({ "ok": true }),
        };
        assert_eq!(outcome, expected, "{step}");
        assert_eq!(stored(&store), step["nfts"], "{step}");
    }
}

#[test]
fn the_gallery_lists_the_owners_unhidden_nfts_on_the_network() {
    let case = watched_case("gallery candidates");
    let memory = Memory::default();
    memory.0.borrow_mut().insert(
        KEY.into(),
        json!({ "nfts": case["nfts"] }).to_string().into_bytes(),
    );
    let store = WatchedNfts::new(&memory, KEY);
    let ids: Vec<String> = store
        .gallery(network(&case["network"]), &felt_of(&case["owner"]))
        .unwrap()
        .into_iter()
        .map(|nft| nft.id)
        .collect();
    let expected: Vec<&str> = case["watched"]
        .as_array()
        .unwrap()
        .iter()
        .map(|nft| text(&nft["id"]))
        .collect();
    assert_eq!(ids, expected);
}

#[test]
fn add_nft_stores_what_the_extension_stored() {
    for case in cases("addNft", 11) {
        let name = text(&case["name"]);
        let message = &case["message"];
        let account = felt_of(&case["account"]["address"]);
        let active = network(&case["account"]["networkId"]);
        let keys = ["contract", "label", "networkId", "tokenId", "type"];
        let target = message
            .as_object()
            .unwrap()
            .keys()
            .all(|key| keys.contains(&key.as_str()))
            .then(|| {
                NftTarget::read(
                    text(&message["contract"]),
                    text(&message["networkId"]),
                    text(&message["tokenId"]),
                    message["label"].as_str(),
                )
            })
            .flatten();
        let memory = Memory::default();
        let store = WatchedNfts::new(&memory, KEY);
        let reply = match (target, owned(&case, &account)) {
            (None, _) => json!({ "error": "invalid add nft request" }),
            // Every read faulted: the host reports its own error and the store is not touched.
            (Some(_), None) if !case["exchanges"].as_array().unwrap().is_empty() => {
                json!({ "error": "Could not read NFT ownership" })
            }
            (Some(target), owned) => {
                refusal(store.watch_owned(&target, active, &account, owned.unwrap_or(false)))
            }
        };
        assert_eq!(reply, case["reply"], "{name}");
        assert_eq!(stored(&store), case["watched"], "{name}");
    }
}

#[test]
fn an_approved_watch_asset_card_stores_what_the_extension_stored() {
    let mut read = 0;
    for case in cases("watchAsset", 16) {
        let name = text(&case["name"]);
        let request = read_page_request("wallet_watchAsset", Some(&case["params"].to_string()));
        let Some(card) = case["cards"].as_array().unwrap().first() else {
            assert_eq!(
                request,
                Err(WalletRpcError::invalid_request_payload()),
                "{name}"
            );
            continue;
        };
        read += 1;
        let Ok(PageRequest::Spec(Request::WatchAsset(WatchAsset::Nft {
            address,
            standard,
            token_id,
        }))) = request
        else {
            panic!("{name}: {request:?}");
        };
        let nft = &card["nft"];
        let standard = match standard {
            NftStandard::Erc721 => "ERC721",
            NftStandard::Erc1155 => "ERC1155",
        };
        assert_eq!(address, felt_of(&nft["contract"]), "{name}");
        assert_eq!(standard, nft["standard"], "{name}");
        assert_eq!(token_id, nft["tokenId"], "{name}");
        let target = NftTarget {
            contract: address,
            network: network(&nft["networkId"]),
            token_id: U256::from_decimal(&token_id).unwrap(),
            label: None,
        };
        let account = felt_of(&case["account"]["address"]);
        let memory = Memory::default();
        let store = WatchedNfts::new(&memory, KEY);
        // The fixture records the switch only in the case's name.
        store.set_shown(!name.contains("turned off")).unwrap();
        let denied = case["answers"][0]["error"]["code"] == 113;
        let decided = match owned(&case, &account) {
            _ if denied => json!({ "ok": true }),
            None if !case["exchanges"].as_array().unwrap().is_empty() => {
                json!({ "error": "approval execution failed" })
            }
            owned => refusal(store.watch_from_site(
                &target,
                network(&case["account"]["networkId"]),
                &account,
                owned.unwrap_or(false),
            )),
        };
        assert_eq!(decided, case["decided"], "{name}");
        assert_eq!(stored(&store), case["watched"], "{name}");
    }
    assert_eq!(read, 9, "cards decided");
}
