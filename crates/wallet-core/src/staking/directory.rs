//! Starkscan's validator directory (`GET {base}/{chain}/staking/validators`, schema
//! `StakingValidatorPage`), from `staking-directory.ts` and `staking-directory-ops.ts`.
//!
//! The directory is untrusted third-party data. It only helps the person find a staker address;
//! it never supplies a pool, calldata or any text that is signed. Every field the wallet reads
//! is checked for type, length and format, every array is bounded before it is walked, and
//! anything malformed refuses the whole page, so the screen falls back to a pasted address
//! rather than a partial or shifted list. Fields the wallet does not read are ignored. The
//! directory's own order (most STRK staked first) is kept: nothing here sorts.

use std::collections::HashSet;

use super::coverage::{staking_coverage, Coverage};
use super::{StakingPolicy, StakingRefusal};
use crate::history::starkscan::{starkscan_get, starkscan_json};
use crate::js::big::{parse_bigint, Big};
use crate::js::felt::same_felt;
use crate::js::json::JsValue;
use crate::js::num::{js_trim, utf16_len};
use crate::portfolio::model::TokenConfig;
use crate::portfolio::wire::{HttpOutcome, HttpRequest};
use crate::rpc::format_digits;
use crate::{felt, Felt, NetworkId};

/// `MAX_DIRECTORY_CURSOR_LENGTH`: the longest opaque cursor accepted from, or sent back to, the
/// directory.
pub const MAX_DIRECTORY_CURSOR_LENGTH: usize = 1024;
const MAX_NAME_LENGTH: usize = 64;
const MAX_WEBSITE_LENGTH: usize = 200;
const MAX_TOKENS_PER_VALIDATOR: usize = 32;
const MAX_RAW_DIGITS: usize = 78;
const PPM: u32 = 1_000_000;
/// The only `source` a staking answer may name.
pub(crate) const SNAPSHOT_SOURCE: &str = "finalized_prepared_staking_snapshot";

/// A directory validator's `status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidatorStatus {
    Active,
    Exiting,
    Inactive,
}

/// One pool as listed: a token and whether it takes stake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectoryPool {
    pub active: bool,
    pub token: Felt,
}

/// One validator as read from a page; figures `None` when unknown, never zero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryValidator {
    /// Commission in parts per million.
    pub commission_ppm: Option<u32>,
    /// Delegated stake per token, raw base units as decimal digits.
    pub delegated: Vec<(Felt, Option<String>)>,
    /// 30-day liveness in parts per million.
    pub liveness_30d_ppm: Option<u32>,
    pub name: Option<String>,
    pub pools: Vec<DirectoryPool>,
    pub staker: Felt,
    pub status: Option<ValidatorStatus>,
    /// The website's hostname (https only).
    pub website: Option<String>,
}

/// One validated directory page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryPage {
    pub coverage: Coverage,
    pub next_cursor: Option<String>,
    pub validators: Vec<DirectoryValidator>,
}

/// `isDirectoryCursor`: a cursor the wallet will echo back, 1 to 1,024 printable ASCII
/// characters with no space.
pub fn is_directory_cursor(value: &str) -> bool {
    (1..=MAX_DIRECTORY_CURSOR_LENGTH).contains(&value.len())
        && value.bytes().all(|b| (0x21..=0x7e).contains(&b))
}

/// `starkscanFelt`: an address field of at most 66 characters, `0x` and hex in either case,
/// below the prime (`felt::parse` bounds the digits to 64).
pub(crate) fn starkscan_felt(value: Option<&JsValue>) -> Option<Felt> {
    let text = value?.as_str()?;
    let digits = text.strip_prefix("0x")?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    felt::parse(&text.to_ascii_lowercase())
}

/// `starkscanRaw`: `Some(None)` for `null` (unknown), `Some(Some(digits))` for up to 78 decimal
/// digits (non-zero when `positive`), `None` for anything else (malformed).
pub(crate) fn starkscan_raw(value: Option<&JsValue>, positive: bool) -> Option<Option<String>> {
    let text = match value? {
        JsValue::Null => return Some(None),
        JsValue::String(text) => text,
        _ => return None,
    };
    let valid = !text.is_empty()
        && text.len() <= MAX_RAW_DIGITS
        && text.bytes().all(|b| b.is_ascii_digit())
        && !(positive && text.bytes().all(|b| b == b'0'));
    valid.then(|| Some(text.clone()))
}

/// `⌊numerator × 10^6 / denominator⌋` for decimal digit strings, when `numerator <= denominator`.
fn ppm(numerator: &str, denominator: &str) -> Option<u32> {
    let numerator = parse_bigint(numerator)?.magnitude;
    let denominator = parse_bigint(denominator)?.magnitude;
    if numerator > denominator {
        return None;
    }
    let mut target = numerator;
    target.mul_add(PPM, 0);
    let times = |q: u32| -> Big {
        let mut product = denominator.clone();
        product.mul_add(q, 0);
        product
    };
    // The largest q in 0..=10^6 with denominator × q <= target (a binary search: no division).
    let (mut low, mut high) = (0u32, PPM);
    while low < high {
        let mid = low + (high - low).div_ceil(2);
        if times(mid) <= target {
            low = mid;
        } else {
            high = mid - 1;
        }
    }
    Some(low)
}

// `ratioPpm`: a ratio at most 1 with exactly the two known keys.
fn ratio_ppm(value: Option<&JsValue>) -> Option<Option<u32>> {
    let value = value?;
    if *value == JsValue::Null {
        return Some(None);
    }
    let entries = value.as_object()?;
    if entries
        .iter()
        .any(|(key, _)| key != "denominatorRaw" && key != "numeratorRaw")
    {
        return None;
    }
    let numerator = starkscan_raw(value.get("numeratorRaw"), false)??;
    let denominator = starkscan_raw(value.get("denominatorRaw"), true)??;
    ppm(&numerator, &denominator).map(Some)
}

// C0/C1 controls, and the bidi overrides and isolates that can reorder a name on screen.
fn is_unsafe(c: char) -> bool {
    let code = u32::from(c);
    code <= 0x1f
        || (0x7f..=0x9f).contains(&code)
        || code == 0x200e
        || code == 0x200f
        || (0x202a..=0x202e).contains(&code)
        || (0x2066..=0x2069).contains(&code)
}

// `textOf`: `Some(None)` for null or blank, the trimmed text when it fits, `None` otherwise.
fn text_of(value: Option<&JsValue>, max: usize) -> Option<Option<String>> {
    let text = match value? {
        JsValue::Null => return Some(None),
        JsValue::String(text) => text,
        _ => return None,
    };
    if utf16_len(text) > max || text.chars().any(is_unsafe) {
        return None;
    }
    let trimmed = js_trim(text);
    Some((!trimmed.is_empty()).then(|| trimmed.to_string()))
}

// The hostname `new URL(text)` gives for a plain `https://` URL with no credentials, ASCII
// labels and a name (not an address) for a host; `None` for anything else, which is never shown.
fn website_host(text: &str) -> Option<String> {
    let rest = text
        .get(..8)
        .filter(|scheme| scheme.eq_ignore_ascii_case("https://"))
        .and_then(|_| text.get(8..))?;
    // Credentials (`user@`, `user:pw@`) leave an `@` that no label below accepts.
    let authority = &rest[..rest.find(['/', '?', '#', '\\']).unwrap_or(rest.len())];
    let host = match authority.rsplit_once(':') {
        Some((host, port)) if port.is_empty() || port.parse::<u16>().is_ok() => host,
        Some(_) => return None,
        None => authority,
    };
    let host = host.to_ascii_lowercase();
    let labels: Vec<&str> = host.split('.').collect();
    let named = labels.iter().all(|label| {
        !label.is_empty()
            && label
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    }) && labels
        .last()
        .is_some_and(|label| label.bytes().any(|b| b.is_ascii_alphabetic()));
    named.then_some(host)
}

// `websiteOf`: malformed text refuses the page; a URL that is not plain https is only dropped.
fn website_of(value: Option<&JsValue>) -> Option<Option<String>> {
    Some(text_of(value, MAX_WEBSITE_LENGTH)?.and_then(|text| website_host(&text)))
}

fn bounded_array(value: Option<&JsValue>) -> Option<&[JsValue]> {
    value?
        .as_array()
        .filter(|items| items.len() <= MAX_TOKENS_PER_VALIDATOR)
}

fn pool_of(value: &JsValue) -> Option<DirectoryPool> {
    value.as_object()?;
    let token = starkscan_felt(value.get("tokenAddress"))?;
    starkscan_felt(value.get("address"))?;
    let active = match value.get("status")?.as_str()? {
        "active" => true,
        "retired" => false,
        _ => return None,
    };
    Some(DirectoryPool { active, token })
}

fn stake_of(value: &JsValue) -> Option<(Felt, Option<String>)> {
    value.as_object()?;
    let token = starkscan_felt(value.get("tokenAddress"))?;
    Some((token, starkscan_raw(value.get("delegatedStakeRaw"), false)?))
}

fn validator_of(value: &JsValue) -> Option<DirectoryValidator> {
    value.as_object()?;
    let staker = starkscan_felt(value.get("address")).filter(|staker| *staker != Felt::ZERO)?;
    let status = match value.get("status")? {
        JsValue::Null => None,
        JsValue::String(status) => Some(match status.as_str() {
            "active" => ValidatorStatus::Active,
            "exiting" => ValidatorStatus::Exiting,
            "inactive" => ValidatorStatus::Inactive,
            _ => return None,
        }),
        _ => return None,
    };
    Some(DirectoryValidator {
        name: text_of(value.get("name"), MAX_NAME_LENGTH)?,
        website: website_of(value.get("website"))?,
        commission_ppm: ratio_ppm(value.get("commission"))?,
        liveness_30d_ppm: ratio_ppm(value.get("liveness30d"))?,
        pools: bounded_array(value.get("pools"))?
            .iter()
            .map(pool_of)
            .collect::<Option<_>>()?,
        delegated: bounded_array(value.get("stake"))?
            .iter()
            .map(stake_of)
            .collect::<Option<_>>()?,
        staker,
        status,
    })
}

/// `parseStakingDirectoryPage` for `network`: `None` when anything the wallet reads is malformed
/// or out of bounds, when the page answers for another chain, holds more than the policy's page
/// size, or lists a staker twice.
pub fn parse_directory_page(
    value: &JsValue,
    network: NetworkId,
    policy: &StakingPolicy,
) -> Option<DirectoryPage> {
    value.as_object()?;
    let coverage = staking_coverage(value.get("coverage"), policy.max_catching_up_lag_blocks)?;
    if value.get("chainId")?.as_str()? != network.catalog_id()
        || value.get("source")?.as_str()? != SNAPSHOT_SOURCE
    {
        return None;
    }
    let next_cursor = match value.get("nextCursor")? {
        JsValue::Null => None,
        JsValue::String(cursor) if is_directory_cursor(cursor) => Some(cursor.clone()),
        _ => return None,
    };
    let items = value
        .get("items")?
        .as_array()
        .filter(|items| items.len() <= policy.directory_page_size)?;
    let validators: Vec<DirectoryValidator> =
        items.iter().map(validator_of).collect::<Option<_>>()?;
    let mut seen = HashSet::new();
    validators
        .iter()
        .all(|validator| seen.insert(validator.staker))
        .then_some(DirectoryPage {
            coverage,
            next_cursor,
            validators,
        })
}

/// A percentage from hundredths of a percent, as `(x / 100).toFixed(2)` with trailing zeros
/// dropped (`"10%"`, `"99.5%"`, `"12.34%"`).
pub(crate) fn hundredths_percent(hundredths: u64) -> String {
    let (whole, fraction) = (hundredths / 100, hundredths % 100);
    if fraction == 0 {
        format!("{whole}%")
    } else {
        format!(
            "{whole}.{}%",
            format!("{fraction:02}").trim_end_matches('0')
        )
    }
}

/// `ppmPercent`: parts per million as a percentage, rounded down to 0.01%.
pub fn ppm_percent(ppm: u32) -> String {
    hundredths_percent(u64::from(ppm / 100))
}

/// One tracked token's pool as an entry shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryPoolEntry {
    /// The pool's total delegated, in the wallet's own decimals; `None` when unknown.
    pub delegated: Option<String>,
    pub symbol: String,
}

/// `StakingDirectoryEntry`: one validator as the screen lists it, display strings only. Nothing
/// here is signed or used to build a call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryEntry {
    pub commission: Option<String>,
    pub name: Option<String>,
    pub pools: Vec<DirectoryPoolEntry>,
    /// The canonical staker address.
    pub staker: String,
    pub uptime: Option<String>,
    pub website: Option<String>,
}

/// `directoryEntries`: the validators worth offering, status `active` with an active pool for at
/// least one of `tokens` on `network`, in the directory's order. Delegated totals are formatted
/// in the wallet's own decimals, never the directory's.
pub fn directory_entries(
    page: &DirectoryPage,
    tokens: &[TokenConfig],
    network: NetworkId,
) -> Vec<DirectoryEntry> {
    let same = |address: &str, felt: &Felt| same_felt(address, &felt::canonical(felt));
    page.validators
        .iter()
        .filter(|validator| validator.status == Some(ValidatorStatus::Active))
        .filter_map(|validator| {
            let pools: Vec<DirectoryPoolEntry> = tokens
                .iter()
                .filter(|token| token.network == network)
                .filter(|token| {
                    validator
                        .pools
                        .iter()
                        .any(|pool| pool.active && same(&token.address, &pool.token))
                })
                .map(|token| DirectoryPoolEntry {
                    delegated: validator
                        .delegated
                        .iter()
                        .find(|(stake_token, _)| same(&token.address, stake_token))
                        .and_then(|(_, raw)| raw.as_deref())
                        .map(|raw| format_digits(raw, token.decimals, token.decimals)),
                    symbol: token.symbol.clone(),
                })
                .collect();
            (!pools.is_empty()).then(|| DirectoryEntry {
                commission: validator.commission_ppm.map(ppm_percent),
                name: validator.name.clone(),
                pools,
                staker: felt::canonical(&validator.staker),
                uptime: validator.liveness_30d_ppm.map(ppm_percent),
                website: validator.website.clone(),
            })
        })
        .collect()
}

/// `URLSearchParams` serialisation of one value (`application/x-www-form-urlencoded`).
fn form_encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'*' | b'-' | b'.' | b'_' => char::from(b).to_string(),
            b' ' => "+".into(),
            _ if b.is_ascii_alphanumeric() => char::from(b).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// One directory page's request (`getStakingValidators`): `{base}/{chain}/staking/validators?
/// limit=<page size>[&cursor=…]` on the explorer lane's `api_base`. `Ok(None)` is "off" (no
/// directory on this network); a cursor outside `is_directory_cursor` is never echoed.
pub fn directory_request(
    network: NetworkId,
    policy: &StakingPolicy,
    api_base: &str,
    cursor: Option<&str>,
) -> Result<Option<HttpRequest>, StakingRefusal> {
    if cursor.is_some_and(|cursor| !is_directory_cursor(cursor)) {
        return Err(StakingRefusal::InvalidRequest);
    }
    let staking = policy.network(network).ok_or(StakingRefusal::Unavailable)?;
    if !staking.starkscan {
        return Ok(None);
    }
    let mut url = format!(
        "{api_base}/{}/staking/validators?limit={}",
        network.catalog_id(),
        policy.directory_page_size
    );
    if let Some(cursor) = cursor {
        url.push_str("&cursor=");
        url.push_str(&form_encode(cursor));
    }
    Ok(Some(starkscan_get(url)))
}

/// `StakingDirectoryView` past "off": a snapshot Starkscan reports as incomplete (no validators
/// offered), or one page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectoryView {
    Incomplete,
    Ready {
        entries: Vec<DirectoryEntry>,
        next_cursor: Option<String>,
    },
}

/// Judges a directory reply (`stakingDirectoryResponse`): any transport, status, size or schema
/// failure is `DirectoryUnreadable`, and the screen offers the pasted address.
pub fn directory_view(
    outcome: &HttpOutcome,
    network: NetworkId,
    tokens: &[TokenConfig],
    policy: &StakingPolicy,
) -> Result<DirectoryView, StakingRefusal> {
    let page = starkscan_json(outcome)
        .and_then(|value| parse_directory_page(&value, network, policy))
        .ok_or(StakingRefusal::DirectoryUnreadable)?;
    Ok(match page.coverage {
        Coverage::Incomplete => DirectoryView::Incomplete,
        Coverage::Usable => DirectoryView::Ready {
            entries: directory_entries(&page, tokens, network),
            next_cursor: page.next_cursor,
        },
    })
}
