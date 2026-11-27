//! P16's `typedData` and `outside` blocks (`fixtures/extension-dapp.jsonl`):
//!
//! - `typedData`: `CheckedTypedData::check` under `DAPP_POLICY.typed_data` accepts what
//!   `isTypedDataShape` accepts; `message_hash_for_chain` on Mainnet and Sepolia gives the hash
//!   starknet.js gave, or refuses where `typedDataHashForChain` refused, for the same reason; and
//!   `describe_typed_data` is `describeTypedData`;
//! - `outside`: `review_outside_execution` at `Stage::Queue` (with the allowance log) and
//!   `Stage::Sign` (without it) flags, summarizes and refuses each SNIP-9 outside execution as
//!   `reviewOutsideExecution` did, over the call-review fixture's trusted set;
//! - both: for each document the gate accepts, `CheckedTypedData::sign_rows` prints the side
//!   panel's card (`decodeSignRequest` through `signValueText`), refuses where it is
//!   `classified: false`, and a document the core cannot hash shows no card.
//!
//! Krusty hashes less than starknet.js (`typed_data.rs`'s module doc); each case where the
//! extension signs and the core fails closed is listed as a deviation.

use crate::dapp_parity_support;

use bramble_core::catalogs::networks::registry;
use bramble_core::dapp::DAPP_POLICY;
use dapp_parity_support::*;
use serde_json::{json, Value};
use starknet_wallet_core::allowances::record::AllowanceRecord;
use starknet_wallet_core::catalog::TokenInfo;
use starknet_wallet_core::dapp::sign_rows::{SignBlock, SignRow};
use starknet_wallet_core::dapp::typed_data::{describe_typed_data, CheckedTypedData};
use starknet_wallet_core::dapp::{review_outside_execution, Stage, TypedDataRefusal};
use starknet_wallet_core::send::call_decode::Known;
use starknet_wallet_core::send::review_flags::ReviewContext;
use starknet_wallet_core::{felt, Felt, NetworkId};

const MAINNET: &str = "0x534e5f4d41494e";
const SEPOLIA: &str = "0x534e5f5345504f4c4941";
/// Mainnet STRK, which `tokenContractRecipientFlag` always knows.
const STRK: &str = "0x04718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d";

/// Where Krusty refuses what starknet.js hashes: the extension would sign, the core fails closed.
const DEVIATIONS: &[(&str, &str)] = &[
    (
        "a message with an undeclared key",
        "Krusty refuses an undeclared message key; starknet.js hashes past it",
    ),
    (
        "domain fields in another order",
        "Krusty hashes only its own domain order; starknet.js follows the declaration",
    ),
    (
        "an enum, the unit variant",
        "both accept and bind it, but for a selected `()` variant Krusty's hash is not \
         starknet.js's (the payload variant agrees): the core would sign a different message \
         hash than the extension for the same card",
    ),
];

/// Where the core's outside-execution review parts from the extension's.
const OUTSIDE_DEVIATIONS: &[(&str, &str)] = &[(
    "a spender mimicking a contact",
    "the core refuses a grant whose spender mimics a trusted address at signing (wallet-core \
     #147, after Bramble #1038, unmerged); the extension on feature/mobile-wallet signs it",
)];

/// Where the core's sign card parts from the side panel's.
const CARD_DEVIATIONS: &[(&str, &str)] = &[
    (
        "a message with an undeclared key",
        "Krusty refuses the document, so the core shows no card; the extension shows one",
    ),
    (
        "domain fields in another order",
        "Krusty refuses the document, so the core shows no card; the extension shows one",
    ),
    (
        "revision 1, a string",
        "no ByteArray felt in the row: only Krusty's encoder computes it (krusty-kms #280)",
    ),
    (
        "a merkletree of three struct leaves",
        "no `root ⟨merkle root⟩` row: only Krusty's encoder computes it (krusty-kms #280)",
    ),
];

fn refusal(refused: TypedDataRefusal) -> &'static str {
    match refused {
        TypedDataRefusal::Invalid => "Invalid typed data",
        TypedDataRefusal::Unhashable => "Typed data cannot be hashed",
        TypedDataRefusal::ChainMismatch => "Typed data chain does not match the active network",
    }
}

#[test]
fn typed_data_is_gated_bound_and_summarized_as_the_extension_does() {
    let mut deviations = Deviations::new(DEVIATIONS);
    for case in cases("typedData", 32) {
        let name = name(&case);
        let document = js_value(&case["document"]);
        let account = felt_of(text(&case["account"]));
        assert_eq!(
            describe_typed_data(&document),
            text(&case["summary"]),
            "{name}"
        );
        let checked = CheckedTypedData::check(&document, &DAPP_POLICY.typed_data);
        assert_eq!(checked.is_ok(), case["shape"] == true, "{name}");
        let on = |chain: &str| match &checked {
            Err(refused) => json!({ "refused": refusal(*refused) }),
            Ok(typed) => match typed.message_hash_for_chain(&account, &felt_of(chain)) {
                Ok(hash) => json!({ "hash": felt::canonical(&hash) }),
                Err(refused) => json!({ "refused": refusal(refused) }),
            },
        };
        let theirs = |value: &Value| match value.get("hash") {
            Some(hash) => json!({ "hash": felt::canonical(&felt_of(text(hash))) }),
            None => value.clone(),
        };
        deviations.check(
            &name,
            json!([on(MAINNET), on(SEPOLIA)]),
            json!([theirs(&case["mainnet"]), theirs(&case["sepolia"])]),
        );
    }
    deviations.finish();
}

fn tokens(values: &Value) -> Vec<TokenInfo> {
    values
        .as_array()
        .unwrap()
        .iter()
        .map(|token| TokenInfo {
            address: text(&token["address"]).into(),
            decimals: token["decimals"].as_u64().unwrap() as u8,
            name: text(&token["name"]).into(),
            symbol: text(&token["symbol"]).into(),
        })
        .collect()
}

fn felts(value: &Value) -> Vec<Felt> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|item| felt_of(text(item)))
        .collect()
}

/// The core's review as the extension writes one: `null`, `{ error }`, or flags and summaries
/// (the core's first summary line is the card's `describeTypedData`, which the extension keeps
/// beside the review, so it is checked apart).
fn review(
    typed: &CheckedTypedData,
    account: &Felt,
    context: &ReviewContext,
    stage: Stage,
    summary: &str,
) -> Value {
    match review_outside_execution(typed, NetworkId::Mainnet, account, context, stage) {
        Ok(None) => Value::Null,
        Err(error) => json!({ "error": error.code }),
        Ok(Some(review)) => {
            assert_eq!(review.summaries[0], summary);
            let flags: Vec<Value> = review
                .flags
                .iter()
                .map(|flag| {
                    let text = flag.text();
                    let facts: serde_json::Map<String, Value> = text
                        .facts
                        .into_iter()
                        .map(|(key, value)| (key.to_string(), json!(value)))
                        .collect();
                    json!({
                        "code": flag.code(),
                        "message": flag.message(),
                        "text": { "key": text.key, "facts": facts },
                    })
                })
                .collect();
            json!({ "flags": flags, "summaries": review.summaries[1..] })
        }
    }
}

/// The extension's review with a refusal's sentence as the code a page would see.
fn theirs(value: &Value) -> Value {
    match value.get("error").and_then(Value::as_str) {
        Some("invalid typed data") => json!({ "error": 114 }),
        Some(_) => json!({ "error": 163 }),
        None => value.clone(),
    }
}

#[test]
fn outside_executions_are_reviewed_as_the_extension_reviews_them() {
    let mut deviations = Deviations::new(OUTSIDE_DEVIATIONS);
    for case in cases("outside", 17) {
        let name = name(&case);
        let document = js_value(&case["document"]);
        let Ok(typed) = CheckedTypedData::check(&document, &DAPP_POLICY.typed_data) else {
            // `queueSignTypedData` refuses these at its hash gate before any review runs.
            assert_eq!(case["shape"], false, "{name}");
            assert_eq!(theirs(&case["queue"]), json!({ "error": 114 }), "{name}");
            continue;
        };
        let context = &case["context"];
        let account = felt_of(text(&case["account"]["address"]));
        let tokens = tokens(&context["tokens"]);
        let nft_contracts: Vec<String> = context["nftContracts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|nft| text(nft).to_owned())
            .collect();
        let trusted: Vec<Felt> = ["ownAccounts", "addressBook", "recentRecipients"]
            .iter()
            .flat_map(|list| felts(&context[*list]))
            .collect();
        let token_contracts: Vec<Felt> = std::iter::once(felt_of(STRK))
            .chain(
                registry()
                    .tokens
                    .iter()
                    .filter(|token| token.network_id == "SN_MAIN")
                    .map(|token| felt_of(&token.address)),
            )
            .collect();
        let standing: Vec<AllowanceRecord> = case["standing"]
            .as_array()
            .unwrap()
            .iter()
            .map(|record| AllowanceRecord::from_json(record).expect("a record"))
            .collect();
        let at = |stage: Stage, standing: &[AllowanceRecord]| {
            let review_context = ReviewContext {
                known: Known {
                    tokens: &tokens,
                    nft_contracts: &nft_contracts,
                },
                trusted: &trusted,
                known_contracts: &[],
                token_contracts: &token_contracts,
                standing,
            };
            review(
                &typed,
                &account,
                &review_context,
                stage,
                text(&case["summary"]),
            )
        };
        deviations.check(
            &name,
            json!({ "queue": at(Stage::Queue, &standing), "sign": at(Stage::Sign, &[]) }),
            json!({ "queue": theirs(&case["queue"]), "sign": theirs(&case["sign"]) }),
        );
    }
    deviations.finish();
}

fn block_rows(block: &SignBlock) -> Value {
    let pairs = |rows: &[SignRow]| -> Vec<Value> {
        rows.iter().map(|row| json!([row.key, row.value])).collect()
    };
    json!({ "signed": pairs(&block.signed), "unsigned": pairs(&block.unsigned) })
}

/// The core's card as the extension writes one: `null` when the core cannot hash the document
/// on any chain the case recorded (no card is queued), `{ classified: false }` when `sign_rows`
/// refuses, the rows otherwise.
fn our_card(typed: &CheckedTypedData, account: &Felt, chains: &[&str]) -> Value {
    if chains.iter().all(|chain| {
        typed
            .message_hash_for_chain(account, &felt_of(chain))
            .is_err()
    }) {
        return Value::Null;
    }
    match typed.sign_rows() {
        None => json!({ "classified": false }),
        Some(rows) => json!({
            "classified": true,
            "domain": block_rows(&rows.domain),
            "message": block_rows(&rows.message),
        }),
    }
}

/// The extension's card: `null` when it hashed on none of `chains` (it queues no card either),
/// only `classified` when the card withholds Sign (the core refuses the request).
fn their_card(case: &Value, chains: &[&str]) -> Value {
    if !chains.is_empty()
        && chains
            .iter()
            .all(|chain| case[*chain].get("hash").is_none())
    {
        return Value::Null;
    }
    let card = &case["card"];
    if card["classified"] == false {
        return json!({ "classified": false });
    }
    card.clone()
}

#[test]
fn sign_cards_are_decoded_as_the_side_panel_decodes_them() {
    let mut deviations = Deviations::new(CARD_DEVIATIONS);
    let mut cards = 0;
    // `typedData` records both chains; `outside` documents are Mainnet's and record no hash.
    for (block, count, chains, theirs) in [
        (
            "typedData",
            32,
            &[MAINNET, SEPOLIA][..],
            &["mainnet", "sepolia"][..],
        ),
        ("outside", 17, &[MAINNET][..], &[][..]),
    ] {
        for case in cases(block, count) {
            let name = name(&case);
            let document = js_value(&case["document"]);
            let Ok(typed) = CheckedTypedData::check(&document, &DAPP_POLICY.typed_data) else {
                assert!(
                    case.get("card").is_none(),
                    "{name}: a card for a refused document"
                );
                continue;
            };
            if case["queue"].get("error").is_some() {
                // The outside review refused it at queue, on both sides (the test above): no card.
                continue;
            }
            cards += 1;
            // The side panel warns on the primary type alone, as `DappCard::SignTypedData`'s
            // `outside_execution` does (`dapp_sign_outside.rs`).
            assert_eq!(
                case["isOutsideExecution"] == true,
                typed.value().get("primaryType").and_then(|ty| ty.as_str())
                    == Some("OutsideExecution"),
                "{name}"
            );
            let account = match &case["account"] {
                Value::String(address) => felt_of(address),
                actor => felt_of(text(&actor["address"])),
            };
            deviations.check(
                &name,
                our_card(&typed, &account, chains),
                their_card(&case, theirs),
            );
        }
    }
    assert_eq!(cards, 32, "cards compared");
    deviations.finish();
}
