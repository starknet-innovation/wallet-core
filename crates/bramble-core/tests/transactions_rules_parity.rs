//! The send rules the extension's own code computed (`extension-transactions-{rules,
//! presentation,verification,status}.json`, see `fixtures/README.md`): fee policy, amount
//! grammar and call encoding, presentation, the verifier's flags, status judgement and timing,
//! through `wallet_core::send` with Bramble's constants. Moved from mobile's
//! `send/fixture_tests.rs` in Phase 2.3; its explorer-URL and activity-store cases move with
//! those modules (2.4).

use bramble_core::{
    CONFIRMATION_DELAYS_MS, FEE_MARGIN_PERCENT, REVIEWED_FEE_HEADROOM, SEND_POLICY,
};
use serde_json::Value;
use wallet_core::send::calls::{self, ReviewFlag, SendAmountError};
use wallet_core::send::fees::{self, FeeQuote, FeeRefusal, FeeUnit};
use wallet_core::send::judge::{self, Observation, TxStatus};
use wallet_core::send::plan::{DEPLOY_POLL_ATTEMPTS, DEPLOY_POLL_INTERVAL_MS};
use wallet_core::send::u256::U256;
use wallet_core::{felt, Felt};

fn fixture() -> Value {
    let mut whole = serde_json::Map::new();
    for name in ["rules", "presentation", "verification", "status"] {
        let path = format!(
            "{}/tests/fixtures/extension-transactions-{name}.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let part: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        whole.extend(part.as_object().unwrap().clone());
    }
    Value::Object(whole)
}

fn hex_u128(value: &Value) -> Option<u128> {
    u128::from_str_radix(value.as_str()?.trim_start_matches("0x"), 16).ok()
}

fn unit(value: &Value) -> FeeUnit {
    if value == "FRI" {
        FeeUnit::Fri
    } else {
        FeeUnit::Wei
    }
}

#[test]
fn fee_bounds_match_starknet_js() {
    for case in fixture()["fees"]["bounds"].as_array().unwrap() {
        let expected = &case["resourceBounds"];
        let fits = ["l1_gas", "l2_gas", "l1_data_gas"].iter().all(|r| {
            hex_u128(&expected[r]["max_amount"]).is_some_and(|a| a <= u128::from(u64::MAX))
                && hex_u128(&expected[r]["max_price_per_unit"]).is_some()
        });
        match fees::quote(&case["estimate"], FEE_MARGIN_PERCENT) {
            Ok((bounds, quote)) => {
                for (name, bound) in [
                    ("l1_gas", bounds.l1_gas),
                    ("l2_gas", bounds.l2_gas),
                    ("l1_data_gas", bounds.l1_data_gas),
                ] {
                    assert_eq!(
                        Some(bound.max_amount),
                        hex_u128(&expected[name]["max_amount"])
                    );
                    assert_eq!(
                        Some(bound.max_price_per_unit),
                        hex_u128(&expected[name]["max_price_per_unit"])
                    );
                }
                assert_eq!(quote.spend.to_decimal(), case["maxSpend"].as_str().unwrap());
                let amount = fees::fee_amount(&quote);
                let reviewed = &case["reviewed"];
                assert_eq!(amount.max_spend, reviewed["overallFee"].as_str().unwrap());
                assert_eq!(quote.unit, unit(&reviewed["unit"]));
                assert_eq!(fits, fees::krusty_bounds(&bounds).is_ok(), "{case}");
            }
            // starknet.js computes bounds past u128; the core refuses them instead of truncating.
            Err(refusal) => {
                assert_eq!(refusal, FeeRefusal::BoundsOutOfRange);
                assert!(!fits, "{case}");
            }
        }
    }
}

#[test]
fn reviewed_fee_ceiling_matches_the_extension() {
    let whole = fixture();
    let fees_json = &whole["fees"];
    for headroom in [&fees_json["headroom"], &whole["timing"]["feeHeadroom"]] {
        assert_eq!(headroom["numerator"], REVIEWED_FEE_HEADROOM.numerator);
        assert_eq!(headroom["denominator"], REVIEWED_FEE_HEADROOM.denominator);
    }
    let quote = |v: &Value| FeeQuote {
        spend: U256::from_decimal(v["overallFee"].as_str().unwrap()).unwrap(),
        unit: unit(&v["unit"]),
    };
    for case in fees_json["ceiling"].as_array().unwrap() {
        assert_eq!(
            fees::exceeds_reviewed(
                &quote(&case["reviewed"]),
                &quote(&case["quoted"]),
                REVIEWED_FEE_HEADROOM
            ),
            case["exceeds"].as_bool().unwrap(),
            "{case}"
        );
    }
}

#[test]
fn amounts_and_transfer_call_match_the_extension() {
    let calls_json = &fixture()["calls"];
    for case in calls_json["amounts"].as_array().unwrap() {
        let decimals = u8::try_from(case["decimals"].as_u64().unwrap()).unwrap();
        let parsed = calls::parse_token_amount(case["amount"].as_str().unwrap(), decimals);
        match (case["raw"].as_str(), case["error"].as_str()) {
            (Some(raw), None) if raw.len() > 77 && U256::from_decimal(raw).is_none() => {
                assert_eq!(parsed, Err(SendAmountError::TooLarge), "{case}");
            }
            (Some(raw), None) => assert_eq!(parsed.unwrap().to_decimal(), raw, "{case}"),
            (None, Some(error)) => {
                let expected = match error {
                    "Invalid transfer amount" => SendAmountError::Invalid,
                    "Too many decimal places" => SendAmountError::TooManyDecimals,
                    "Transfer amount must be greater than zero" => SendAmountError::NotPositive,
                    other => panic!("unmapped error {other}"),
                };
                assert_eq!(parsed, Err(expected), "{case}");
            }
            _ => panic!("bad case {case}"),
        }
    }
    assert_eq!(calls_json["transferSelector"], calls::TRANSFER_SELECTOR);
    let call = &calls_json["transferCall"];
    let token = felt::parse(call["contractAddress"].as_str().unwrap()).unwrap();
    let recipient = Felt::from_dec_str(call["calldata"][0].as_str().unwrap()).unwrap();
    let amount = U256::from_decimal(call["calldata"][1].as_str().unwrap()).unwrap();
    let calldata = calls::transfer_execute_calldata(&token, &recipient, &amount).unwrap();
    let expected: Vec<Felt> = calls_json["executeCalldata"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| Felt::from_dec_str(v.as_str().unwrap()).unwrap())
        .collect();
    assert_eq!(calldata, expected);
    for case in calls_json["zeroAddressErrors"].as_array().unwrap() {
        assert_eq!(case["error"], "Invalid transfer address");
        let token = calls::validate_recipient(case["token"].as_str().unwrap()).unwrap();
        let recipient = calls::validate_recipient(case["recipient"].as_str().unwrap()).unwrap();
        assert!(
            calls::transfer_execute_calldata(&token, &recipient, &U256::from_u128(1)).is_none()
        );
    }
    // `cairo.uint256` refuses 2^256; the core refuses it at parse time.
    assert!(calls_json["overflowError"].as_str().is_some());
    let over = "115792089237316195423570985008687907853269984665640564039457.584007913129639936";
    assert_eq!(
        calls::parse_token_amount(over, 18),
        Err(SendAmountError::TooLarge)
    );
}

#[test]
fn presentation_rules_match_the_extension() {
    let presentation = &fixture()["presentation"];
    for case in presentation["recipients"].as_array().unwrap() {
        let valid =
            calls::validate_recipient(case["input"].as_str().unwrap()).map(|f| felt::canonical(&f));
        assert_eq!(valid.as_deref(), case["valid"].as_str(), "{case}");
    }
    for case in presentation["shortAddresses"].as_array().unwrap() {
        assert_eq!(
            calls::short_label(case["input"].as_str().unwrap()),
            case["short"].as_str().unwrap()
        );
    }
    for case in presentation["feeText"].as_array().unwrap() {
        let spend = U256::from_decimal(case["fee"]["overallFee"].as_str().unwrap()).unwrap();
        let quote = FeeQuote {
            spend,
            unit: unit(&case["fee"]["unit"]),
        };
        assert_eq!(
            fees::fee_amount(&quote).display,
            case["text"].as_str().unwrap()
        );
        assert_eq!(
            fees::format_fri(&spend),
            case["formatFri"].as_str().unwrap()
        );
    }
    for case in presentation["tokenAmounts"].as_array().unwrap() {
        let raw = U256::from_decimal(case["raw"].as_str().unwrap()).unwrap();
        let decimals = u8::try_from(case["decimals"].as_u64().unwrap()).unwrap();
        assert_eq!(
            calls::format_token_amount(&raw, decimals),
            case["text"].as_str().unwrap()
        );
    }
}

/// ETH and STRK, the tokens the verification cases send. Their addresses and decimals are the
/// same on both networks; Bramble's token catalog supplies them once it moves here (Phase 2.6).
const TOKENS: [(&str, &str); 2] = [
    (
        "0x49d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7",
        "ETH",
    ),
    (
        "0x4718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d",
        "STRK",
    ),
];

#[test]
fn verification_summary_and_flags_match_the_extension() {
    let tokens: Vec<Felt> = TOKENS
        .iter()
        .map(|(a, _)| felt::parse(a).unwrap())
        .collect();
    for case in fixture()["verification"].as_array().unwrap() {
        let recipient = felt::parse(case["recipient"].as_str().unwrap()).unwrap();
        let account = felt::parse(case["account"].as_str().unwrap()).unwrap();
        let token = calls::validate_recipient(case["token"].as_str().unwrap()).unwrap();
        let (_, symbol) = TOKENS
            .iter()
            .find(|(address, _)| *address == felt::canonical(&token))
            .unwrap();
        let summary = calls::transfer_summary(
            &U256::from_u128(1_500_000_000_000_000_000),
            18,
            symbol,
            &recipient,
        );
        assert_eq!(summary, case["summary"].as_str().unwrap());
        let trusted: Vec<Felt> = case["trusted"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| felt::parse(a.as_str().unwrap()).unwrap())
            .collect();
        let flags: Vec<&str> = calls::transfer_flags(&recipient, &account, &trusted, &tokens)
            .iter()
            .map(|flag| match flag {
                ReviewFlag::SelfTransfer => "self-transfer",
                ReviewFlag::TokenContractRecipient => "token-contract-recipient",
                ReviewFlag::AddressPoisoning => "address-poisoning",
            })
            .collect();
        let expected: Vec<&str> = case["flags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["code"].as_str().unwrap())
            .collect();
        assert_eq!(flags, expected, "{case}");
    }
}

#[test]
fn status_judgement_matches_the_extension() {
    for case in fixture()["status"]["judged"].as_array().unwrap() {
        let mut body = case["reply"].clone();
        body["jsonrpc"] = "2.0".into();
        body["id"] = 1.into();
        let report = &case["report"];
        match judge::status(judge::answer(&body.to_string(), 1)) {
            Observation::Found {
                status,
                finalized_on_l1,
                ..
            } => {
                assert!(report["found"].as_bool().unwrap(), "{case}");
                let text = match status {
                    TxStatus::Pending => "pending",
                    TxStatus::Preconfirmed => "preconfirmed",
                    TxStatus::Accepted => "accepted",
                    TxStatus::Rejected => "rejected",
                    TxStatus::Reverted => "reverted",
                };
                assert_eq!(text, report["status"].as_str().unwrap(), "{case}");
                assert_eq!(finalized_on_l1, report["finalizedOnL1"].as_bool().unwrap());
            }
            Observation::NotFound => assert!(!report["found"].as_bool().unwrap(), "{case}"),
            Observation::Unknown => panic!("unreadable {case}"),
        }
    }
}

#[test]
fn timing_matches_the_extension() {
    let timing = &fixture()["timing"];
    let delays: Vec<u32> = timing["confirmationDelaysMs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| u32::try_from(v.as_u64().unwrap()).unwrap())
        .collect();
    assert_eq!(delays, CONFIRMATION_DELAYS_MS);
    assert_eq!(SEND_POLICY.confirmation_delays_ms, CONFIRMATION_DELAYS_MS);
    assert_eq!(timing["deployPollIntervalMs"], DEPLOY_POLL_INTERVAL_MS);
    assert_eq!(timing["deployPollAttempts"], DEPLOY_POLL_ATTEMPTS);
}
