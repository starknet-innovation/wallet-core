//! The allowance log (P12) against the extension's own store (`fixtures/
//! extension-allowances.jsonl`, `bun run mobile:parity`): each case seeds the stored value, runs
//! the extension's writes one by one (`record`, `applyDelta`, `remove`, `settleRevocations`), and
//! the core's log must leave the same stored value after each; then `recordApproveAllowances`
//! over a submitted batch (`calls::record_submitted`).

use crate::allowances_support;
use crate::portfolio_support;

use allowances_support::{cases, felt, name, network, nft_contracts, readings, text, tokens};
use bramble_core::ALLOWANCE_POLICY;
use portfolio_support::Memory;
use serde_json::Value;
use starknet_wallet_core::allowances::calls::{record_submitted, AllowanceOwner};
use starknet_wallet_core::allowances::record::AllowanceInput;
use starknet_wallet_core::allowances::retention::Ranking;
use starknet_wallet_core::allowances::store::{
    AllowanceDelta, AllowanceLog, DeltaDirection, RevokeOutcome, RevokeStatus,
};
use starknet_wallet_core::host::Storage;
use starknet_wallet_core::send::call_decode::Known;
use starknet_wallet_core::send::calls::Call;
use starknet_wallet_core::send::u256::U256;

const KEY: &str = ALLOWANCE_POLICY.store_key;

fn seeded(seed: Option<&Value>) -> Memory {
    let storage = Memory::default();
    if let Some(seed) = seed.filter(|s| !s.is_null()) {
        storage.write(KEY, seed.to_string().as_bytes()).unwrap();
    }
    storage
}

fn stored(storage: &Memory) -> Value {
    storage
        .read(KEY)
        .unwrap()
        .map_or(Value::Null, |bytes| serde_json::from_slice(&bytes).unwrap())
}

// `AllowanceInput`'s amount is a `U256`: an input whose amount is not decimal base units (which
// the extension writes nothing for) cannot be built, so it is `None`.
fn input(value: &Value) -> Option<AllowanceInput> {
    Some(AllowanceInput {
        account_id: text(&value["accountId"]).into(),
        network: network(&value["networkId"]),
        token: text(&value["token"]).into(),
        spender: text(&value["spender"]).into(),
        amount: U256::from_decimal(text(&value["amount"]))?,
        lower_bound: value["lowerBound"] == true,
    })
}

fn outcomes(value: &Value) -> Vec<RevokeOutcome> {
    let items = value.as_array().unwrap();
    items
        .iter()
        .map(|o| RevokeOutcome {
            hash: text(&o["hash"]).into(),
            status: match text(&o["status"]) {
                "accepted" => RevokeStatus::Accepted,
                _ => RevokeStatus::Failed,
            },
        })
        .collect()
}

#[test]
fn the_log_writes_what_the_extension_writes() {
    let ranking = Ranking::default();
    for case in cases("log") {
        let storage = seeded(case.get("seed"));
        let mut log = AllowanceLog::new(&storage, ALLOWANCE_POLICY);
        for (n, step) in case["steps"].as_array().unwrap().iter().enumerate() {
            // An input the core cannot build writes nothing, so the value must stand unchanged.
            match text(&step["op"]) {
                "record" => {
                    if let Some(input) = input(&step["input"]) {
                        let hash = step.get("hash").map(text);
                        log.record(&input, hash, &ranking).unwrap();
                    }
                }
                "applyDelta" => {
                    let delta = &step["delta"];
                    if let Some(input) = input(&delta["input"]) {
                        let delta = AllowanceDelta {
                            direction: match text(&delta["direction"]) {
                                "add" => DeltaDirection::Add,
                                _ => DeltaDirection::Subtract,
                            },
                            input,
                            transaction_hash: delta.get("transactionHash").map(|h| text(h).into()),
                        };
                        log.apply_delta(&delta, &ranking).unwrap();
                    }
                }
                "remove" => log.remove(text(&step["id"])).unwrap(),
                _ => log
                    .settle_revocations(&readings(&step["readings"]), &outcomes(&step["outcomes"]))
                    .unwrap(),
            }
            assert_eq!(stored(&storage), step["stored"], "{} step {n}", name(&case));
        }
    }
}

#[test]
fn a_submitted_batch_files_what_the_extension_files() {
    for case in cases("recording") {
        let (tokens, nfts) = (tokens(&case["assets"]), nft_contracts(&case["assets"]));
        let known = Known {
            tokens: &tokens,
            nft_contracts: &nfts,
        };
        let calls: Vec<Call> = case["calls"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| Call {
                contract_address: felt(&c["contractAddress"]),
                entrypoint: text(&c["entrypoint"]).into(),
                calldata: c["calldata"].as_array().unwrap().iter().map(felt).collect(),
            })
            .collect();
        let owner_id = text(&case["owner"]);
        let owner = AllowanceOwner {
            account_id: owner_id.into(),
            network: network(&Value::from(owner_id.split(':').next().unwrap())),
        };
        let storage = seeded(Some(&case["seed"]));
        let mut log = AllowanceLog::new(&storage, ALLOWANCE_POLICY);
        let hash = text(&case["hash"]);
        record_submitted(&mut log, &calls, &owner, &known, hash, &Ranking::default());
        assert_eq!(stored(&storage), case["stored"], "{}", name(&case));
    }
}
