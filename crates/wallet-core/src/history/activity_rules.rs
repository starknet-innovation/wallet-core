//! The extension's per-field activity rules split from `activity.rs`: the token-address and
//! direction grammars (`isRecordedTokenAddress`, `activityToken`), the fee and price shapes
//! (`isTransactionFee`, `isSubmittedPrice`), who paid the fee (`feePayer`, `userFee`), and the
//! planted-row cleanup of stored rows (`withoutPlantedRows`, audit #163).

use serde_json::{json, Map, Value};

use super::activity::{
    ActivityEntry, ActivityKind, ActivityOrigin, FeeUnit, SubmittedPrice, TransactionFee,
    TransferDirection,
};
use crate::js::felt::{canonical_felt, is_non_zero_felt, is_stark_field_element};
use crate::portfolio::model::DisplayCurrency;

/// `MAX_ACTIVITY_TIMESTAMP_MS`: the last instant an ECMAScript `Date` can hold.
pub const MAX_ACTIVITY_TIMESTAMP_MS: u64 = 8_640_000_000_000_000;

fn only_keys<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Map<String, Value>> {
    value
        .as_object()
        .filter(|map| map.keys().all(|key| keys.contains(&key.as_str())))
}

/// `isTransactionFee`: exactly `{amount, unit}`, `unit` `FRI` or `WEI`, `amount` a canonical felt.
pub fn transaction_fee(value: &Value) -> Option<TransactionFee> {
    let map = only_keys(value, &["amount", "unit"])?;
    let unit = FeeUnit::from_code(map.get("unit")?.as_str()?)?;
    let amount = map.get("amount")?.as_str()?;
    (is_stark_field_element(amount) && canonical_felt(amount).as_deref() == Some(amount)).then(
        || TransactionFee {
            amount: amount.into(),
            unit,
        },
    )
}

/// The fee the person signed in a token other than STRK (`ActivityEntry.userFee`): a gasless
/// transaction's fee transfer, whose base units `amount` holds as decimal digits. The receipt's
/// STRK is the relayer's, so such a row never takes a receipt fee.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserFee {
    pub amount: String,
    pub token: String,
    pub token_address: String,
}

/// Who paid a row's network fee when the account did not (`ActivityEntry.feePayer`): a sponsor,
/// such as a sponsored deployment's relay. Such a row never takes a receipt fee either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeePayer {
    Sponsor,
}

/// `isUserFee`: exactly `{amount, token, tokenAddress}`, `amount` 1 to 78 digits without a
/// leading zero, `token` 1 to 32 of `[A-Za-z0-9.]`, `tokenAddress` a canonical non-zero felt.
pub fn user_fee(value: &Value) -> Option<UserFee> {
    let map = only_keys(value, &["amount", "token", "tokenAddress"])?;
    let (amount, token) = (map.get("amount")?.as_str()?, map.get("token")?.as_str()?);
    let address = map.get("tokenAddress")?.as_str()?;
    let digits = amount.as_bytes();
    let amount_ok = (1..=78).contains(&digits.len())
        && digits[0] != b'0'
        && digits.iter().all(u8::is_ascii_digit);
    let token_ok = (1..=32).contains(&token.len())
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.');
    let address_ok =
        is_non_zero_felt(address) && canonical_felt(address).as_deref() == Some(address);
    (amount_ok && token_ok && address_ok).then(|| UserFee {
        amount: amount.into(),
        token: token.into(),
        token_address: address.into(),
    })
}

/// A gasless execution signed and about to be posted to the relay (the extension's
/// `RelaySignature`, `relay-record.ts`), kept on its row until the relay answers with a
/// transaction hash. While it is set the row's `hash` is the signed message's hash, which no
/// node knows, so the row is never observed as a transaction; past `execute_before` (Unix
/// seconds) the execution can no longer run, though whether it did is not known here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayExecution {
    pub nonce: String,
    pub execute_before: u64,
}

/// `isRelaySignature`'s share that a row carries: exactly `{executeBefore, nonce}`, a safe
/// integer and a canonical felt.
pub fn relay_execution(value: &Value) -> Option<RelayExecution> {
    let map = only_keys(value, &["executeBefore", "nonce"])?;
    let nonce = map.get("nonce")?.as_str()?;
    let before = map
        .get("executeBefore")?
        .as_u64()
        .filter(|b| *b < 1 << 53)?;
    (is_stark_field_element(nonce) && canonical_felt(nonce).as_deref() == Some(nonce)).then(|| {
        RelayExecution {
            nonce: nonce.into(),
            execute_before: before,
        }
    })
}

pub fn relay_json(relay: &RelayExecution) -> Value {
    json!({"executeBefore": relay.execute_before, "nonce": relay.nonce})
}

pub fn user_fee_json(fee: &UserFee) -> Value {
    json!({"amount": fee.amount, "token": fee.token, "tokenAddress": fee.token_address})
}

/// `isSubmittedPrice`: exactly `{currency, pricedAt, unitPrice}`, a display currency, a safe
/// integer time no later than `MAX_ACTIVITY_TIMESTAMP_MS`, and a finite positive price.
pub fn submitted_price(value: &Value) -> Option<SubmittedPrice> {
    let map = only_keys(value, &["currency", "pricedAt", "unitPrice"])?;
    let currency = DisplayCurrency::from_code(map.get("currency")?.as_str()?)?;
    let at = map.get("pricedAt")?.as_f64()?;
    let unit_price = map.get("unitPrice")?.as_f64()?;
    (at.fract() == 0.0
        && (0.0..=MAX_ACTIVITY_TIMESTAMP_MS as f64).contains(&at)
        && unit_price.is_finite()
        && unit_price > 0.0)
        .then_some(SubmittedPrice {
            currency,
            priced_at: at as u64,
            unit_price,
        })
}

/// A receipt's fee in the stored spelling (`feePaid`).
pub fn fee_json(fee: &TransactionFee) -> Value {
    json!({"amount": fee.amount, "unit": fee.unit.code()})
}

/// A submitted price in the stored spelling.
pub fn price_json(price: &SubmittedPrice) -> Value {
    json!({
        "currency": price.currency.code(),
        "pricedAt": price.priced_at,
        "unitPrice": price.unit_price,
    })
}

/// `isRecordedTokenAddress`: absent, or one canonical non-zero field element beside a symbol.
pub fn is_recorded_token_address(address: Option<&str>, token: Option<&str>) -> bool {
    address.is_none_or(|address| {
        token.is_some()
            && is_non_zero_felt(address)
            && canonical_felt(address).as_deref() == Some(address)
    })
}

/// `activityToken`: the token's canonical address when it is a non-zero field element.
pub fn activity_token_address(address: &str) -> Option<String> {
    canonical_felt(address).filter(|address| is_non_zero_felt(address))
}

/// `isZeroAmountLabel`: `/^0(?:\.0*)?$/`, a formatted amount of zero.
fn is_zero_amount_label(amount: &str) -> bool {
    match amount.strip_prefix('0') {
        Some(rest) => {
            rest.is_empty()
                || rest
                    .strip_prefix('.')
                    .is_some_and(|z| z.chars().all(|c| c == '0'))
        }
        None => false,
    }
}

/// The symbol the explorer lane gives a token the wallet does not track.
pub const UNTRACKED_TOKEN_SYMBOL: &str = "TOKEN";

/// `withoutPlantedRows` (audit #163): the planted-row rule applied to rows stored before it
/// existed, which reach the merge as local rows and would otherwise be kept. A zero-amount
/// transfer and an outgoing untracked-token transfer are dropped; an incoming untracked-token
/// transfer loses its counterparty. A row this wallet recorded (`origin: "wallet"`) is spared.
pub fn without_planted_rows(entries: Vec<ActivityEntry>) -> Vec<ActivityEntry> {
    entries
        .into_iter()
        .filter_map(|mut entry| {
            if entry.kind != ActivityKind::Transfer || entry.origin == Some(ActivityOrigin::Wallet)
            {
                return Some(entry);
            }
            if entry.amount.as_deref().is_some_and(is_zero_amount_label) {
                return None;
            }
            if entry.token.as_deref() != Some(UNTRACKED_TOKEN_SYMBOL) {
                return Some(entry);
            }
            if entry.summary.starts_with("Sent ") {
                return None;
            }
            entry.counterparty = None;
            Some(entry)
        })
        .collect()
}

pub fn direction_text(direction: TransferDirection) -> &'static str {
    match direction {
        TransferDirection::In => "in",
        TransferDirection::Out => "out",
    }
}

pub fn parse_direction(text: &str) -> Option<TransferDirection> {
    match text {
        "in" => Some(TransferDirection::In),
        "out" => Some(TransferDirection::Out),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::send::judge::TxStatus;
    use crate::NetworkId;

    #[test]
    fn planted_rows_are_dropped_or_stripped_as_the_extension_does() {
        let row = |amount: &str, token: &str, summary: &str| ActivityEntry {
            account_id: "SN_MAIN:0x1".into(),
            amount: Some(amount.into()),
            counterparty: Some("0x5".into()),
            direction: None,
            failure_notice: None,
            fee_payer: None,
            user_fee: None,
            relay: None,
            fee_paid: None,
            origin: None,
            submitted_price: None,
            finalized_on_l1: None,
            hash: "0xabc".into(),
            kind: ActivityKind::Transfer,
            network: NetworkId::Mainnet,
            not_found_count: None,
            status: TxStatus::Accepted,
            submitted_at: 1,
            summary: summary.into(),
            token: Some(token.into()),
            token_address: None,
        };
        for zero in ["0", "0.", "0.000"] {
            assert!(without_planted_rows(vec![row(zero, "STRK", "Received")]).is_empty());
        }
        for kept in ["0.01", "00", "10", "0.0001"] {
            assert_eq!(
                without_planted_rows(vec![row(kept, "STRK", "Received")]).len(),
                1
            );
        }
        assert!(without_planted_rows(vec![row("1", "TOKEN", "Sent 1 TOKEN to 0x5")]).is_empty());
        let incoming = without_planted_rows(vec![row("1", "TOKEN", "Received 1 TOKEN")]);
        assert_eq!(incoming[0].counterparty, None);
        let tracked = without_planted_rows(vec![row("1", "STRK", "Sent 1 STRK to 0x5")]);
        assert_eq!(tracked[0].counterparty.as_deref(), Some("0x5"));
    }

    #[test]
    fn token_addresses_must_be_canonical_field_elements() {
        let above_p = format!("0x{}", "f".repeat(64));
        assert!(!is_recorded_token_address(Some(&above_p), Some("X")));
        assert!(!is_recorded_token_address(Some("0x0"), Some("X")));
        assert!(!is_recorded_token_address(Some("0x07"), Some("X")));
        assert!(!is_recorded_token_address(Some("0x7"), None));
        assert!(is_recorded_token_address(Some("0x7"), Some("X")));
        assert_eq!(activity_token_address("-5"), None);
        assert_eq!(activity_token_address("0x07").as_deref(), Some("0x7"));
    }
}
