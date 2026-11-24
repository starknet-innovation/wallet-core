//! The extension's own cases for `activityToCsv`, ported with their inputs and expected strings:
//! `packages/features/transactions/tests/activity/activity-csv.test.ts` ("csv test") and
//! `tests/storage/activity-csv-formula-injection.test.ts` ("injection test"), at Bramble
//! `a01b240c`. The extension's tests write some rows through its log first; here the row the log
//! would hold (status `pending`, `submittedAt` the log's clock) is built directly. The cases after
//! them cover branches of `activity-csv.ts` those tests leave out. No generator writes a fixture
//! for this yet.

use super::*;
use crate::history::activity::{SubmittedPrice, TxStatus};
use crate::portfolio::model::DisplayCurrency;
use crate::NetworkId;

const ACCOUNT_ID: &str =
    "SN_MAIN:0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcde";
const ADDRESS: &str = "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcde";
const COUNTERPARTY: &str = "0x1176a1bd84444c89232ec27754698e5d2e7e1a7f1539f12027f28b23ec9f3d8";

// The injection test's `entry`.
fn entry() -> ActivityEntry {
    ActivityEntry {
        account_id: ACCOUNT_ID.into(),
        amount: None,
        counterparty: None,
        direction: None,
        failure_notice: None,
        fee_payer: None,
        user_fee: None,
        relay: None,
        fee_paid: None,
        finalized_on_l1: None,
        hash: "0x1".into(),
        kind: ActivityKind::Invoke,
        network: NetworkId::Mainnet,
        not_found_count: None,
        origin: None,
        status: TxStatus::Pending,
        submitted_at: 1_700_000_000_000,
        submitted_price: None,
        summary: "Approve spender".into(),
        token: None,
        token_address: None,
    }
}

fn csv(entries: &[ActivityEntry]) -> String {
    activity_to_csv(entries, &crate::catalog::testing::networks())
}

fn rows(entries: &[ActivityEntry]) -> Vec<String> {
    csv(entries).split('\n').map(str::to_string).collect()
}

// The tests' `row.split(",")` over the first data row.
fn cells(entry: ActivityEntry) -> Vec<String> {
    rows(&[entry])[1].split(',').map(str::to_string).collect()
}

fn transfer(direction: Option<TransferDirection>, summary: &str, token: &str) -> ActivityEntry {
    ActivityEntry {
        amount: Some("2".into()),
        counterparty: Some(COUNTERPARTY.into()),
        direction,
        hash: "0x2".into(),
        kind: ActivityKind::Transfer,
        status: TxStatus::Accepted,
        summary: summary.into(),
        token: Some(token.into()),
        ..entry()
    }
}

// csv test: "serializes rows and escapes embedded quotes and commas" (the log's clock read 1000).
#[test]
fn serializes_rows_and_escapes_quotes_and_commas() {
    let rows = rows(&[ActivityEntry {
        hash: "0xabc".into(),
        submitted_at: 1000,
        summary: "Calls \"swap\", then transfer".into(),
        ..entry()
    }]);
    assert_eq!(rows[0], "datetimeUtc,networkId,accountAddress,kind,status,amount,token,counterparty,transactionHash,explorerUrl,valueAtSubmission,valueCurrency,summary");
    assert!(rows[1].contains("\"Calls \"\"swap\"\", then transfer\""));
    assert!(rows[1].contains(&format!("\"{ADDRESS}\"")));
    assert!(rows[1].contains("\"https://starkscan.co/tx/0xabc\""));
    // The whole row, as `activity-csv.ts` builds it.
    assert_eq!(
        rows[1],
        format!("\"1970-01-01 00:00:01\",\"SN_MAIN\",\"{ADDRESS}\",\"invoke\",\"pending\",\"\",\"\",\"\",\"0xabc\",\"https://starkscan.co/tx/0xabc\",\"\",\"\",\"Calls \"\"swap\"\", then transfer\"")
    );
}

// csv test: "renders the UTC date-time and splits structured columns".
#[test]
fn renders_the_utc_date_time_and_splits_structured_columns() {
    let cells = cells(ActivityEntry {
        amount: Some("1.5".into()),
        counterparty: Some("0x777abbcc".into()),
        direction: Some(TransferDirection::Out),
        hash: "0xdef".into(),
        kind: ActivityKind::Transfer,
        summary: "Send 1.5 ETH".into(),
        token: Some("ETH".into()),
        ..entry()
    });
    let expected = [
        "\"2023-11-14 22:13:20\"",
        "\"SN_MAIN\"",
        &format!("\"{ADDRESS}\""),
        "\"transfer\"",
        "\"pending\"",
        "\"1.5\"",
        "\"ETH\"",
        "\"0x777abbcc\"",
        "\"0xdef\"",
        "\"https://starkscan.co/tx/0xdef\"",
        "\"\"",
        "\"\"",
        "\"Sent 1.5 ETH to 0x777abbcc\"",
    ];
    assert_eq!(cells, expected);
}

// csv test: "exports the value at the price captured on submission, and only that".
#[test]
fn exports_the_value_at_the_captured_price() {
    let cells = cells(ActivityEntry {
        amount: Some("1.5".into()),
        direction: Some(TransferDirection::Out),
        hash: "0xdef".into(),
        kind: ActivityKind::Transfer,
        submitted_price: Some(SubmittedPrice {
            currency: DisplayCurrency::Eur,
            priced_at: 1_699_999_990_000,
            unit_price: 2000.004,
        }),
        summary: "Send 1.5 ETH".into(),
        token: Some("ETH".into()),
        ..entry()
    });
    assert_eq!(cells[10], "\"3000.01\"");
    assert_eq!(cells[11], "\"EUR\"");
}

// csv test: "rebuilds a transfer summary with the full counterparty and one verb".
#[test]
fn rebuilds_a_transfer_summary_with_the_full_counterparty() {
    let rows = rows(&[
        transfer(
            Some(TransferDirection::Out),
            "Send 2 STRK to 0x1176…f3d8",
            "STRK",
        ),
        transfer(
            Some(TransferDirection::In),
            "Received 2 STRK from 0x1176…f3d8",
            "STRK",
        ),
    ]);
    assert!(rows[1].ends_with(&format!("\"Sent 2 STRK to {COUNTERPARTY}\"")));
    assert!(rows[2].ends_with(&format!("\"Received 2 STRK from {COUNTERPARTY}\"")));
}

// csv test: "takes a transfer's direction from the record, never from its summary".
#[test]
fn takes_the_direction_from_the_record_never_the_summary() {
    let summary = "Send 2 Received to 0x1176…f3d8";
    let rows = rows(&[
        transfer(Some(TransferDirection::Out), summary, "Received"),
        // A legacy row with no recorded direction keeps its summary as recorded.
        transfer(None, summary, "Received"),
    ]);
    assert!(rows[1].ends_with(&format!("\"Sent 2 Received to {COUNTERPARTY}\"")));
    assert!(rows[2].ends_with("\"Send 2 Received to 0x1176…f3d8\""));
}

// csv test: "exports blank detail cells for entries lacking the optional fields".
#[test]
fn exports_blank_detail_cells_without_the_optional_fields() {
    let cells = cells(ActivityEntry {
        kind: ActivityKind::Deploy,
        summary: "Deploy account contract".into(),
        ..entry()
    });
    assert_eq!(cells[0], "\"2023-11-14 22:13:20\"");
    for blank in [5, 6, 7, 10, 11] {
        assert_eq!(cells[blank], "\"\"", "cell {blank}");
    }
    assert_eq!(cells[9], "\"https://starkscan.co/tx/0x1\"");
    assert_eq!(cells[12], "\"Deploy account contract\"");
}

// injection test: "neutralizes a summary opening with {lead}".
#[test]
fn neutralizes_a_summary_opening_a_formula() {
    for summary in ["=1+1", "+1+1", "-2+3", "@SUM(A1:A9)", "\t=1+1", "\r=1+1"] {
        let cells = cells(ActivityEntry {
            summary: summary.into(),
            ..entry()
        });
        assert_eq!(cells[12], format!("\"'{summary}\""), "{summary:?}");
    }
}

// injection test: "neutralizes a formula hidden behind {padding} padding".
#[test]
fn neutralizes_a_formula_behind_padding() {
    for summary in [
        " =1+1",
        "\n=1+1",
        "\u{b}=1+1",
        "\u{a0}=1+1",
        "\u{feff}=1+1",
        "\u{0}=1+1",
        "\u{7f}=1+1",
        " \t  @SUM(A1:A9)",
    ] {
        let csv = csv(&[ActivityEntry {
            summary: summary.into(),
            ..entry()
        }]);
        assert!(csv.ends_with(&format!("\"'{summary}\"")), "{summary:?}");
    }
}

// injection test: "does not prefix ordinary text that merely starts with padding".
#[test]
fn leaves_padded_ordinary_text_unprefixed() {
    let cells = cells(ActivityEntry {
        summary: " Send 1.5 ETH".into(),
        ..entry()
    });
    assert_eq!(cells[12], "\" Send 1.5 ETH\"");
}

// injection test: "neutralizes attacker text reaching the token and counterparty columns".
#[test]
fn neutralizes_the_token_and_counterparty_columns() {
    let cells = cells(ActivityEntry {
        counterparty: Some("@evil.example".into()),
        summary: "Send".into(),
        token: Some("=cmd|'/c calc'!A1".into()),
        ..entry()
    });
    assert_eq!(cells[6], "\"'=cmd|'/c calc'!A1\"");
    assert_eq!(cells[7], "\"'@evil.example\"");
}

// injection test: "leaves ordinary text untouched apart from the surrounding quotes".
#[test]
fn leaves_ordinary_text_untouched() {
    let cells = cells(ActivityEntry {
        summary: "Send 1.5 ETH".into(),
        token: Some("ETH".into()),
        ..entry()
    });
    assert_eq!(cells[6], "\"ETH\"");
    assert_eq!(cells[12], "\"Send 1.5 ETH\"");
}

// injection test: "quotes every column so a delimiter or quote cannot break the row".
#[test]
fn quotes_every_column() {
    let rows = rows(&[ActivityEntry {
        summary: "Calls \"swap\", then transfer".into(),
        ..entry()
    }]);
    assert_eq!(rows.len(), 2);
    assert!(rows[1].starts_with("\"2023-11-14 22:13:20\",\"SN_MAIN\","));
    assert!(rows[1].ends_with("\"Calls \"\"swap\"\", then transfer\""));
}

// injection test: "keeps a comma-bearing formula inside a single quoted cell". The test's last
// case ("end to end through the log") writes `=1+1` through the log first; the log is not part of
// the export here, and `neutralizes_a_summary_opening_a_formula` covers the cell.
#[test]
fn keeps_a_comma_bearing_formula_in_one_cell() {
    let rows = rows(&[ActivityEntry {
        summary: "=HYPERLINK(\"https://evil.example\",\"claim\")".into(),
        ..entry()
    }]);
    assert_eq!(rows.len(), 2);
    assert!(rows[1].ends_with("\"'=HYPERLINK(\"\"https://evil.example\"\",\"\"claim\"\")\""));
}

// Not in the extension's tests from here on.

#[test]
fn an_empty_list_is_the_header_alone() {
    assert_eq!(csv(&[]), ACTIVITY_CSV_HEADER);
}

#[test]
fn every_kind_and_status_is_its_stored_word() {
    for (kind, word) in [
        (ActivityKind::Declare, "declare"),
        (ActivityKind::Deploy, "deploy"),
        (ActivityKind::Invoke, "invoke"),
        (ActivityKind::Transfer, "transfer"),
    ] {
        assert_eq!(
            cells(ActivityEntry { kind, ..entry() })[3],
            format!("\"{word}\"")
        );
    }
    for (status, word) in [
        (TxStatus::Accepted, "accepted"),
        (TxStatus::Pending, "pending"),
        (TxStatus::Preconfirmed, "preconfirmed"),
        (TxStatus::Rejected, "rejected"),
        (TxStatus::Reverted, "reverted"),
    ] {
        assert_eq!(
            cells(ActivityEntry { status, ..entry() })[4],
            format!("\"{word}\"")
        );
    }
}

#[test]
fn dates_keep_whole_seconds_from_the_epoch_on() {
    let at = |submitted_at| {
        cells(ActivityEntry {
            submitted_at,
            ..entry()
        })[0]
            .clone()
    };
    assert_eq!(at(0), "\"1970-01-01 00:00:00\"");
    assert_eq!(at(1_700_000_000_999), "\"2023-11-14 22:13:20\"");
    assert_eq!(at(951_782_400_000), "\"2000-02-29 00:00:00\"");
    assert_eq!(at(253_402_300_799_000), "\"9999-12-31 23:59:59\"");
}

#[test]
fn the_account_and_the_explorer_link_follow_their_rules() {
    // A value without the separator passes through; Sepolia names its own network.
    let sepolia = cells(ActivityEntry {
        account_id: "0xabc".into(),
        network: NetworkId::Sepolia,
        ..entry()
    });
    assert_eq!(sepolia[1], "\"SN_SEPOLIA\"");
    assert_eq!(sepolia[2], "\"0xabc\"");
    // A hash that is not a lowercase non-zero felt gets no link.
    for hash in ["0xABC", "0x0"] {
        let cells = cells(ActivityEntry {
            hash: hash.into(),
            ..entry()
        });
        assert_eq!(cells[8], format!("\"{hash}\""));
        assert_eq!(cells[9], "\"\"");
    }
}

#[test]
fn a_value_needs_a_price_and_a_plain_positive_amount() {
    let priced = |amount: Option<&str>, currency| {
        let cells = cells(ActivityEntry {
            amount: amount.map(str::to_string),
            submitted_price: Some(SubmittedPrice {
                currency,
                priced_at: 1,
                unit_price: 0.5,
            }),
            ..entry()
        });
        (cells[10].clone(), cells[11].clone())
    };
    let blank = ("\"\"".to_string(), "\"\"".to_string());
    assert_eq!(
        priced(Some(" 3 "), DisplayCurrency::Usd),
        ("\"1.50\"".into(), "\"USD\"".into())
    );
    assert_eq!(
        priced(Some("1e3"), DisplayCurrency::Gbp),
        ("\"500.00\"".into(), "\"GBP\"".into())
    );
    for amount in [
        None,
        Some(""),
        Some("  "),
        Some("0"),
        Some("-1"),
        Some("abc"),
    ] {
        assert_eq!(priced(amount, DisplayCurrency::Usd), blank, "{amount:?}");
    }
}

#[test]
fn a_transfer_missing_a_detail_or_another_kind_keeps_its_summary() {
    let out = Some(TransferDirection::Out);
    let mut missing = transfer(out, "Send 2 STRK", "STRK");
    missing.counterparty = Some("  ".into());
    let invoke = ActivityEntry {
        kind: ActivityKind::Invoke,
        ..transfer(out, "Swap 2 STRK", "STRK")
    };
    let no_token = transfer(out, "Send 2", " ");
    let rows = rows(&[missing, invoke, no_token]);
    assert!(rows[1].ends_with("\"Send 2 STRK\""));
    assert!(rows[2].ends_with("\"Swap 2 STRK\""));
    assert!(rows[3].ends_with("\"Send 2\""));
    // The rebuilt summary trims each field; the raw cells keep their spelling.
    let padded = ActivityEntry {
        amount: Some(" 2 ".into()),
        ..transfer(out, "Send", "STRK")
    };
    let cells = cells(padded);
    assert_eq!(cells[5], "\" 2 \"");
    assert_eq!(cells[12], format!("\"Sent 2 STRK to {COUNTERPARTY}\""));
}

#[test]
fn non_ascii_and_line_breaks_stay_inside_their_cell() {
    let accented = cells(ActivityEntry {
        summary: "Envoyé 1 ÉTH → café 🚀".into(),
        token: Some("ÉTH".into()),
        ..entry()
    });
    assert_eq!(accented[6], "\"ÉTH\"");
    assert_eq!(accented[12], "\"Envoyé 1 ÉTH → café 🚀\"");
    // A line feed that opens no formula is kept, quoted, as the extension writes it.
    let csv = csv(&[ActivityEntry {
        summary: "two\nlines".into(),
        ..entry()
    }]);
    assert!(csv.ends_with(",\"two\nlines\""));
    // Padding other than JavaScript whitespace or a control character is not skipped.
    let cells = cells(ActivityEntry {
        summary: "\u{200b}=1+1".into(),
        ..entry()
    });
    assert_eq!(cells[12], "\"\u{200b}=1+1\"");
}
