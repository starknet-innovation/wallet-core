//! The activity CSV export, ported from `transactions/src/activity/activity-csv.ts`
//! (`activityToCsv`): one header row, then one row per entry in the order given, `\n` between
//! rows and none after the last. Every cell is quoted, and a cell that opens a spreadsheet
//! formula, even behind padding an importer skips, is prefixed with `'` (MCW-V09-123): token
//! symbols, counterparties and summaries are text this wallet never controls. The host writes
//! and shares the text; the file name is the wallet's.

use super::detail::submitted_value;
use super::store::{kind_text, status_text};
use crate::catalog::NetworkInfo;
use crate::history::activity::{ActivityEntry, ActivityKind, TransferDirection};
use crate::history::explorer::{account_id_address, explorer_transaction_url};
use crate::history::time::format_utc_date_time;
use crate::js::num::{is_js_whitespace, js_trim, to_fixed};
use crate::portfolio::model::network_code;

/// The header: the export contract a spreadsheet imports, so its names and order are fixed.
pub const ACTIVITY_CSV_HEADER: &str = "datetimeUtc,networkId,accountAddress,kind,status,amount,token,counterparty,transactionHash,explorerUrl,valueAtSubmission,valueCurrency,summary";

// `withoutLeadPadding`: JavaScript `trimStart`, then any control character, space or DEL, until
// neither is left.
fn without_lead_padding(value: &str) -> &str {
    let mut current = value.trim_start_matches(is_js_whitespace);
    while let Some(rest) = current.strip_prefix(|c: char| c <= '\u{20}' || c == '\u{7f}') {
        current = rest.trim_start_matches(is_js_whitespace);
    }
    current
}

// `opensFormula`: `CSV_FORMULA_LEAD = /^[\t\r=+@-]/` on the cell or on its unpadded form.
fn opens_formula(value: &str) -> bool {
    let lead = |text: &str| text.starts_with(['\t', '\r', '=', '+', '@', '-']);
    lead(value) || lead(without_lead_padding(value))
}

// `csvField`.
fn csv_field(value: &str) -> String {
    let guard = if opens_formula(value) { "'" } else { "" };
    format!("\"{guard}{}\"", value.replace('"', "\"\""))
}

// `csvSummary`: a plain transfer is rebuilt from its recorded fields, so it names the whole
// counterparty with one verb. The direction comes only from the recorded field, never from the
// summary, whose token symbol a third party chooses (Sherlock 196); any other row, or one
// missing a detail, keeps its summary as recorded.
fn csv_summary(entry: &ActivityEntry) -> String {
    let field = |value: &Option<String>| js_trim(value.as_deref().unwrap_or("")).to_string();
    let (amount, token, counterparty) = (
        field(&entry.amount),
        field(&entry.token),
        field(&entry.counterparty),
    );
    match entry.direction {
        Some(direction)
            if entry.kind == ActivityKind::Transfer
                && !amount.is_empty()
                && !token.is_empty()
                && !counterparty.is_empty() =>
        {
            let (verb, preposition) = match direction {
                TransferDirection::In => ("Received", "from"),
                TransferDirection::Out => ("Sent", "to"),
            };
            format!("{verb} {amount} {token} {preposition} {counterparty}")
        }
        _ => entry.summary.clone(),
    }
}

/// `activityToCsv`: `networks` is the wallet's catalog, which gives each row's explorer link.
pub fn activity_to_csv(entries: &[ActivityEntry], networks: &[NetworkInfo]) -> String {
    let mut lines = vec![ACTIVITY_CSV_HEADER.to_string()];
    for entry in entries {
        // `submittedValue`: to the cent, in the currency it was priced in; never an estimate.
        let (value, currency) = submitted_value(entry).map_or_else(Default::default, |(v, c)| {
            (to_fixed(v, 2), c.code().to_uppercase())
        });
        let cells = [
            format_utc_date_time(entry.submitted_at),
            network_code(entry.network).to_string(),
            account_id_address(&entry.account_id).to_string(),
            kind_text(entry.kind).to_string(),
            status_text(entry.status).to_string(),
            entry.amount.clone().unwrap_or_default(),
            entry.token.clone().unwrap_or_default(),
            entry.counterparty.clone().unwrap_or_default(),
            entry.hash.clone(),
            explorer_transaction_url(networks, entry.network, entry.hash.clone())
                .unwrap_or_default(),
            value,
            currency,
            csv_summary(entry),
        ];
        let row: Vec<String> = cells.iter().map(|cell| csv_field(cell)).collect();
        lines.push(row.join(","));
    }
    lines.join("\n")
}

#[cfg(test)]
#[path = "csv_tests.rs"]
mod tests;
