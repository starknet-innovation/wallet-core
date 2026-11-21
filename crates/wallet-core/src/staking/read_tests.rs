//! `StakingRead` against the extension's staking chain (`staking-chain.ts`): the facts, the
//! verified pool and membership, the refusals of `readStakingPool`, and the pinned-read
//! contract (`withPinnedBlock`, `withRpcFallback`).

use serde_json::json;

use super::decode::{PoolMember, VerifiedPool};
use super::read::*;
use super::test_chain::*;
use super::StakingRefusal;
use crate::history::events::ScanStep;
use crate::portfolio::wire::RpcReply;
use crate::Felt;

fn only(reading: &StakingReading) -> &PoolOutcome {
    assert_eq!(reading.pools.len(), 1);
    &reading.pools[0].outcome
}

#[test]
fn selectors_are_starknet_keccak() {
    use sha3::{Digest, Keccak256};
    for (name, expected) in [
        ("get_active_tokens", GET_ACTIVE_TOKENS),
        ("contract_parameters_v1", CONTRACT_PARAMETERS_V1),
        ("staker_pool_info", STAKER_POOL_INFO),
        ("get_staker_info_v1", GET_STAKER_INFO_V1),
        ("get_pool_member_info_v1", GET_POOL_MEMBER_INFO_V1),
    ] {
        let mut digest: [u8; 32] = Keccak256::digest(name.as_bytes()).into();
        digest[0] &= 0x03;
        assert_eq!(Felt::from_bytes_be(&digest), f(expected), "{name}");
    }
}

#[test]
fn reads_the_verified_pool_and_the_membership_at_one_pinned_block() {
    let mut chain = chain();
    chain.member = Some(Member {
        unclaimed: 25 * 10u128.pow(16),
        ..member()
    });
    let reading = run(read_of(vec![strk_pair()]), &mut chain).unwrap();
    assert!(reading.is_for(&f(ACCOUNT), crate::NetworkId::Mainnet));
    assert_eq!(
        reading.facts,
        Some(StakingFacts {
            active_tokens: vec![f(STRK)],
            exit_window_seconds: 604_800,
        })
    );
    assert_eq!(
        only(&reading),
        &PoolOutcome::Verified {
            pool: VerifiedPool {
                accepting: true,
                commission: 1000,
                delegated: 10u128.pow(21),
                pool: f(STAKING_POOL),
            },
            member: Some(PoolMember {
                amount: 5 * 10u128.pow(18),
                reward_address: f(ACCOUNT),
                unclaimed_rewards: 25 * 10u128.pow(16),
                unpool_amount: 0,
                unpool_time: None,
            }),
        }
    );
    assert_eq!(
        chain.reads,
        [
            "get_active_tokens",
            "contract_parameters_v1",
            "staker_pool_info",
            "get_staker_info_v1",
            "contract_parameters_v1",
            "get_pool_member_info_v1",
        ]
    );
    assert!(chain
        .pins
        .iter()
        .all(|pin| *pin == json!({ "block_hash": BLOCK })));
}

#[test]
fn a_staking_contract_paying_anything_but_strk_has_no_facts() {
    let mut chain = chain();
    chain.reward_token = Felt::from(0x777u32);
    let reading = run(read_of(vec![strk_pair()]), &mut chain).unwrap();
    assert_eq!(reading.facts, None);
    assert!(reading.pools.is_empty());
    assert_eq!(chain.reads.len(), 2, "nothing is read past the facts");
}

#[test]
fn a_pool_naming_another_staker_is_refused() {
    let mut chain = chain();
    chain.pool_staker = Felt::from(0x999u32);
    let reading = run(read_of(vec![strk_pair()]), &mut chain).unwrap();
    assert_eq!(
        only(&reading),
        &PoolOutcome::Refused(StakingRefusal::Unverified)
    );
}

#[test]
fn a_staker_the_staking_contract_does_not_list_is_refused_after_asking() {
    let mut chain = chain();
    let pair = StakingPair {
        staker: Felt::from(0x5a4u32),
        token: f(STRK),
    };
    let reading = run(read_of(vec![pair]), &mut chain).unwrap();
    assert_eq!(
        only(&reading),
        &PoolOutcome::Refused(StakingRefusal::Unverified)
    );
    assert!(chain.reads.contains(&"staker_pool_info"));
    assert!(!chain.reads.contains(&"get_pool_member_info_v1"));
}

#[test]
fn a_token_the_staker_has_no_pool_for_is_refused() {
    let mut chain = chain();
    let eth = Felt::from(0xe7au32);
    chain.active_tokens.push(eth);
    let pair = StakingPair {
        staker: f(STAKER),
        token: eth,
    };
    let reading = run(read_of(vec![pair]), &mut chain).unwrap();
    assert_eq!(
        only(&reading),
        &PoolOutcome::Refused(StakingRefusal::NoPoolForToken)
    );
}

#[test]
fn a_token_the_staking_contract_does_not_take_is_not_read() {
    let mut chain = chain();
    chain.active_tokens = vec![Felt::from(0x777u32)];
    let reading = run(read_of(vec![strk_pair()]), &mut chain).unwrap();
    assert_eq!(only(&reading), &PoolOutcome::Inactive);
    assert_eq!(chain.reads, ["get_active_tokens", "contract_parameters_v1"]);
}

#[test]
fn an_unreadable_membership_is_refused() {
    let mut chain = chain();
    chain.member = Some(Member {
        reward_address: Felt::ZERO,
        ..member()
    });
    let reading = run(read_of(vec![strk_pair()]), &mut chain).unwrap();
    assert_eq!(
        only(&reading),
        &PoolOutcome::Refused(StakingRefusal::MemberUnreadable)
    );
}

#[test]
fn a_leaving_validator_is_verified_but_not_accepting() {
    let mut chain = chain();
    chain.staker_leaving = true;
    let reading = run(read_of(vec![strk_pair()]), &mut chain).unwrap();
    assert!(matches!(
        only(&reading),
        PoolOutcome::Verified { pool, member: None } if !pool.accepting
    ));
}

#[test]
fn pairs_keep_their_order_and_a_staker_is_asked_once() {
    let mut chain = chain();
    let other = Felt::from(0x700u32);
    chain.extra_stakers = vec![other];
    let inactive = StakingPair {
        staker: f(STAKER),
        token: Felt::from(0x777u32),
    };
    let pairs = vec![
        StakingPair {
            staker: other,
            token: f(STRK),
        },
        inactive,
        strk_pair(),
    ];
    let reading = run(read_of(pairs.clone()), &mut chain).unwrap();
    let read: Vec<StakingPair> = reading.pools.iter().map(|p| p.pair).collect();
    assert_eq!(read, pairs);
    assert_eq!(reading.pools[1].outcome, PoolOutcome::Inactive);
    assert!(matches!(
        reading.pools[0].outcome,
        PoolOutcome::Verified { pool, .. } if pool.pool == Felt::from(0x5b00u32)
    ));
    let twice = run(
        read_of(vec![strk_pair(), strk_pair()]),
        &mut super::test_chain::chain(),
    );
    assert_eq!(twice.map(|r| r.pools.len()), Some(2));
    let mut counted = super::test_chain::chain();
    run(read_of(vec![strk_pair(), strk_pair()]), &mut counted);
    let asked = counted
        .reads
        .iter()
        .filter(|r| **r == "staker_pool_info")
        .count();
    assert_eq!(asked, 1);
}

#[test]
fn a_missing_block_is_re_pinned_at_most_three_times() {
    // Each pin's facts stage sends two calls; the second answer after a miss is stale. Three
    // misses are two re-pins, then the third pin reads.
    let mut chain = chain();
    chain.block_misses = 3;
    assert!(run(read_of(vec![strk_pair()]), &mut chain).is_some());
    // Five are a miss on each of three pins: the read fails closed.
    let mut chain = super::test_chain::chain();
    chain.block_misses = 5;
    assert_eq!(run(read_of(vec![strk_pair()]), &mut chain), None);
}

fn reply(id: u64, body: serde_json::Value) -> RpcReply {
    RpcReply {
        id,
        status: 200,
        body: Some(body.to_string()),
    }
}

fn pinned(read: &mut StakingRead) {
    let calls = read.next_calls();
    assert_eq!(calls.len(), 1);
    let block = json!({ "jsonrpc": "2.0", "id": calls[0].id, "result": { "block_hash": BLOCK, "block_number": 7 } });
    assert_eq!(read.accept(reply(calls[0].id, block)), ScanStep::Continue);
}

#[test]
fn any_other_node_error_fails_the_read() {
    let mut read = read_of(vec![strk_pair()]);
    pinned(&mut read);
    let calls = read.next_calls();
    let error =
        json!({ "jsonrpc": "2.0", "id": calls[0].id, "error": { "code": 40, "message": "x" } });
    assert_eq!(read.accept(reply(calls[0].id, error)), ScanStep::Continue);
    assert!(read.next_calls().is_empty());
    assert_eq!(read.finish(), None);
    // "Block not found" for the pin itself is not a stale pin: it fails too.
    let mut read = read_of(vec![strk_pair()]);
    let calls = read.next_calls();
    let missing =
        json!({ "jsonrpc": "2.0", "id": calls[0].id, "error": { "code": 24, "message": "x" } });
    read.accept(reply(calls[0].id, missing));
    assert_eq!(read.finish(), None);
    assert!(read.next_calls().is_empty());
}

#[test]
fn a_malformed_block_or_a_result_that_is_not_an_array_fails_the_read() {
    for block in [
        json!({ "block_hash": "0x0", "block_number": 7 }),
        json!({ "block_hash": BLOCK, "block_number": -1 }),
        json!({ "block_hash": BLOCK, "block_number": 1.5 }),
        json!({ "block_hash": BLOCK }),
        json!({ "block_number": 7 }),
    ] {
        let mut read = read_of(vec![strk_pair()]);
        let id = read.next_calls()[0].id;
        read.accept(reply(
            id,
            json!({ "jsonrpc": "2.0", "id": id, "result": block }),
        ));
        assert_eq!(read.finish(), None, "{block}");
        assert!(read.next_calls().is_empty());
    }
    let mut chain = chain();
    let mut read = read_of(vec![strk_pair()]);
    pinned(&mut read);
    let calls = read.next_calls();
    let id = calls[0].id;
    read.accept(reply(
        id,
        json!({ "jsonrpc": "2.0", "id": id, "result": "0x1" }),
    ));
    // The stage's other answer changes nothing: the read has already failed.
    read.accept(chain.reply(&calls[1]));
    assert_eq!(read.finish(), None);
}

#[test]
fn a_pool_listed_for_a_token_the_contract_does_not_take_is_never_read() {
    // `STAKER` also lists a pool for a token outside the active list; that pool answers nothing,
    // so reading it would fail the whole read.
    let mut chain = chain();
    let other = Felt::from(0xe7au32);
    chain.extra_pools = vec![(other, Felt::from(0xdeadu32))];
    let inactive = StakingPair {
        staker: f(STAKER),
        token: other,
    };
    let reading = run(read_of(vec![inactive, strk_pair()]), &mut chain).unwrap();
    assert_eq!(reading.pools[0].outcome, PoolOutcome::Inactive);
    assert!(matches!(
        reading.pools[1].outcome,
        PoolOutcome::Verified { .. }
    ));
}

#[test]
fn a_restart_gives_the_next_endpoint_its_own_three_pins() {
    let mut chain = chain();
    // Two pins miss on the first endpoint (each facts stage's second answer is stale).
    chain.block_misses = 4;
    let mut read = read_of(vec![strk_pair()]);
    for _ in 0..2 {
        let pin = read.next_calls();
        assert_eq!(read.accept(chain.reply(&pin[0])), ScanStep::Continue);
        for call in read.next_calls() {
            assert_eq!(read.accept(chain.reply(&call)), ScanStep::Continue);
        }
    }
    // The endpoint then fails at transport level; the next one misses twice more and reads.
    let pin = read.next_calls();
    let failed = RpcReply {
        id: pin[0].id,
        status: 0,
        body: None,
    };
    assert_eq!(read.accept(failed), ScanStep::Transport);
    read.restart();
    chain.block_misses = 4;
    assert!(run(read, &mut chain).is_some());
}

#[test]
fn a_transport_failure_replays_on_the_next_endpoint() {
    let mut chain = chain();
    let mut read = read_of(vec![strk_pair()]);
    pinned(&mut read);
    let calls = read.next_calls();
    let failed = RpcReply {
        id: calls[0].id,
        status: 429,
        body: Some("{}".into()),
    };
    assert_eq!(read.accept(failed), ScanStep::Transport);
    read.restart();
    // The first endpoint's late reply is stale after the restart.
    assert_eq!(read.accept(chain.reply(&calls[1])), ScanStep::Continue);
    assert!(run(read, &mut chain).is_some());
}

#[test]
fn a_reply_to_nothing_sent_or_answered_twice_is_a_transport_failure() {
    let mut chain = chain();
    let mut read = read_of(vec![strk_pair()]);
    pinned(&mut read);
    let calls = read.next_calls();
    let beyond = RpcReply {
        id: calls[1].id + 1,
        ..chain.reply(&calls[1])
    };
    assert_eq!(read.accept(beyond), ScanStep::Transport);
    assert_eq!(read.accept(chain.reply(&calls[0])), ScanStep::Continue);
    assert_eq!(read.accept(chain.reply(&calls[0])), ScanStep::Transport);
    // Nothing is handed out twice while replies are outstanding.
    assert!(read.next_calls().is_empty());
    let mut fresh = read_of(vec![strk_pair()]);
    assert_eq!(
        fresh.accept(reply(1, json!({ "jsonrpc": "2.0", "id": 1, "result": [] }))),
        ScanStep::Transport,
        "a reply before anything was sent"
    );
}

#[test]
fn a_reply_from_before_a_re_pin_is_ignored() {
    let mut chain = chain();
    chain.block_misses = 1;
    let mut read = read_of(vec![strk_pair()]);
    pinned(&mut read);
    let calls = read.next_calls();
    // The first call misses the block: the read re-pins.
    assert_eq!(read.accept(chain.reply(&calls[0])), ScanStep::Continue);
    // The second call's answer arrives after the re-pin: stale, ignored.
    assert_eq!(read.accept(chain.reply(&calls[1])), ScanStep::Continue);
    let pin = read.next_calls();
    assert_eq!(pin.len(), 1, "a fresh pin");
    assert_eq!(read.accept(chain.reply(&pin[0])), ScanStep::Continue);
    assert!(run(read, &mut chain).is_some());
}

#[test]
fn a_contract_that_is_not_a_felt_reads_nothing() {
    let network = super::StakingNetwork {
        staking_contract: "0xZZ",
        ..NETWORK
    };
    assert!(StakingRead::new(&network, &strk_info(), f(ACCOUNT), vec![]).is_none());
}

#[test]
fn a_new_attempt_keeps_no_facts_from_the_last() {
    let mut chain = chain();
    let mut read = read_of(vec![strk_pair()]);
    pinned(&mut read);
    for call in read.next_calls() {
        assert_eq!(read.accept(chain.reply(&call)), ScanStep::Continue);
    }
    // The first endpoint read the facts, then fails while the stakers are out.
    let calls = read.next_calls();
    let failed = RpcReply {
        id: calls[0].id,
        status: 0,
        body: None,
    };
    assert_eq!(read.accept(failed), ScanStep::Transport);
    read.restart();
    // The next endpoint's staking contract pays rewards in another token: no facts.
    chain.reward_token = Felt::from(0x777u32);
    let reading = run(read, &mut chain).unwrap();
    assert_eq!(reading.facts, None);
    assert!(reading.pools.is_empty());
}
