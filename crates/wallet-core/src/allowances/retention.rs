//! Which records the log keeps at a bound (`allowance-retention.ts`). Inside a scope the least
//! risky allowances go first, and among equally risky ones the oldest written (Sherlock 183);
//! across scopes the fullest scopes give up records first, so one busy account never empties an
//! idle one. Risk is ranked across tokens (issue 647): untracked tokens first, then tracked
//! tokens by their decimal-normalised amount, unlimited grants last whatever their token.

use std::cmp::Ordering;
use std::collections::HashMap;

use super::record::{is_unlimited, AllowanceRecord};
use crate::js::felt::canonical_felt;
use crate::NetworkId;

/// `MAX_RANK_DECIMALS`: a token claiming more ranks as untracked.
const MAX_RANK_DECIMALS: u32 = 255;

/// A token the wallet trusts, as ranking reads it (`AllowanceRankToken`). Pass only the tokens
/// the account trusts: a dapp-named token could claim few decimals to outrank real grants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankToken {
    pub network: NetworkId,
    pub address: String,
    pub decimals: u32,
}

/// The decimals of each trusted token, for one write (`rankingFor`). The default ranks every
/// record as untracked: raw amounts, unlimited grants last.
#[derive(Debug, Clone, Default)]
pub struct Ranking(HashMap<String, u32>);

fn token_key(network: NetworkId, address: &str) -> String {
    let address = canonical_felt(address).unwrap_or_else(|| address.to_string());
    format!("{}|{address}", network.catalog_id())
}

impl Ranking {
    pub fn new(tokens: &[RankToken]) -> Self {
        Self(
            tokens
                .iter()
                .filter(|token| token.decimals <= MAX_RANK_DECIMALS)
                .map(|token| (token_key(token.network, &token.address), token.decimals))
                .collect(),
        )
    }

    fn decimals(&self, record: &AllowanceRecord) -> Option<u32> {
        self.0
            .get(&token_key(record.network, &record.token))
            .copied()
    }

    // 0: untracked, evicted first; 1: tracked; 2: unlimited, evicted last.
    fn tier(record: &AllowanceRecord, decimals: Option<u32>) -> u8 {
        match (is_unlimited(record.amount), decimals) {
            (true, _) => 2,
            (false, None) => 0,
            (false, Some(_)) => 1,
        }
    }

    // `compareRisk`: `Greater` when `left` is the riskier grant. Two tracked grants compare in
    // whole tokens, a/10^da against b/10^db as a*10^db against b*10^da; anything else in one
    // tier by raw amount. A lower bound ranks at the figure it proves.
    fn compare_risk(&self, left: &AllowanceRecord, right: &AllowanceRecord) -> Ordering {
        let (left_decimals, right_decimals) = (self.decimals(left), self.decimals(right));
        let tier = Self::tier(left, left_decimals).cmp(&Self::tier(right, right_decimals));
        if tier != Ordering::Equal {
            return tier;
        }
        match (
            Self::tier(left, left_decimals),
            left_decimals,
            right_decimals,
        ) {
            (1, Some(da), Some(db)) => scaled_cmp(left, db, right, da),
            _ => scaled_cmp(left, 0, right, 0),
        }
    }
}

// `a * 10^za` against `b * 10^zb`, exactly: a power of ten appends zeros to the decimal digits,
// so the larger product is the longer numeral, or at equal length the greater one.
fn scaled_cmp(a: &AllowanceRecord, za: u32, b: &AllowanceRecord, zb: u32) -> Ordering {
    let digits = |record: &AllowanceRecord, zeros: u32| {
        let text = record.amount.to_decimal();
        if text == "0" {
            text
        } else {
            text + &"0".repeat(zeros as usize)
        }
    };
    let (a, b) = (digits(a, za), digits(b, zb));
    a.len().cmp(&b.len()).then_with(|| a.cmp(&b))
}

/// `keepLargest`, as a mask over `records`: the `limit` riskiest are kept, dropping the least
/// risky first and, among equal ones, the oldest written (the reversed input is sorted stably,
/// so the newer of two equal records ranks first).
pub fn keep_largest(records: &[&AllowanceRecord], limit: usize, ranking: &Ranking) -> Vec<bool> {
    if records.len() <= limit {
        return vec![true; records.len()];
    }
    let mut order: Vec<usize> = (0..records.len()).rev().collect();
    order.sort_by(|&left, &right| ranking.compare_risk(records[right], records[left]));
    let mut keep = vec![false; records.len()];
    for index in order.into_iter().take(limit) {
        keep[index] = true;
    }
    keep
}

// `scopeBudgets`: each round levels down every scope holding the most, so the scopes that caused
// the overflow pay for it and the survivors end within one entry of an equal share.
fn scope_budgets(counts: &[usize], limit: usize) -> Vec<usize> {
    let mut budgets = counts.to_vec();
    let mut total: usize = budgets.iter().sum();
    while total > limit {
        let tallest = budgets.iter().copied().max().unwrap_or(0);
        for budget in budgets.iter_mut() {
            if *budget == tallest && total > limit {
                *budget -= 1;
                total -= 1;
            }
        }
    }
    budgets
}

/// `trimToTotal`: fits the log in `limit` entries, taking evictions from the fullest scopes and,
/// inside each, from its least risky allowances.
pub fn trim_to_total(
    entries: Vec<AllowanceRecord>,
    limit: usize,
    ranking: &Ranking,
) -> Vec<AllowanceRecord> {
    if entries.len() <= limit {
        return entries;
    }
    // Scopes in the order they first appear, as a JavaScript `Map` iterates.
    let mut scopes: Vec<(String, Vec<usize>)> = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let scope = entry.scope();
        match scopes.iter_mut().find(|(known, _)| *known == scope) {
            Some((_, members)) => members.push(index),
            None => scopes.push((scope, vec![index])),
        }
    }
    let counts: Vec<usize> = scopes.iter().map(|(_, members)| members.len()).collect();
    let mut keep = vec![false; entries.len()];
    for ((_, members), budget) in scopes.iter().zip(scope_budgets(&counts, limit)) {
        let records: Vec<&AllowanceRecord> = members.iter().map(|&i| &entries[i]).collect();
        for (&index, kept) in members.iter().zip(keep_largest(&records, budget, ranking)) {
            keep[index] = kept;
        }
    }
    entries
        .into_iter()
        .zip(keep)
        .filter_map(|(entry, keep)| keep.then_some(entry))
        .collect()
}
