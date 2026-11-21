//! The chain reads behind every staking screen and action (`staking-reads.ts`, run through
//! `readAtPinnedBlock`): the staking contract's facts (its active tokens, its exit window, and
//! that it pays rewards in STRK), then, for each validator and token asked about, the staker's
//! pools and status and, where it lists a pool for the token, that pool's own parameters and the
//! account's membership. Every read of one `StakingRead` is pinned to one accepted block, so a
//! pool and a membership always describe one state.
//!
//! Sans-IO, under `HistoryScan`'s contract: `next_calls` hands out a stage's bodies at once (the
//! extension's `Promise.all`), `accept` judges each reply, `restart` replays on the next endpoint
//! after `ScanStep::Transport`, and `finish` yields the reading. A node that does not know the
//! pinned block (`isBlockNotFoundError`) is re-pinned, at most three pins; any other JSON-RPC
//! error, a malformed block, or a result that is not an array fails the read (the extension's
//! thrown read, which every caller shows as unreadable). Decoding is `decode`'s; a reply it
//! refuses becomes a refusal for that validator, never a guessed pool.

use serde_json::json;

use super::decode::{
    decode_active_tokens, decode_exit_window, decode_pool_member, decode_pool_parameters,
    decode_staker_pool_info, decode_staker_status, verify_staking_pool, PoolMember,
    StakingPoolReads, VerifiedPool,
};
use super::{StakingNetwork, StakingRefusal};
use crate::catalog::TokenInfo;
use crate::history::events::ScanStep;
use crate::js::felt::{canonical_felt, same_felt};
use crate::js::json::JsValue;
use crate::js::num::is_safe_integer;
use crate::portfolio::wire::{classify, rpc_call, Reply, RpcReply, RpcRequest};
use crate::send::judge;
use crate::{felt, Felt, NetworkId};

/// `starknet_keccak` of each entrypoint read; Keccak-checked in the tests.
pub const GET_ACTIVE_TOKENS: &str =
    "0x3f3cf5e475189c2cec4d370647dc1c1e0ef821e68b827a66c21b3ceb538db4d";
pub const CONTRACT_PARAMETERS_V1: &str =
    "0x1bbe983762bed00ab1fb31234611c56f8561e3cdcc963d7b5eb3875517a7435";
pub const STAKER_POOL_INFO: &str =
    "0x27aca223b48bc87a94edf580fbe4ad386c81bb31f6810b3dcfa60bf65f8f7f8";
pub const GET_STAKER_INFO_V1: &str =
    "0x1ffac027da63178bc6dfbc131df9716162b4369e8e023533c792bbfa5ae2b22";
pub const GET_POOL_MEMBER_INFO_V1: &str =
    "0xa0ec7d248e7cac7ad635c76ae431955a3ded9d7d9da3f80ee03c5c2383d9bf";

/// `PINNED_READ_ATTEMPTS`.
const PIN_ATTEMPTS: u32 = 3;

/// One validator (staker) and token to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StakingPair {
    pub staker: Felt,
    pub token: Felt,
}

/// The staking contract's network-wide facts (`StakingChainFacts`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StakingFacts {
    pub active_tokens: Vec<Felt>,
    pub exit_window_seconds: u64,
}

/// What one pair's reads concluded (`StakingPoolReading`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PoolOutcome {
    /// The staking contract does not accept this token: nothing was read for it.
    Inactive,
    /// The pool could not be verified, or the membership could not be read.
    Refused(StakingRefusal),
    /// A verified pool and the account's membership (`None`: not a member).
    Verified {
        pool: VerifiedPool,
        member: Option<PoolMember>,
    },
}

/// One pair's reading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolReading {
    pub pair: StakingPair,
    pub outcome: PoolOutcome,
}

/// A finished read, bound to the account and network it was read for. `facts` is `None` when
/// the staking contract's answers were malformed, or it pays rewards in anything but `rewards`;
/// then nothing else was read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StakingReading {
    pub network: NetworkId,
    pub account: Felt,
    pub staking_contract: Felt,
    pub rewards: TokenInfo,
    pub facts: Option<StakingFacts>,
    pub pools: Vec<PoolReading>,
}

impl StakingReading {
    /// Whether this reading was made for `account` on `network`.
    pub fn is_for(&self, account: &Felt, network: NetworkId) -> bool {
        self.account == *account && self.network == network
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Pin,
    Facts,
    Stakers,
    Pools,
    Done,
}

/// One `starknet_call` of a stage.
#[derive(Debug, Clone)]
struct Ask {
    contract: Felt,
    selector: &'static str,
    calldata: Vec<Felt>,
}

/// The reads for one account and a set of pairs. A wallet that shares one across threads wraps it
/// in its own lock.
#[derive(Debug)]
pub struct StakingRead {
    reading: StakingReading,
    pairs: Vec<StakingPair>,
    stage: Stage,
    failed: bool,
    pins: u32,
    block: Felt,
    asks: Vec<Ask>,
    answers: Vec<Option<Vec<String>>>,
    sent: bool,
    first_id: u64,
    next_id: u64,
    // Per pair: whether its token is active, and its staker's pool info and status.
    active: Vec<bool>,
    stakers: Vec<(Felt, StakingPoolReads)>,
    // Per pair: the pool the staker lists for its token, while the pool stage is out.
    listed: Vec<Option<Felt>>,
}

fn pair_staker<'a>(
    stakers: &'a [(Felt, StakingPoolReads)],
    staker: &Felt,
) -> Option<&'a StakingPoolReads> {
    stakers
        .iter()
        .find(|(known, _)| known == staker)
        .map(|(_, reads)| reads)
}

impl StakingRead {
    /// Reads `pairs` for `account` against `staking`'s contract, which must pay its rewards in
    /// `rewards` (STRK: the wallet labels and scales them so). `None` when the network record's
    /// contract is not a non-zero felt.
    pub fn new(
        staking: &StakingNetwork,
        rewards: &TokenInfo,
        account: Felt,
        pairs: Vec<StakingPair>,
    ) -> Option<Self> {
        let staking_contract = felt::parse_non_zero(staking.staking_contract)?;
        Some(Self {
            reading: StakingReading {
                network: staking.network,
                account,
                staking_contract,
                rewards: rewards.clone(),
                facts: None,
                pools: Vec::new(),
            },
            pairs,
            stage: Stage::Pin,
            failed: false,
            pins: 0,
            block: Felt::ZERO,
            asks: Vec::new(),
            answers: vec![None],
            sent: false,
            first_id: 1,
            next_id: 1,
            active: Vec::new(),
            stakers: Vec::new(),
            listed: Vec::new(),
        })
    }

    /// Discards all progress so the reads replay on another endpoint (a fresh pin there).
    pub fn restart(&mut self) {
        self.failed = false;
        self.pins = 0;
        self.enter(Stage::Pin, Vec::new());
    }

    fn enter(&mut self, stage: Stage, asks: Vec<Ask>) {
        self.stage = stage;
        self.answers = vec![None; asks.len().max(usize::from(stage == Stage::Pin))];
        self.asks = asks;
        self.sent = false;
        // Replies to anything sent before are now stale.
        self.first_id = self.next_id;
    }

    fn fail(&mut self) {
        self.failed = true;
        self.stage = Stage::Done;
    }

    /// The calls to send now; empty while waiting for replies or when done.
    pub fn next_calls(&mut self) -> Vec<RpcRequest> {
        if self.sent || self.stage == Stage::Done {
            return Vec::new();
        }
        self.sent = true;
        let block = felt::canonical(&self.block);
        let count = self.answers.len() as u64;
        self.next_id = self.first_id + count;
        if self.stage == Stage::Pin {
            return vec![rpc_call(
                self.first_id,
                "starknet_blockHashAndNumber",
                json!([]),
            )];
        }
        self.asks
            .iter()
            .zip(self.first_id..)
            .map(|(ask, id)| {
                let calldata: Vec<String> = ask.calldata.iter().map(felt::canonical).collect();
                rpc_call(
                    id,
                    "starknet_call",
                    json!({
                        "block_id": { "block_hash": block },
                        "request": {
                            "calldata": calldata,
                            "contract_address": felt::canonical(&ask.contract),
                            "entry_point_selector": ask.selector,
                        },
                    }),
                )
            })
            .collect()
    }

    /// Judges one reply. A reply to a call sent before a re-pin or restart is ignored; one to a
    /// call never sent, or answered twice, is a transport failure.
    pub fn accept(&mut self, reply: RpcReply) -> ScanStep {
        if self.stage == Stage::Done || reply.id < self.first_id {
            return ScanStep::Continue;
        }
        let index = (reply.id - self.first_id) as usize;
        if !self.sent || self.answers.get(index).is_none_or(Option::is_some) {
            return ScanStep::Transport;
        }
        match classify(&reply, reply.id) {
            Reply::Transport => return ScanStep::Transport,
            Reply::Semantic => {
                let stale_pin = reply
                    .body
                    .as_deref()
                    .and_then(|body| judge::answer(body, reply.id));
                if self.stage != Stage::Pin && judge::block_not_found(&stale_pin) {
                    self.pins += 1;
                    if self.pins >= PIN_ATTEMPTS {
                        self.fail();
                    } else {
                        self.enter(Stage::Pin, Vec::new());
                    }
                } else {
                    self.fail();
                }
            }
            Reply::Result(value) => self.answered(index, value),
        }
        ScanStep::Continue
    }

    fn answered(&mut self, index: usize, value: JsValue) {
        if self.stage == Stage::Pin {
            match accepted_block(&value) {
                Some(block) => {
                    self.block = block;
                    self.facts_stage();
                }
                None => self.fail(),
            }
            return;
        }
        // A result that is not an array is a thrown read; a non-string entry is a slot no
        // decoder accepts.
        let Some(items) = value.as_array() else {
            return self.fail();
        };
        let felts = items
            .iter()
            .map(|item| item.as_str().unwrap_or_default().to_string())
            .collect();
        self.answers[index] = Some(felts);
        if self.answers.iter().all(Option::is_some) {
            self.advance();
        }
    }

    fn facts_stage(&mut self) {
        let contract = self.reading.staking_contract;
        let ask = |selector| Ask {
            contract,
            selector,
            calldata: Vec::new(),
        };
        self.enter(
            Stage::Facts,
            vec![ask(GET_ACTIVE_TOKENS), ask(CONTRACT_PARAMETERS_V1)],
        );
    }

    fn answer(&self, index: usize) -> &[String] {
        self.answers
            .get(index)
            .and_then(Option::as_deref)
            .unwrap_or_default()
    }

    fn advance(&mut self) {
        match self.stage {
            Stage::Facts => self.judge_facts(),
            Stage::Stakers => self.judge_stakers(),
            Stage::Pools => self.judge_pools(),
            Stage::Pin | Stage::Done => {}
        }
    }

    // `readStakingChainFacts`: rewards are paid in the staking contract's own token, for BTC
    // pools too, and the wallet labels and scales them as STRK, so it confirms that first.
    fn judge_facts(&mut self) {
        let parameters = self.answer(1);
        let rewards_in_strk = parameters
            .get(1)
            .is_some_and(|token| same_felt(token, &self.reading.rewards.address));
        let facts = decode_active_tokens(self.answer(0))
            .zip(decode_exit_window(parameters))
            .filter(|_| rewards_in_strk)
            .map(|(active_tokens, exit_window_seconds)| StakingFacts {
                active_tokens,
                exit_window_seconds,
            });
        let Some(facts) = facts else {
            self.stage = Stage::Done;
            return;
        };
        self.active = self
            .pairs
            .iter()
            .map(|pair| facts.active_tokens.contains(&pair.token))
            .collect();
        self.reading.facts = Some(facts);
        let mut stakers: Vec<Felt> = Vec::new();
        for (pair, active) in self.pairs.iter().zip(&self.active) {
            if *active && !stakers.contains(&pair.staker) {
                stakers.push(pair.staker);
            }
        }
        let contract = self.reading.staking_contract;
        let asks = stakers
            .iter()
            .flat_map(|staker| {
                [STAKER_POOL_INFO, GET_STAKER_INFO_V1].map(|selector| Ask {
                    contract,
                    selector,
                    calldata: vec![*staker],
                })
            })
            .collect();
        self.stakers = stakers
            .into_iter()
            .map(|staker| (staker, StakingPoolReads::default()))
            .collect();
        self.enter(Stage::Stakers, asks);
        if self.asks.is_empty() {
            self.finalize(&[]);
        }
    }

    // The listed pool of each active pair, then its parameters and the account's membership.
    fn judge_stakers(&mut self) {
        for index in 0..self.stakers.len() {
            let pool_info = decode_staker_pool_info(self.answer(index * 2));
            let status = decode_staker_status(self.answer(index * 2 + 1));
            let reads = &mut self.stakers[index].1;
            reads.pool_info = pool_info;
            reads.status = status;
        }
        let account = self.reading.account;
        let mut listed: Vec<Option<Felt>> = Vec::new();
        let mut asks = Vec::new();
        for (pair, active) in self.pairs.iter().zip(&self.active) {
            let pool = pair_staker(&self.stakers, &pair.staker)
                .filter(|_| *active)
                .and_then(|reads| reads.pool_info.as_ref())
                .and_then(|info| info.pools.iter().find(|pool| pool.token == pair.token))
                .map(|pool| pool.pool);
            if let Some(pool) = pool {
                asks.push(Ask {
                    contract: pool,
                    selector: CONTRACT_PARAMETERS_V1,
                    calldata: Vec::new(),
                });
                asks.push(Ask {
                    contract: pool,
                    selector: GET_POOL_MEMBER_INFO_V1,
                    calldata: vec![account],
                });
            }
            listed.push(pool);
        }
        self.enter(Stage::Pools, asks);
        if self.asks.is_empty() {
            self.finalize(&listed);
        } else {
            self.listed = listed;
        }
    }

    fn judge_pools(&mut self) {
        let listed = std::mem::take(&mut self.listed);
        self.finalize(&listed);
    }

    // `readStakingPool` for every pair, in order, from the answers gathered.
    fn finalize(&mut self, listed: &[Option<Felt>]) {
        let mut answer = 0;
        let mut pools = Vec::with_capacity(self.pairs.len());
        for (index, pair) in self.pairs.iter().enumerate() {
            let outcome = if !self.active.get(index).copied().unwrap_or(false) {
                PoolOutcome::Inactive
            } else {
                let mut reads = pair_staker(&self.stakers, &pair.staker)
                    .cloned()
                    .unwrap_or_default();
                let mut member = None;
                if listed.get(index).copied().flatten().is_some() {
                    reads.parameters = decode_pool_parameters(self.answer(answer));
                    member = Some(decode_pool_member(self.answer(answer + 1)));
                    answer += 2;
                }
                let staking = &self.reading.staking_contract;
                match verify_staking_pool(&reads, &pair.staker, staking, &pair.token) {
                    Err(refusal) => PoolOutcome::Refused(refusal),
                    Ok(pool) => match member.flatten() {
                        None => PoolOutcome::Refused(StakingRefusal::MemberUnreadable),
                        Some(member) => PoolOutcome::Verified { pool, member },
                    },
                }
            };
            pools.push(PoolReading {
                pair: *pair,
                outcome,
            });
        }
        self.reading.pools = pools;
        self.stage = Stage::Done;
    }

    /// The reading, once every call has been answered; `None` while reads remain or when the
    /// read failed (the extension's "Couldn't read staking", or "Couldn't load your stakes").
    pub fn finish(&self) -> Option<StakingReading> {
        (self.stage == Stage::Done && !self.failed).then(|| self.reading.clone())
    }
}

/// `latestAcceptedBlock`: a non-zero block hash and a safe, non-negative block number.
fn accepted_block(value: &JsValue) -> Option<Felt> {
    value
        .get("block_number")?
        .as_f64()
        .filter(|number| is_safe_integer(*number) && *number >= 0.0)?;
    let hash = canonical_felt(value.get("block_hash")?.as_str()?)?;
    felt::parse_non_zero(&hash)
}
