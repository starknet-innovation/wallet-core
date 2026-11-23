//! Allowances (P12) against what the extension's own code produced
//! (`fixtures/extension-allowances.jsonl`, `bun run mobile:parity`): `Approval` decoding, the
//! `allowance` reply, the unlimited cut-off, which stored records load, the merge and the
//! listing's rows and notices, retention ranking, the revoke's target and call, and the outcomes
//! that settle a revoke. The log, recording and the chain reads are `allowances_log_parity.rs`
//! and `allowances_scan_parity.rs`.

mod allowances_support;

use allowances_support::{
    amount, cases, felt, name, network, nft_contracts, readings, record, records, text, tokens,
    view, view_json,
};
use serde_json::{json, Value};
use starknet_wallet_core::allowances::calls::{approved_asset_kind, AssetKind};
use starknet_wallet_core::allowances::discovery::{
    decode_approval_event, listing_notices, merge_allowance_views, outstanding_allowances,
    parse_allowance_amount, AllowanceNotice, ExternalAllowance,
};
use starknet_wallet_core::allowances::record::{is_unlimited, parse_amount, AllowanceRecord};
use starknet_wallet_core::allowances::retention::{
    keep_largest, trim_to_total, RankToken, Ranking,
};
use starknet_wallet_core::allowances::revoke::{revoke_outcomes, RevokeRefusal, RevokeTarget};
use starknet_wallet_core::allowances::store::RevokeStatus;
use starknet_wallet_core::history::activity::entry_from_json;
use starknet_wallet_core::js::json::{parse, JsValue};
use starknet_wallet_core::send::call_decode::Known;
use starknet_wallet_core::send::types::{Account, Authorization};
use starknet_wallet_core::Felt;

fn js_array(value: &Value) -> Vec<JsValue> {
    match parse(&value.to_string()).unwrap() {
        JsValue::Array(items) => items,
        _ => Vec::new(),
    }
}

#[test]
fn approval_events_decode_as_the_extension_decodes_them() {
    for case in cases("decode") {
        let event = &case["event"];
        let decoded = decode_approval_event(&js_array(&event["keys"]), &js_array(&event["data"]));
        let got = decoded.map_or(
            Value::Null,
            |(owner, spender)| json!({ "owner": owner, "spender": spender }),
        );
        assert_eq!(got, case["decoded"], "{}", name(&case));
    }
}

#[test]
fn allowance_replies_parse_as_the_extension_parses_them() {
    for case in cases("amount") {
        let got = parse_allowance_amount(&js_array(&case["response"]));
        let got = got.map_or(Value::Null, |a| json!(a.to_decimal()));
        assert_eq!(got, case["amount"], "{}", name(&case));
    }
}

#[test]
fn the_unlimited_cut_off_matches() {
    for case in cases("unlimited") {
        let got = parse_amount(text(&case["amount"])).is_some_and(is_unlimited);
        assert_eq!(got, case["unlimited"] == true, "{}", name(&case));
    }
}

#[test]
fn stored_records_load_as_the_extension_loads_them() {
    for case in cases("record") {
        let loaded = AllowanceRecord::from_json(&case["value"]);
        assert_eq!(loaded.is_some(), case["valid"] == true, "{}", name(&case));
        // What loads is written back unchanged: one stored spelling per record.
        if let Some(loaded) = loaded {
            assert_eq!(loaded.to_json(), case["value"], "{}", name(&case));
        }
    }
}

#[test]
fn the_merge_matches() {
    for case in cases("merge") {
        let input = &case["input"];
        let external: Vec<ExternalAllowance> = input["external"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| ExternalAllowance {
                token: text(&e["token"]).into(),
                spender: text(&e["spender"]).into(),
                amount: amount(&e["amount"]),
            })
            .collect();
        let multisig = match input.get("multisig") {
            Some(list) => records(list),
            None => Vec::new(),
        };
        let views = merge_allowance_views(
            text(&input["account"]["id"]),
            network(&input["networkId"]),
            &records(&input["inWallet"]),
            &multisig,
            &external,
            &readings(&input["readings"]),
        );
        let got: Vec<Value> = views.iter().map(view_json).collect();
        assert_eq!(Value::Array(got), case["views"], "{}", name(&case));
    }
}

#[test]
fn the_listing_rows_match() {
    for case in cases("outstanding") {
        let (tokens, nfts) = (tokens(&case["assets"]), nft_contracts(&case["assets"]));
        let known = Known {
            tokens: &tokens,
            nft_contracts: &nfts,
        };
        let fungible = |token: &str| {
            felt_of(token).is_some_and(|t| approved_asset_kind(&known, &t) == AssetKind::Fungible)
        };
        // The extension is handed the ids the chain answered for; the core, those readings.
        let confirmed = case["confirmed"].as_array().unwrap();
        let confirmed: Vec<Value> = confirmed
            .iter()
            .map(|id| json!({ "id": id, "amount": "0" }))
            .collect();
        let views = case["views"].as_array().unwrap().iter().map(view).collect();
        let listed = outstanding_allowances(views, fungible, &readings(&Value::Array(confirmed)));
        let got: Vec<Value> = listed.iter().map(view_json).collect();
        assert_eq!(Value::Array(got), case["listed"], "{}", name(&case));
    }
}

fn felt_of(value: &str) -> Option<Felt> {
    starknet_wallet_core::felt::parse(value)
}

#[test]
fn the_listing_notices_match() {
    for case in cases("notices") {
        let got = listing_notices(
            case["scanComplete"] == true,
            case["multisigUnread"] == true,
            case["retentionFull"] == true,
        );
        let got: Vec<&str> = got
            .iter()
            .map(|notice| match notice {
                AllowanceNotice::ScanIncomplete => "scan-incomplete",
                AllowanceNotice::MultisigUnread => "multisig-unread",
                AllowanceNotice::RetentionFull => "retention-full",
            })
            .collect();
        assert_eq!(json!(got), case["notices"], "{}", name(&case));
    }
}

#[test]
fn retention_keeps_what_the_extension_keeps() {
    for case in cases("retention") {
        let rank_tokens: Vec<RankToken> = case["tokens"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| RankToken {
                network: network(&t["networkId"]),
                address: text(&t["address"]).into(),
                decimals: t["decimals"].as_u64().unwrap() as u32,
            })
            .collect();
        let ranking = Ranking::new(&rank_tokens);
        let all = records(&case["records"]);
        let limit = case["limit"].as_u64().unwrap() as usize;
        let kept: Vec<String> = match text(&case["fn"]) {
            "keepLargest" => {
                let refs: Vec<&AllowanceRecord> = all.iter().collect();
                let mask = keep_largest(&refs, limit, &ranking);
                all.iter()
                    .zip(mask)
                    .filter(|(_, keep)| *keep)
                    .map(|(r, _)| r.id.clone())
                    .collect()
            }
            _ => trim_to_total(all, limit, &ranking)
                .into_iter()
                .map(|r| r.id)
                .collect(),
        };
        assert_eq!(json!(kept), case["kept"], "{}", name(&case));
    }
}

#[test]
fn the_revoke_targets_and_calls_match() {
    let empty: [String; 0] = [];
    let known = Known {
        tokens: &[],
        nft_contracts: &empty,
    };
    for case in cases("revoke") {
        let account_id = text(&case["accountId"]);
        let (network_id, address) = account_id.split_once(':').unwrap();
        let account = Account {
            network: network(&json!(network_id)),
            address: felt(&json!(address)),
            public_key: Felt::ONE,
            authorization: Authorization::StarkPair,
            deployment: None,
        };
        // No stored record: the id alone names the target (`allowanceTarget`).
        let resolved = RevokeTarget::resolve(text(&case["id"]), &account, &[], &known);
        if case["target"].is_null() {
            assert_eq!(
                resolved.err(),
                Some(RevokeRefusal::NotFound),
                "{}",
                name(&case)
            );
            continue;
        }
        let call = resolved.unwrap().call();
        let expected = &case["call"];
        assert_eq!(
            call.contract_address,
            felt(&case["target"]["token"]),
            "{}",
            name(&case)
        );
        assert_eq!(
            call.contract_address,
            felt(&expected["contractAddress"]),
            "{}",
            name(&case)
        );
        assert_eq!(
            call.entrypoint,
            text(&expected["entrypoint"]),
            "{}",
            name(&case)
        );
        let calldata: Vec<Felt> = expected["calldata"]
            .as_array()
            .unwrap()
            .iter()
            .map(felt)
            .collect();
        assert_eq!(call.calldata, calldata, "{}", name(&case));
        assert_eq!(
            call.calldata[0],
            felt(&case["target"]["spender"]),
            "{}",
            name(&case)
        );
    }
}

#[test]
fn revoke_outcomes_match() {
    for case in cases("outcomes") {
        let entries: Vec<_> = case["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| entry_from_json(&parse(&e.to_string()).unwrap()).expect("a valid row"))
            .collect();
        let pending: Vec<AllowanceRecord> = case["records"]
            .as_array()
            .unwrap()
            .iter()
            .map(record)
            .collect();
        let got: Vec<Value> = revoke_outcomes(&pending, network(&case["network"]), &entries)
            .into_iter()
            .map(|o| {
                let status = match o.status {
                    RevokeStatus::Accepted => "accepted",
                    RevokeStatus::Failed => "failed",
                };
                json!({ "hash": o.hash, "status": status })
            })
            .collect();
        assert_eq!(Value::Array(got), case["outcomes"], "{}", name(&case));
    }
}
