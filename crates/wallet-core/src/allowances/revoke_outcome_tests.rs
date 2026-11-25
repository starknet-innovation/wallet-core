// `allowance-revoke-outcome.test.ts` (the extension's root `tests/background/portfolio/`): a
// submitted revoke's outcome, read from the wallet's own activity rows, and how the next listing
// settles the record on it and on readings.

use super::calls::{record_changes, record_submitted, AllowanceChange, ChangeKind};
use super::calls_tests::*;
use super::discovery::AllowanceReading;
use super::record::AllowanceInput;
use super::retention::Ranking;
use super::revoke::*;
use super::revoke_tests::{build, reading, resolve, LONG_SPENDER};
use super::store::{AllowanceLog, RevokeOutcome, RevokeStatus};
use super::TEST_POLICY;
use crate::history::activity::{ActivityEntry, ActivityKind, ActivityOrigin, TxStatus};
use crate::host::testing::Memory;
use crate::send::plan::PendingKind;
use crate::send::u256::U256;
use crate::NetworkId;

const APPROVE_HASH: &str = "0x1001";
const REVOKE_HASH: &str = "0x1002";

fn entry(account_id: &str, network: NetworkId, hash: &str, status: TxStatus) -> ActivityEntry {
    ActivityEntry {
        account_id: account_id.into(),
        amount: None,
        counterparty: None,
        direction: None,
        failure_notice: None,
        fee_payer: None,
        user_fee: None,
        relay: None,
        fee_paid: None,
        finalized_on_l1: None,
        hash: hash.into(),
        kind: ActivityKind::Invoke,
        network,
        not_found_count: None,
        origin: Some(ActivityOrigin::Wallet),
        status,
        submitted_at: 1,
        submitted_price: None,
        summary: "Revoke".into(),
        token: None,
        token_address: None,
    }
}

fn outcomes(log: &AllowanceLog, entries: &[ActivityEntry]) -> Vec<RevokeOutcome> {
    revoke_outcomes(&log.list().unwrap(), NetworkId::Mainnet, entries)
}

// Grants through a submitted batch, then revokes the grant as the manager does: resolve, build,
// and file the changes the plan's pending record carries under the revoke's hash.
fn grant_then_revoke(storage: &Memory) -> (AllowanceLog<'_>, String) {
    let lists = Lists::standard();
    let mut log = AllowanceLog::new(storage, TEST_POLICY);
    let calls = [grant(TOKEN, "approve", LONG_SPENDER, 1000, 0)];
    let ranking = Ranking::default();
    record_submitted(
        &mut log,
        &calls,
        &owner(),
        &lists.known(),
        APPROVE_HASH,
        &ranking,
    );
    let id = log.list().unwrap()[0].id.clone();
    let target = resolve(&id, &log.list().unwrap(), &lists.known()).unwrap();
    let invoke = build(&target, &lists.known(), &[]).unwrap();
    let PendingKind::Invoke { allowances, .. } = invoke.pending_kind() else {
        panic!("a revoke is a call batch");
    };
    record_changes(&mut log, &allowances, REVOKE_HASH, &ranking);
    (log, id)
}

fn shape(log: &AllowanceLog) -> Vec<(U256, bool, Option<String>)> {
    log.list()
        .unwrap()
        .into_iter()
        .map(|r| (r.amount, r.revoking, r.revoke_hash))
        .collect()
}

#[test]
fn a_revoke_carries_exactly_the_zero_approve_for_its_pair() {
    let lists = Lists::standard();
    let id = format!("{ACCOUNT}:{CANONICAL_TOKEN}:{LONG_SPENDER}");
    let target = resolve(&id, &[], &lists.known()).unwrap();
    let invoke = build(&target, &lists.known(), &[]).unwrap();
    assert_eq!(
        invoke.pending_kind(),
        PendingKind::Invoke {
            asset: None,
            allowances: vec![AllowanceChange {
                kind: ChangeKind::Set,
                input: AllowanceInput {
                    account_id: ACCOUNT.into(),
                    network: NetworkId::Mainnet,
                    token: CANONICAL_TOKEN.into(),
                    spender: LONG_SPENDER.into(),
                    amount: U256::ZERO,
                    lower_bound: false,
                },
            }],
        }
    );
}

#[test]
fn the_stored_revoke_records_the_hash_of_the_transaction_that_carried_it() {
    let storage = Memory::default();
    let (log, _) = grant_then_revoke(&storage);
    assert_eq!(
        shape(&log),
        [(amount("1000"), true, Some(REVOKE_HASH.to_string()))]
    );
}

#[test]
fn outcomes_come_only_from_the_wallets_own_settled_rows_in_the_revoking_scope() {
    let storage = Memory::default();
    let (log, _) = grant_then_revoke(&storage);
    let failed = |hash: &str| RevokeOutcome {
        hash: hash.into(),
        status: RevokeStatus::Failed,
    };
    for status in [TxStatus::Reverted, TxStatus::Rejected] {
        let row = entry(ACCOUNT, NetworkId::Mainnet, REVOKE_HASH, status);
        assert_eq!(outcomes(&log, &[row]), [failed(REVOKE_HASH)]);
    }
    let accepted = entry(ACCOUNT, NetworkId::Mainnet, REVOKE_HASH, TxStatus::Accepted);
    assert_eq!(
        outcomes(&log, &[accepted]),
        [RevokeOutcome {
            hash: REVOKE_HASH.into(),
            status: RevokeStatus::Accepted
        }]
    );
    // A padded spelling of the hash is the same transaction.
    let padded = entry(ACCOUNT, NetworkId::Mainnet, "0x01002", TxStatus::Reverted);
    assert_eq!(outcomes(&log, &[padded]), [failed(REVOKE_HASH)]);
    // "a pending / preconfirmed revoke keeps the pending mark".
    for status in [TxStatus::Pending, TxStatus::Preconfirmed] {
        let row = entry(ACCOUNT, NetworkId::Mainnet, REVOKE_HASH, status);
        assert!(outcomes(&log, &[row]).is_empty());
    }
    // "a chain-discovered row with the revoke's hash is not an outcome".
    let mut discovered = entry(ACCOUNT, NetworkId::Mainnet, REVOKE_HASH, TxStatus::Reverted);
    discovered.origin = None;
    // "a wallet row for the hash in another scope is not an outcome".
    let other = "SN_MAIN:0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdf";
    let elsewhere = entry(other, NetworkId::Mainnet, REVOKE_HASH, TxStatus::Reverted);
    // Beyond the extension's tests: another network, another hash, a hash that is not a number.
    let sepolia = entry(ACCOUNT, NetworkId::Sepolia, REVOKE_HASH, TxStatus::Reverted);
    let approve = entry(
        ACCOUNT,
        NetworkId::Mainnet,
        APPROVE_HASH,
        TxStatus::Reverted,
    );
    let garbled = entry(ACCOUNT, NetworkId::Mainnet, "nope", TxStatus::Reverted);
    assert!(outcomes(&log, &[discovered, elsewhere, sepolia, approve, garbled]).is_empty());
    // No pending revoke, no outcomes, whatever the rows say.
    let row = entry(ACCOUNT, NetworkId::Mainnet, REVOKE_HASH, TxStatus::Reverted);
    let plain = stored(ACCOUNT, NetworkId::Mainnet, CANONICAL_TOKEN, amount("5"));
    assert!(revoke_outcomes(&[plain], NetworkId::Mainnet, &[row]).is_empty());
}

fn settle(log: &mut AllowanceLog, readings: &[AllowanceReading], status: Option<TxStatus>) {
    let rows: Vec<_> = status
        .map(|s| entry(ACCOUNT, NetworkId::Mainnet, REVOKE_HASH, s))
        .into_iter()
        .collect();
    let settled = outcomes(log, &rows);
    log.settle_revocations(readings, &settled).unwrap();
}

#[test]
fn a_failed_revoke_clears_the_mark_and_keeps_the_grant() {
    for status in [TxStatus::Reverted, TxStatus::Rejected] {
        let storage = Memory::default();
        let (mut log, _) = grant_then_revoke(&storage);
        settle(&mut log, &[], Some(status));
        assert_eq!(shape(&log), [(amount("1000"), false, None)]);
    }
}

#[test]
fn an_accepted_revoke_settles_only_on_a_reading() {
    // "an accepted revoke with no chain reading keeps the row and its mark".
    let storage = Memory::default();
    let (mut log, id) = grant_then_revoke(&storage);
    settle(&mut log, &[], Some(TxStatus::Accepted));
    assert_eq!(
        shape(&log),
        [(amount("1000"), true, Some(REVOKE_HASH.to_string()))]
    );
    // "accepted, a stale non-zero reading, then zero: the row goes".
    settle(&mut log, &[reading(&id, "1000")], Some(TxStatus::Accepted));
    assert_eq!(shape(&log), [(amount("1000"), true, None)]);
    settle(&mut log, &[reading(&id, "0")], None);
    assert!(log.list().unwrap().is_empty());
    // "accepted with a genuine re-grant: the row shows the re-grant amount", and keeps it when
    // the next read fails.
    let storage = Memory::default();
    let (mut log, id) = grant_then_revoke(&storage);
    settle(&mut log, &[reading(&id, "250")], Some(TxStatus::Accepted));
    settle(&mut log, &[], None);
    assert_eq!(shape(&log), [(amount("250"), true, None)]);
}
