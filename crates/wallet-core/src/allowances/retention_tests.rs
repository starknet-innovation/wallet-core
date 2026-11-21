// `allowance-store-retention-risk.test.ts`, `allowance-store-scope-cap.test.ts`,
// `allowance-store-eviction-tokens.test.ts` and `allowance-store-budget.test.ts` (the extension's
// root `tests/storage/`), through the log as the extension drives them, plus the bounds they
// leave out. A token list that cannot be read is the caller's: it passes `Ranking::default()`.

use super::record::{allowance_id, allowance_scope, AllowanceInput};
use super::retention::{RankToken, Ranking};
use super::store::AllowanceLog;
use super::{AllowancePolicy, TEST_POLICY};
use crate::host::testing::Memory;
use crate::send::u256::U256;
use crate::NetworkId;

const ACCOUNT: &str = "SN_MAIN:0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcde";
const TOKEN: &str = "0x049d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7";
const CAP: usize = 50;
const TOTAL: usize = 200;

fn unlimited() -> U256 {
    U256::from_u128(u128::MAX)
}

fn spender_at(index: usize) -> String {
    format!("0x{:x}", index + 1)
}

fn grant(account: &str, index: usize, token: &str, amount: U256) -> AllowanceInput {
    let network = if account.starts_with("SN_SEPOLIA:") {
        NetworkId::Sepolia
    } else {
        NetworkId::Mainnet
    };
    AllowanceInput {
        account_id: account.into(),
        network,
        token: token.into(),
        spender: spender_at(index),
        amount,
        lower_bound: false,
    }
}

fn put(log: &mut AllowanceLog, entry: &AllowanceInput, ranking: &Ranking) {
    log.record(entry, None, ranking).unwrap();
}

fn fill(log: &mut AllowanceLog, account: &str, count: usize) {
    for index in 0..count {
        put(
            log,
            &grant(account, index, TOKEN, U256::from_u128(1000)),
            &Ranking::default(),
        );
    }
}

fn spenders(log: &AllowanceLog) -> Vec<String> {
    log.list().unwrap().into_iter().map(|r| r.spender).collect()
}

fn count_for(log: &AllowanceLog, account: &str) -> usize {
    let entries = log.list().unwrap();
    entries.iter().filter(|r| r.account_id == account).count()
}

fn id_of(account: &str, index: usize) -> String {
    allowance_id(account, TOKEN, &spender_at(index)).unwrap()
}

#[test]
fn eviction_drops_the_smallest_grant_not_the_oldest() {
    let untracked = Ranking::default();
    // Sherlock 183 "an old unlimited grant outlives later smaller ones".
    let storage = Memory::default();
    let mut log = AllowanceLog::new(&storage, TEST_POLICY);
    put(&mut log, &grant(ACCOUNT, 0, TOKEN, unlimited()), &untracked);
    fill_from(&mut log, 1, CAP, U256::from_u128(1000));
    let kept = spenders(&log);
    assert_eq!(kept.len(), CAP);
    assert!(kept.contains(&spender_at(0)));
    assert!(
        !kept.contains(&spender_at(1)),
        "the oldest of the equal grants goes"
    );
    // "a batch of one-wei approvals does not clear a full scope".
    let storage = Memory::default();
    let mut log = AllowanceLog::new(&storage, TEST_POLICY);
    fill_from(&mut log, 0, CAP - 1, U256::from_u128(1000));
    fill_from(&mut log, CAP, 2 * CAP - 1, U256::from_u128(1));
    let kept = spenders(&log);
    assert_eq!(kept.len(), CAP);
    assert!((0..CAP).all(|i| kept.contains(&spender_at(i))));
    // "a lower bound ranks at the figure it proves".
    let storage = Memory::default();
    let mut log = AllowanceLog::new(&storage, TEST_POLICY);
    let proven = AllowanceInput {
        lower_bound: true,
        ..grant(ACCOUNT, 0, TOKEN, U256::from_u128(5000))
    };
    put(&mut log, &proven, &untracked);
    fill_from(&mut log, 1, CAP, U256::from_u128(1000));
    assert!(spenders(&log).contains(&spender_at(0)));
    // Sherlock 192 "a pending revoke of an unlimited grant is not the first to be evicted".
    let storage = Memory::default();
    let mut log = AllowanceLog::new(&storage, TEST_POLICY);
    put(&mut log, &grant(ACCOUNT, 0, TOKEN, unlimited()), &untracked);
    put(&mut log, &grant(ACCOUNT, 0, TOKEN, U256::ZERO), &untracked);
    fill_from(&mut log, 1, CAP, U256::from_u128(1000));
    assert!(spenders(&log).contains(&spender_at(0)));
}

// Grants to spenders `first..=last` on the account's scope, untracked.
fn fill_from(log: &mut AllowanceLog, first: usize, last: usize, amount: U256) {
    for index in first..=last {
        put(
            log,
            &grant(ACCOUNT, index, TOKEN, amount),
            &Ranking::default(),
        );
    }
}

#[test]
fn a_scope_at_its_cap_and_one_over() {
    // Beyond the tests: exactly the cap evicts nothing; one more evicts one, the oldest.
    let storage = Memory::default();
    let mut log = AllowanceLog::new(&storage, TEST_POLICY);
    fill(&mut log, ACCOUNT, CAP);
    assert_eq!(count_for(&log, ACCOUNT), CAP);
    // scope-cap.test.ts "evicts only the oldest records of the writing scope".
    put(
        &mut log,
        &grant(ACCOUNT, CAP, TOKEN, U256::from_u128(1000)),
        &Ranking::default(),
    );
    let ids: Vec<String> = log.list().unwrap().into_iter().map(|r| r.id).collect();
    assert_eq!(ids.len(), CAP);
    assert!(!ids.contains(&id_of(ACCOUNT, 0)));
    assert!(ids.contains(&id_of(ACCOUNT, CAP)));
}

#[test]
fn one_scope_filling_never_evicts_another() {
    let other = "SN_MAIN:0x2234567890abcdef1234567890abcdef1234567890abcdef1234567890abcde";
    let sepolia = "SN_SEPOLIA:0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcde";
    // scope-cap.test.ts "keeps another account's allowances when one account fills the cap" and
    // "keeps another network's allowances when one network fills the cap".
    for neighbour in [other, sepolia] {
        let storage = Memory::default();
        let mut log = AllowanceLog::new(&storage, TEST_POLICY);
        fill(&mut log, neighbour, 1);
        fill(&mut log, ACCOUNT, CAP + 5);
        assert_eq!(count_for(&log, neighbour), 1);
        assert_eq!(count_for(&log, ACCOUNT), CAP);
    }
    // "the same address on two networks is two scopes and two ids".
    let main = grant(ACCOUNT, 0, TOKEN, U256::from_u128(1));
    let test = grant(sepolia, 0, TOKEN, U256::from_u128(1));
    assert_ne!(
        allowance_scope(&main.account_id, main.network),
        allowance_scope(&test.account_id, test.network)
    );
}

fn saturating_accounts() -> Vec<String> {
    (0..TOTAL.div_ceil(CAP))
        .map(|i| {
            format!(
                "SN_MAIN:0x{:x}234567890abcdef1234567890abcdef1234567890abcdef123456789",
                i + 3
            )
        })
        .collect()
}

const LATE: &str = "SN_MAIN:0x9234567890abcdef1234567890abcdef1234567890abcdef123456789";

#[test]
fn the_total_cap_is_shared_fairly() {
    // scope-cap.test.ts "a newly busy account cannot empty an idle account's allowance list" and
    // "evicting for the total cap still takes a scope's oldest record first".
    let storage = Memory::default();
    let mut log = AllowanceLog::new(&storage, TEST_POLICY);
    let accounts = saturating_accounts();
    for account in &accounts {
        fill(&mut log, account, CAP);
    }
    assert_eq!(log.list().unwrap().len(), TOTAL);
    fill(&mut log, LATE, CAP);
    assert!(log.list().unwrap().len() <= TOTAL);
    let shares: Vec<usize> = accounts
        .iter()
        .map(String::as_str)
        .chain([LATE])
        .map(|account| count_for(&log, account))
        .collect();
    let (min, max) = (shares.iter().min().unwrap(), shares.iter().max().unwrap());
    assert!(*min > 0);
    assert!(max - min <= CAP / 10, "{shares:?}");
    let ids: Vec<String> = log.list().unwrap().into_iter().map(|r| r.id).collect();
    for account in &accounts {
        assert!(!ids.contains(&id_of(account, 0)));
        assert!(ids.contains(&id_of(account, CAP - 1)));
    }
}

#[test]
fn the_total_cap_keeps_the_latest_grant_and_trims_an_oversized_log() {
    // budget.test.ts "a freshly granted allowance survives even when the total cap binds" and
    // "record never rejects a write while several accounts saturate their scope".
    let storage = Memory::default();
    let mut log = AllowanceLog::new(&storage, TEST_POLICY);
    let accounts: Vec<String> = (0..8)
        .map(|i| format!("SN_SEPOLIA:0x{:x}", 0x7777 + i))
        .collect();
    for account in &accounts {
        for index in 0..CAP {
            put(
                &mut log,
                &grant(account, index, TOKEN, U256::from_u128(9)),
                &Ranking::default(),
            );
            assert_eq!(log.list().unwrap()[0].id, id_of(account, index));
        }
    }
    assert_eq!(log.list().unwrap().len(), TOTAL);
    assert!(accounts.iter().all(|a| count_for(&log, a) > 0));
    // "a log persisted over the cap by an older build is trimmed on the next write".
    let small = AllowancePolicy {
        entries_total: 150,
        ..TEST_POLICY
    };
    let mut log = AllowanceLog::new(&storage, small);
    assert_eq!(log.list().unwrap().len(), TOTAL);
    fill(&mut log, ACCOUNT, 1);
    assert_eq!(log.list().unwrap().len(), 150);
}

// eviction-tokens.test.ts's two tracked tokens of different decimals, and an untracked one.
const SIX: &str = "0xa6";
const EIGHTEEN: &str = "0xa18";
const UNTRACKED: &str = "0xbad";

fn tracked() -> Ranking {
    let token = |address: &str, decimals| RankToken {
        network: NetworkId::Mainnet,
        address: address.into(),
        decimals,
    };
    Ranking::new(&[token(SIX, 6), token(EIGHTEEN, 18), token("0xd06", 256)])
}

fn half_eighteen() -> U256 {
    U256::from_u128(5 * 10u128.pow(17))
}

fn thousand_six() -> U256 {
    U256::from_u128(1000 * 10u128.pow(6))
}

fn tokens_kept(log: &AllowanceLog) -> Vec<String> {
    log.list().unwrap().into_iter().map(|r| r.token).collect()
}

#[test]
fn eviction_compares_tracked_tokens_in_whole_tokens() {
    let ranking = tracked();
    let scoped = |ranking: &Ranking, extra: (&str, U256)| {
        let storage = Memory::default();
        let mut log = AllowanceLog::new(&storage, TEST_POLICY);
        for index in 0..CAP {
            put(
                &mut log,
                &grant(ACCOUNT, index, EIGHTEEN, half_eighteen()),
                ranking,
            );
        }
        put(&mut log, &grant(ACCOUNT, CAP, extra.0, extra.1), ranking);
        let kept = tokens_kept(&log);
        assert_eq!(kept.len(), CAP);
        kept
    };
    // "compares tracked tokens in whole tokens, not raw base units".
    assert!(scoped(&ranking, (SIX, thousand_six())).contains(&SIX.to_string()));
    // "a token list that cannot be read ranks everything as untracked": raw order.
    let raw = Ranking::default();
    assert!(!scoped(&raw, (SIX, thousand_six())).contains(&SIX.to_string()));
    // A token claiming more than 255 decimals ranks as untracked (MAX_RANK_DECIMALS).
    assert!(!scoped(&ranking, ("0xd06", unlimited_minus_one())).contains(&"0xd06".to_string()));
}

fn unlimited_minus_one() -> U256 {
    U256::from_u128(u128::MAX - 1)
}

#[test]
fn untracked_tokens_go_first_and_unlimited_grants_last() {
    let ranking = tracked();
    let big = U256::from_u128(10u128.pow(30));
    // "evicts an untracked token before any tracked one, however large its raw amount".
    let storage = Memory::default();
    let mut log = AllowanceLog::new(&storage, TEST_POLICY);
    for index in 0..CAP {
        put(
            &mut log,
            &grant(ACCOUNT, index, SIX, U256::from_u128(1)),
            &ranking,
        );
    }
    put(&mut log, &grant(ACCOUNT, CAP, UNTRACKED, big), &ranking);
    assert!(!tokens_kept(&log).contains(&UNTRACKED.to_string()));
    // "keeps an unlimited grant last, even on an untracked token".
    let storage = Memory::default();
    let mut log = AllowanceLog::new(&storage, TEST_POLICY);
    put(
        &mut log,
        &grant(ACCOUNT, 0, UNTRACKED, unlimited()),
        &ranking,
    );
    for index in 1..=CAP {
        put(&mut log, &grant(ACCOUNT, index, SIX, big), &ranking);
    }
    assert!(tokens_kept(&log).contains(&UNTRACKED.to_string()));
    // Two unlimited grants compare by raw amount: 2^256 - 1 outranks 2^128 - 1.
    let storage = Memory::default();
    let mut log = AllowanceLog::new(&storage, TEST_POLICY);
    let max = U256 {
        high: u128::MAX,
        low: u128::MAX,
    };
    put(&mut log, &grant(ACCOUNT, 0, SIX, max), &ranking);
    for index in 1..=CAP {
        put(&mut log, &grant(ACCOUNT, index, SIX, unlimited()), &ranking);
    }
    assert!(spenders(&log).contains(&spender_at(0)));
    assert!(!spenders(&log).contains(&spender_at(1)));
}

#[test]
fn the_total_cap_ranks_by_the_same_rule() {
    // eviction-tokens.test.ts "the total cap ranks by the same rule".
    let ranking = tracked();
    let storage = Memory::default();
    let mut log = AllowanceLog::new(&storage, TEST_POLICY);
    let per_scope = TOTAL / 5;
    let scopes: Vec<String> = (0..5)
        .map(|scope| format!("{}{scope}", &ACCOUNT[..ACCOUNT.len() - 1]))
        .collect();
    for account in &scopes {
        for index in 0..per_scope {
            put(
                &mut log,
                &grant(account, index, EIGHTEEN, half_eighteen()),
                &ranking,
            );
        }
    }
    put(
        &mut log,
        &grant(&scopes[0], per_scope, SIX, thousand_six()),
        &ranking,
    );
    let entries = log.list().unwrap();
    assert_eq!(entries.len(), TOTAL);
    assert!(entries.iter().any(|r| r.token == SIX));
}
