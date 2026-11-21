//! The allowance record (`allowance-record.ts`), its id and scope, and the field rules a stored
//! record is held to (`allowance-store.ts`'s `isAllowanceRecord`, `hasValidAllowanceFields`,
//! `toRecord`), plus the unlimited cut-off (`allowance-retention.ts`).
//!
//! Amounts are `U256`: an ERC-20 allowance is a uint256, every writer in the extension produces
//! one (a decoded calldata pair, a clamped sum, a chain reading), and so a stored amount above
//! 2^256 - 1, which the extension's 78-digit rule would admit, is refused here.

use serde_json::{json, Map, Value};

use crate::history::explorer::is_account_id_for_network;
use crate::js::felt::{canonical_felt, is_non_zero_felt};
use crate::send::u256::U256;
use crate::NetworkId;

/// `UNLIMITED_ALLOWANCE_THRESHOLD`: `2^128 - 1`, the max-u128 sentinel; anything that sets the
/// high word is past it too.
pub const UNLIMITED_ALLOWANCE_THRESHOLD: U256 = U256 {
    high: 0,
    low: u128::MAX,
};

/// `isUnlimitedAllowance`, over an amount already parsed.
pub fn is_unlimited(amount: U256) -> bool {
    amount >= UNLIMITED_ALLOWANCE_THRESHOLD
}

/// `isBaseUnitsAmount` (`0`, or up to 78 digits without a leading zero) within uint256.
pub fn parse_amount(text: &str) -> Option<U256> {
    if text != "0" && text.starts_with('0') {
        return None;
    }
    U256::from_decimal(text)
}

/// One allowance to write (`AllowanceInput`). `lower_bound` marks an amount the wallet knows is
/// at least this large but cannot state exactly; a zero lower bound is not a removal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowanceInput {
    pub account_id: String,
    pub network: NetworkId,
    pub token: String,
    pub spender: String,
    pub amount: U256,
    pub lower_bound: bool,
}

/// One tracked allowance (`AllowanceRecord`), id `<accountId>:<token>:<spender>`. `revoking`
/// marks a submitted revoke the chain has not confirmed (the amount is still the spender's
/// reach); `revoke_hash`, only beside it, is that revoke's canonical transaction hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowanceRecord {
    pub id: String,
    pub account_id: String,
    pub network: NetworkId,
    pub token: String,
    pub spender: String,
    pub amount: U256,
    pub lower_bound: bool,
    pub revoking: bool,
    pub revoke_hash: Option<String>,
}

/// `allowanceId`: the account id, then the token and spender as `canonicalFelt` spells them;
/// `None` when either is not a number.
pub fn allowance_id(account_id: &str, token: &str, spender: &str) -> Option<String> {
    Some(format!(
        "{account_id}:{}:{}",
        canonical_felt(token)?,
        canonical_felt(spender)?
    ))
}

/// `allowanceScope`: the retention and identity scope, an account on a network.
pub fn allowance_scope(account_id: &str, network: NetworkId) -> String {
    format!("{}|{account_id}", network.catalog_id())
}

/// `isCanonicalHash`: a non-zero felt in its canonical spelling.
pub(crate) fn is_canonical_hash(value: &str) -> bool {
    is_non_zero_felt(value) && canonical_felt(value).as_deref() == Some(value)
}

const KNOWN_KEYS: [&str; 9] = [
    "accountId",
    "amount",
    "id",
    "lowerBound",
    "networkId",
    "revokeHash",
    "revoking",
    "spender",
    "token",
];

// An optional flag whose only spelling is `true`: absent is false, anything else is refused.
fn flag(value: Option<&Value>) -> Option<bool> {
    match value {
        None => Some(false),
        Some(Value::Bool(true)) => Some(true),
        Some(_) => None,
    }
}

impl AllowanceRecord {
    /// `hasValidAllowanceFields`: the account belongs to the network, token and spender are
    /// non-zero felts, and the id is exactly the three joined.
    fn has_valid_fields(&self) -> bool {
        is_account_id_for_network(&self.account_id, self.network)
            && is_non_zero_felt(&self.token)
            && is_non_zero_felt(&self.spender)
            && self.id == format!("{}:{}:{}", self.account_id, self.token, self.spender)
    }

    /// `isAllowanceRecord` over a stored value: no unknown key, no `null` for an optional one,
    /// the field rules, and a revoke hash only beside `revoking` and canonical.
    pub fn from_json(value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        if object.keys().any(|key| !KNOWN_KEYS.contains(&key.as_str())) {
            return None;
        }
        let text = |key: &str| object.get(key)?.as_str().map(str::to_string);
        let revoking = flag(object.get("revoking"))?;
        let revoke_hash = match object.get("revokeHash") {
            None => None,
            Some(Value::String(hash)) if revoking && is_canonical_hash(hash) => Some(hash.clone()),
            Some(_) => return None,
        };
        let record = Self {
            id: text("id")?,
            account_id: text("accountId")?,
            network: NetworkId::from_catalog_id(object.get("networkId")?.as_str()?)?,
            token: text("token")?,
            spender: text("spender")?,
            amount: parse_amount(object.get("amount")?.as_str()?)?,
            lower_bound: flag(object.get("lowerBound"))?,
            revoking,
            revoke_hash,
        };
        record.has_valid_fields().then_some(record)
    }

    /// The stored shape: the optional fields only when set, so each record has one spelling.
    pub fn to_json(&self) -> Value {
        let mut object = Map::new();
        object.insert("accountId".into(), json!(self.account_id));
        object.insert("amount".into(), json!(self.amount.to_decimal()));
        object.insert("id".into(), json!(self.id));
        if self.lower_bound {
            object.insert("lowerBound".into(), json!(true));
        }
        object.insert("networkId".into(), json!(self.network.catalog_id()));
        if let Some(hash) = &self.revoke_hash {
            object.insert("revokeHash".into(), json!(hash));
        }
        if self.revoking {
            object.insert("revoking".into(), json!(true));
        }
        object.insert("spender".into(), json!(self.spender));
        object.insert("token".into(), json!(self.token));
        Value::Object(object)
    }

    /// `toRecord`: the input with its token and spender canonical, or `None` when it fails the
    /// field rules.
    pub fn from_input(input: &AllowanceInput) -> Option<Self> {
        let record = Self {
            id: allowance_id(&input.account_id, &input.token, &input.spender)?,
            account_id: input.account_id.clone(),
            network: input.network,
            token: canonical_felt(&input.token)?,
            spender: canonical_felt(&input.spender)?,
            amount: input.amount,
            lower_bound: input.lower_bound,
            revoking: false,
            revoke_hash: None,
        };
        record.has_valid_fields().then_some(record)
    }

    pub fn scope(&self) -> String {
        allowance_scope(&self.account_id, self.network)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCOUNT: &str =
        "SN_MAIN:0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcde";
    const TOKEN: &str = "0x49d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7";

    fn stored(overrides: Value) -> Value {
        let mut base = json!({
            "accountId": ACCOUNT, "amount": "1000", "id": format!("{ACCOUNT}:{TOKEN}:0x5"),
            "networkId": "SN_MAIN", "spender": "0x5", "token": TOKEN,
        });
        for (key, value) in overrides.as_object().cloned().unwrap_or_default() {
            if value.is_null() {
                base.as_object_mut().unwrap().remove(&key);
            } else {
                base[key] = value;
            }
        }
        base
    }

    #[test]
    fn ids_take_canonical_felts() {
        // allowance-store.test.ts "derives a deterministic id from account, canonical token, and
        // spender".
        let padded = "0x049d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7";
        assert_eq!(
            allowance_id(ACCOUNT, padded, "0x05").unwrap(),
            format!("{ACCOUNT}:{TOKEN}:0x5")
        );
        assert_eq!(allowance_id(ACCOUNT, "nope", "0x5"), None);
    }

    #[test]
    fn stored_records_are_held_to_the_field_rules() {
        assert!(AllowanceRecord::from_json(&stored(json!({}))).is_some());
        // allowance-store.test.ts "rejects an account id whose embedded network contradicts the
        // record"; allowance-store-scope-cap.test.ts "a record whose network disagrees".
        assert!(
            AllowanceRecord::from_json(&stored(json!({ "networkId": "SN_SEPOLIA" }))).is_none()
        );
        // allowance-revoke-hash.test.ts: a legacy pending revoke without a hash loads; a
        // canonical hash round-trips; every malformed hash costs the record.
        let legacy = stored(json!({ "revoking": true }));
        assert!(AllowanceRecord::from_json(&legacy).is_some());
        let current = stored(json!({ "revokeHash": "0xabc", "revoking": true }));
        let record = AllowanceRecord::from_json(&current).unwrap();
        assert_eq!(record.revoke_hash.as_deref(), Some("0xabc"));
        assert_eq!(record.to_json(), current);
        for bad in [
            json!({ "revokeHash": "0xabc" }),
            json!({ "revokeHash": "0x0abc", "revoking": true }),
            json!({ "revokeHash": "0xABC", "revoking": true }),
            json!({ "revokeHash": "0x0", "revoking": true }),
            json!({ "revokeHash": "abc", "revoking": true }),
            json!({ "revokeHash": 2748, "revoking": true }),
        ] {
            assert!(
                AllowanceRecord::from_json(&stored(bad.clone())).is_none(),
                "{bad}"
            );
        }
        // Beyond the extension's tests: flags have one spelling, `null` is not absent, unknown
        // keys, a mismatched id, zero felts and amounts outside uint256 are refused.
        for bad in [
            json!({ "lowerBound": false }),
            json!({ "revoking": "true" }),
            json!({ "bogus": true }),
            json!({ "id": format!("{ACCOUNT}:{TOKEN}:0x6") }),
            json!({ "spender": "0x0", "id": format!("{ACCOUNT}:{TOKEN}:0x0") }),
            json!({ "amount": "01" }),
            json!({ "amount": "-5" }),
            json!({ "amount": 1000 }),
            json!({ "amount": "9".repeat(78) }), // the budget test's MAX_AMOUNT
            json!({ "accountId": "SN_MAIN:0x0" }),
        ] {
            assert!(
                AllowanceRecord::from_json(&stored(bad.clone())).is_none(),
                "{bad}"
            );
        }
        let mut with_null = stored(json!({}));
        with_null["lowerBound"] = Value::Null;
        assert!(AllowanceRecord::from_json(&with_null).is_none());
        assert!(AllowanceRecord::from_json(&json!([])).is_none());
    }

    #[test]
    fn amounts_cover_uint256_and_unlimited_starts_at_max_u128() {
        let max = "115792089237316195423570985008687907853269984665640564039457584007913129639935";
        assert_eq!(
            parse_amount(max).map(U256::to_decimal).as_deref(),
            Some(max)
        );
        assert_eq!(parse_amount("0"), Some(U256::ZERO));
        assert_eq!(parse_amount(""), None);
        // allowance-discovery.test.ts "isUnlimitedAllowance".
        let at = |text: &str| is_unlimited(parse_amount(text).unwrap());
        assert!(at("340282366920938463463374607431768211456"));
        assert!(at("340282366920938463463374607431768211455"));
        assert!(!at("340282366920938463463374607431768211454"));
        assert!(!at("1000"));
        assert!(at(max));
    }
}
