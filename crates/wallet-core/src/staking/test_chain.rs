//! Test support: the extension's deterministic staking chain (`tests/support/staking-chain.ts`)
//! as a scripted node behind `StakingRead`. One staking contract, one validator with a STRK pool,
//! more validators on request, and a mutable membership for the account under test. Answers use
//! the deployed contracts' serialisation (Cairo `Option`: Some = 0, None = 1), so the decoders
//! run unmodified.

use serde_json::{json, Value};

use super::read::{
    StakingPair, StakingRead, StakingReading, CONTRACT_PARAMETERS_V1, GET_ACTIVE_TOKENS,
    GET_POOL_MEMBER_INFO_V1, GET_STAKER_INFO_V1, STAKER_POOL_INFO,
};
use super::{StakingNetwork, StakingPolicy};
use crate::catalog::TokenInfo;
use crate::history::events::ScanStep;
use crate::portfolio::model::TokenConfig;
use crate::portfolio::wire::{RpcReply, RpcRequest};
use crate::{felt, Felt, NetworkId};

pub(crate) const STAKING_CONTRACT: &str = "0x5a1";
pub(crate) const STAKER: &str = "0x5a200000000000000000000000000000000000000000000000000000000beef";
pub(crate) const STAKING_POOL: &str = "0x5a3";
pub(crate) const STRK: &str = "0x04718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d";
/// `verify.test.ts`'s account, as `invoke_tests` uses it.
pub(crate) const ACCOUNT: &str =
    "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcde";
pub(crate) const BLOCK: &str = "0xb10c";
pub(crate) const NOW: u64 = 1_800_000_000;

/// Mainnet switched on against the test staking contract (`testStakingNetworks`).
pub(crate) const NETWORK: StakingNetwork = StakingNetwork {
    network: NetworkId::Mainnet,
    enabled: true,
    staking_contract: STAKING_CONTRACT,
    starkscan: true,
};

/// The extension's numbers.
pub(crate) const POLICY: StakingPolicy = StakingPolicy {
    networks: &[NETWORK],
    directory_page_size: 50,
    max_checked_positions: 16,
    max_catching_up_lag_blocks: 100,
    max_preparations: 20,
};

pub(crate) fn f(value: &str) -> Felt {
    felt::parse(value).unwrap()
}

pub(crate) fn strk_info() -> TokenInfo {
    TokenInfo {
        address: STRK.into(),
        decimals: 18,
        name: "Starknet Token".into(),
        symbol: "STRK".into(),
    }
}

pub(crate) fn strk_config() -> TokenConfig {
    TokenConfig {
        id: "SN_MAIN:strk".into(),
        network: NetworkId::Mainnet,
        address: STRK.into(),
        symbol: "STRK".into(),
        name: "Starknet Token".into(),
        decimals: 18,
    }
}

/// The account's membership (`StakingChainState["member"]`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Member {
    pub amount: u128,
    pub reward_address: Felt,
    pub unclaimed: u128,
    pub unpool_amount: u128,
    pub unpool_time: Option<u64>,
}

/// `member()`: 5 STRK staked, rewards to the account, nothing pending.
pub(crate) fn member() -> Member {
    Member {
        amount: 5 * 10u128.pow(18),
        reward_address: f(ACCOUNT),
        unclaimed: 0,
        unpool_amount: 0,
        unpool_time: None,
    }
}

/// Mutable chain state; tests change it between reads.
#[derive(Debug, Clone)]
pub(crate) struct Chain {
    pub active_tokens: Vec<Felt>,
    pub commission: u16,
    pub member: Option<Member>,
    pub pool_staker: Felt,
    pub reward_token: Felt,
    pub staker_leaving: bool,
    pub extra_stakers: Vec<Felt>,
    /// More `(token, pool)` pools `STAKER` lists after its STRK pool (no contract answers there).
    pub extra_pools: Vec<(Felt, Felt)>,
    /// Entrypoints read, in order (selector names).
    pub reads: Vec<&'static str>,
    /// Pinned calls still to answer "Block not found" (JSON-RPC 24).
    pub block_misses: u32,
    /// Every `block_id` a call was pinned to.
    pub pins: Vec<Value>,
}

/// `makeStakingChain`: a verified STRK pool and no membership.
pub(crate) fn chain() -> Chain {
    Chain {
        active_tokens: vec![f(STRK)],
        commission: 1000,
        member: None,
        pool_staker: f(STAKER),
        reward_token: f(STRK),
        staker_leaving: false,
        extra_stakers: Vec::new(),
        extra_pools: Vec::new(),
        reads: Vec::new(),
        block_misses: 0,
        pins: Vec::new(),
    }
}

fn hex(value: u128) -> String {
    format!("{value:#x}")
}

fn canonical(value: &Felt) -> String {
    felt::canonical(value)
}

impl Chain {
    fn pool_for(&self, staker: &Felt) -> Option<Felt> {
        if *staker == f(STAKER) {
            return Some(f(STAKING_POOL));
        }
        let index = self.extra_stakers.iter().position(|s| s == staker)?;
        Some(Felt::from(0x5b00 + index as u64))
    }

    fn staker_of_pool(&self, pool: &Felt) -> Option<Felt> {
        if *pool == f(STAKING_POOL) {
            return Some(self.pool_staker);
        }
        self.extra_stakers
            .iter()
            .find(|staker| self.pool_for(staker) == Some(*pool))
            .copied()
    }

    fn answer(&self, contract: &Felt, selector: &str, calldata: &[Felt]) -> Option<Vec<String>> {
        let commission = hex(u128::from(self.commission));
        if *contract == f(STAKING_CONTRACT) {
            let staker = calldata.first().copied().unwrap_or(Felt::ZERO);
            return Some(match selector {
                GET_ACTIVE_TOKENS => std::iter::once(hex(self.active_tokens.len() as u128))
                    .chain(self.active_tokens.iter().map(canonical))
                    .collect(),
                CONTRACT_PARAMETERS_V1 => vec![
                    "0x1".into(),
                    canonical(&self.reward_token),
                    "0x7".into(),
                    "0x8".into(),
                    "0x9".into(),
                    hex(604_800),
                ],
                STAKER_POOL_INFO => match self.pool_for(&staker) {
                    Some(pool) => {
                        let extra = if staker == f(STAKER) {
                            self.extra_pools.as_slice()
                        } else {
                            &[]
                        };
                        let mut out = vec![
                            "0x0".into(),
                            commission,
                            hex(1 + extra.len() as u128),
                            canonical(&pool),
                            STRK.into(),
                            hex(10u128.pow(21)),
                        ];
                        for (token, pool) in extra {
                            out.extend([canonical(pool), canonical(token), "0x1".into()]);
                        }
                        out
                    }
                    None => vec!["0x1".into(), "0x0".into()],
                },
                GET_STAKER_INFO_V1 => {
                    let s = canonical(&staker);
                    let s = s.as_str();
                    match (self.pool_for(&staker), self.staker_leaving) {
                        (None, _) => vec!["0x1".into()],
                        (Some(_), true) => ["0x0", s, s, "0x0", "0x77359400", "0x0", "0x0", "0x1"]
                            .map(String::from)
                            .to_vec(),
                        (Some(_), false) => ["0x0", s, s, "0x1", "0x0", "0x0", "0x1"]
                            .map(String::from)
                            .to_vec(),
                    }
                }
                _ => return None,
            });
        }
        let pool_staker = self.staker_of_pool(contract)?;
        Some(match selector {
            CONTRACT_PARAMETERS_V1 => vec![
                canonical(&pool_staker),
                "0x0".into(),
                STAKING_CONTRACT.into(),
                STRK.into(),
                commission,
            ],
            GET_POOL_MEMBER_INFO_V1 => match self.member {
                None => vec!["0x1".into()],
                Some(m) => {
                    let mut out = vec![
                        "0x0".into(),
                        canonical(&m.reward_address),
                        hex(m.amount),
                        hex(m.unclaimed),
                        commission,
                        hex(m.unpool_amount),
                    ];
                    match m.unpool_time {
                        None => out.push("0x1".into()),
                        Some(time) => out.extend(["0x0".into(), hex(u128::from(time))]),
                    }
                    out
                }
            },
            _ => return None,
        })
    }

    /// The node's reply to one request.
    pub(crate) fn reply(&mut self, request: &RpcRequest) -> RpcReply {
        let body: Value = serde_json::from_str(&request.body).unwrap();
        let ok = |result: Value| json!({ "jsonrpc": "2.0", "id": request.id, "result": result });
        let error = |code: i64| json!({ "jsonrpc": "2.0", "id": request.id, "error": { "code": code, "message": "x" } });
        let answer = match body["method"].as_str().unwrap() {
            "starknet_blockHashAndNumber" => ok(json!({ "block_hash": BLOCK, "block_number": 7 })),
            "starknet_call" => {
                self.pins.push(body["params"]["block_id"].clone());
                let call = &body["params"]["request"];
                let selector = call["entry_point_selector"].as_str().unwrap();
                self.reads.push(name(selector));
                let contract = f(call["contract_address"].as_str().unwrap());
                let calldata: Vec<Felt> = call["calldata"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| f(v.as_str().unwrap()))
                    .collect();
                if self.block_misses > 0 {
                    self.block_misses -= 1;
                    error(24)
                } else {
                    match self.answer(&contract, selector, &calldata) {
                        Some(felts) => ok(json!(felts)),
                        // A node answering a call no contract takes: a contract error.
                        None => error(40),
                    }
                }
            }
            other => panic!("unexpected method {other}"),
        };
        RpcReply {
            id: request.id,
            status: 200,
            body: Some(answer.to_string()),
        }
    }
}

fn name(selector: &str) -> &'static str {
    match selector {
        GET_ACTIVE_TOKENS => "get_active_tokens",
        CONTRACT_PARAMETERS_V1 => "contract_parameters_v1",
        STAKER_POOL_INFO => "staker_pool_info",
        GET_STAKER_INFO_V1 => "get_staker_info_v1",
        GET_POOL_MEMBER_INFO_V1 => "get_pool_member_info_v1",
        _ => "unknown",
    }
}

/// Runs `read` against `chain` to the end (every stage's calls answered in order).
pub(crate) fn run(mut read: StakingRead, chain: &mut Chain) -> Option<StakingReading> {
    loop {
        let calls = read.next_calls();
        if calls.is_empty() {
            return read.finish();
        }
        for call in &calls {
            assert_eq!(read.accept(chain.reply(call)), ScanStep::Continue);
        }
    }
}

/// A read of `pairs` for the test account.
pub(crate) fn read_of(pairs: Vec<StakingPair>) -> StakingRead {
    StakingRead::new(&NETWORK, &strk_info(), f(ACCOUNT), pairs).unwrap()
}

/// The one pair the extension's tests read: `STAKER` and STRK.
pub(crate) fn strk_pair() -> StakingPair {
    StakingPair {
        staker: f(STAKER),
        token: f(STRK),
    }
}
