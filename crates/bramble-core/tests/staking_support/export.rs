//! A canned Mainnet staking chain in the deployed contracts' layout (wallet-core's
//! `staking::test_chain`, behind the real staking contract), a wallet with its Mainnet account,
//! and the calldata an account signs. Nothing here touches a network.

use std::sync::Arc;

use bramble_core::{
    HttpOutcome, NetworkId, RpcReply, RpcRequest, ScanStep, SendStep, StakingAction, StakingError,
    StakingPlan, StakingPreparation, StepReply, TokenPreferences, TransferPlan, WalletCore,
};
use serde_json::{json, Value};
use starknet_wallet_core::staking::read::{
    CONTRACT_PARAMETERS_V1, GET_ACTIVE_TOKENS, GET_POOL_MEMBER_INFO_V1, GET_STAKER_INFO_V1,
    STAKER_POOL_INFO,
};

use crate::send_support;
pub use send_support::*;

/// `STAKING_POLICY`'s Mainnet staking contract.
pub const STAKING: &str = "0xca1702e64c81d9a07b86bd2c540188d92a2c73cf5cc0e508d949015e7e84a7";
pub const STAKER: &str = "0x5a200000000000000000000000000000000000000000000000000000000beef";
pub const POOL: &str = "0x5a3";
/// The parity context's clock, in Unix seconds (`send_support::NOW` is milliseconds).
pub const NOW_S: u64 = super::NOW;
pub const E18: u128 = 1_000_000_000_000_000_000;
pub const BALANCE: &str = "100000000000000000000";

pub fn hex(value: u128) -> String {
    format!("{value:#x}")
}

#[derive(Clone, Copy)]
pub struct Member {
    pub amount: u128,
    pub unclaimed: u128,
    pub unpool: Option<(u128, u64)>,
}

/// The staking contract, `STAKER` with a STRK pool at `POOL`, and the account's membership.
pub struct Chain {
    pub account: String,
    pub member: Option<Member>,
    /// The staker the pool's own parameters name (`STAKER` unless a test forges it).
    pub pool_staker: &'static str,
}

impl Chain {
    pub fn new(account: &str) -> Self {
        Self {
            account: canon(account),
            member: None,
            pool_staker: STAKER,
        }
    }

    pub fn answer(&self, contract: &str, selector: &str, calldata: &[Value]) -> Vec<String> {
        let felts = |items: &[&str]| items.iter().map(|s| s.to_string()).collect();
        let commission = "0x3e8";
        if contract == STAKING {
            let staker = canon(calldata.first().and_then(Value::as_str).unwrap_or("0x0"));
            let ours = staker == STAKER;
            return match selector {
                GET_ACTIVE_TOKENS => felts(&["0x1", STRK]),
                CONTRACT_PARAMETERS_V1 => felts(&["0x1", STRK, "0x7", "0x8", "0x9", "0x93a80"]),
                STAKER_POOL_INFO if ours => {
                    felts(&["0x0", commission, "0x1", POOL, STRK, "0x3635c9adc5dea00000"])
                }
                STAKER_POOL_INFO => felts(&["0x1", "0x0"]),
                GET_STAKER_INFO_V1 if ours => {
                    felts(&["0x0", STAKER, STAKER, "0x1", "0x0", "0x0", "0x1"])
                }
                GET_STAKER_INFO_V1 => felts(&["0x1"]),
                other => panic!("unexpected staking selector {other}"),
            };
        }
        assert_eq!(contract, POOL, "only the verified pool is read");
        match (selector, self.member) {
            (CONTRACT_PARAMETERS_V1, _) => {
                felts(&[self.pool_staker, "0x0", STAKING, STRK, commission])
            }
            (GET_POOL_MEMBER_INFO_V1, None) => felts(&["0x1"]),
            (GET_POOL_MEMBER_INFO_V1, Some(m)) => {
                let mut out = vec!["0x0".into(), self.account.clone(), hex(m.amount)];
                let (amount, time) = m.unpool.unwrap_or_default();
                out.extend([hex(m.unclaimed), commission.into(), hex(amount)]);
                match m.unpool {
                    None => out.push("0x1".into()),
                    Some(_) => out.extend(["0x0".into(), hex(u128::from(time))]),
                }
                out
            }
            (other, _) => panic!("unexpected pool selector {other}"),
        }
    }

    pub fn reply(&self, request: &RpcRequest) -> RpcReply {
        let body: Value = serde_json::from_str(&request.body).unwrap();
        let result = match body["method"].as_str().unwrap() {
            "starknet_blockHashAndNumber" => json!({"block_hash": "0xb10c", "block_number": 7}),
            "starknet_call" => {
                assert_eq!(body["params"]["block_id"], json!({"block_hash": "0xb10c"}));
                let call = &body["params"]["request"];
                let contract = canon(call["contract_address"].as_str().unwrap());
                let selector = call["entry_point_selector"].as_str().unwrap();
                json!(self.answer(&contract, selector, call["calldata"].as_array().unwrap()))
            }
            other => panic!("unexpected method {other}"),
        };
        let reply = json!({"jsonrpc": "2.0", "id": request.id, "result": result});
        RpcReply {
            id: request.id,
            status: 200,
            body: Some(reply.to_string()),
        }
    }
}

/// Answers every call `next` hands out until it hands out none.
pub fn run(
    chain: &Chain,
    next: impl Fn() -> Vec<RpcRequest>,
    accept: impl Fn(RpcReply) -> ScanStep,
) {
    loop {
        let calls = next();
        if calls.is_empty() {
            return;
        }
        for call in &calls {
            assert_eq!(accept(chain.reply(call)), ScanStep::Continue);
        }
    }
}

pub struct Wallet {
    pub dir: tempfile::TempDir,
    pub core: Arc<WalletCore>,
    pub id: String,
    pub address: String,
    pub prefs: Arc<TokenPreferences>,
}

pub fn mainnet() -> Wallet {
    let (dir, core) = wallet();
    let id = oz_id(&core, NetworkId::Mainnet, 0);
    let address = canon(id.split(':').nth(1).unwrap());
    Wallet {
        dir,
        core,
        id,
        address,
        prefs: TokenPreferences::new(None),
    }
}

impl Wallet {
    pub fn strk_id(&self) -> String {
        let tracked = self.prefs.tracked(NetworkId::Mainnet);
        let strk = tracked.iter().find(|t| t.token.symbol == "STRK").unwrap();
        strk.token.id.clone()
    }

    pub fn prepare(
        &self,
        action: StakingAction,
        amount: Option<&str>,
    ) -> Result<Arc<StakingPreparation>, StakingError> {
        let amount = amount.map(String::from);
        let (id, staker, prefs) = (self.id.clone(), STAKER.into(), self.prefs.clone());
        self.core
            .clone()
            .prepare_staking(id, action, staker, self.strk_id(), amount, prefs)
    }

    /// `action` prepared, read from `chain` and planned.
    pub fn plan(
        &self,
        chain: &Chain,
        action: StakingAction,
        amount: Option<&str>,
    ) -> Result<StakingPlan, StakingError> {
        let prepared = self.prepare(action, amount)?;
        run(chain, || prepared.next_calls(), |r| prepared.accept(r));
        let check = || prepared.check_next_calls();
        run(chain, check, |r| prepared.check_accept(r));
        prepared.plan(NOW_S, Some(BALANCE.into()))
    }

    /// Runs `step` against a Mainnet node until a non-RPC step, the signed write echoing the
    /// row's hash.
    pub fn drive(&self, plan: &TransferPlan, mut step: SendStep) -> SendStep {
        let mut node = Node::new(NetworkId::Mainnet, true);
        loop {
            match &step {
                SendStep::Rpc { .. } => {
                    let (id, method, params, is_write) = rpc_parts(&step);
                    let signed = is_write.then(|| latest_row_hash(&self.core, &self.id));
                    let answer = node.reply(&method, &params, signed.as_deref());
                    step = plan.advance(rpc_reply(id, &answer)).unwrap();
                }
                SendStep::Wait { .. } => step = plan.advance(StepReply::WaitElapsed).unwrap(),
                _ => return step,
            }
        }
    }

    pub fn allowance_log(&self) -> Vec<Value> {
        let text = std::fs::read_to_string(self.dir.path().join("allowances.v1.json"))
            .unwrap_or_else(|_| "[]".into());
        serde_json::from_str::<Value>(&text)
            .unwrap()
            .as_array()
            .unwrap()
            .clone()
    }
}

/// The hash of the newest row, which the plan files before its write leaves.
pub fn latest_row_hash(core: &WalletCore, id: &str) -> String {
    core.activity(id.into()).unwrap()[0].hash.clone()
}

/// The `__execute__` calldata `plan` writes to the node, signed, after one confirm. The fee was
/// estimated on the same calldata, and the write is the batch's only one.
pub fn signed_calldata(wallet: &Wallet, plan: &TransferPlan) -> Vec<String> {
    let (mut estimated, mut signed) = (None, None);
    let mut node = Node::new(NetworkId::Mainnet, true);
    let mut step = plan.advance(StepReply::Start).unwrap();
    loop {
        match &step {
            SendStep::Rpc { .. } => {
                let (id, method, params, is_write) = rpc_parts(&step);
                if method == "starknet_estimateFee" {
                    estimated = Some(felts(&params["request"][0]["calldata"]));
                }
                let hash = is_write.then(|| {
                    assert_eq!(signed, None, "one write");
                    signed = Some(felts(&params["invoke_transaction"]["calldata"]));
                    latest_row_hash(&wallet.core, &wallet.id)
                });
                let answer = node.reply(&method, &params, hash.as_deref());
                step = plan.advance(rpc_reply(id, &answer)).unwrap();
            }
            SendStep::Wait { .. } => step = plan.advance(StepReply::WaitElapsed).unwrap(),
            SendStep::Review => step = plan.advance(StepReply::Confirm).unwrap(),
            _ => break,
        }
    }
    assert!(matches!(step, SendStep::Broadcast { .. }), "{step:?}");
    let signed = signed.expect("a signed write");
    assert_eq!(
        estimated.as_ref(),
        Some(&signed),
        "estimated what it signed"
    );
    signed
}

pub fn refused(error: StakingError) -> String {
    match error {
        StakingError::Refused { reason } => reason,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

pub fn http(body: &Value) -> HttpOutcome {
    HttpOutcome {
        status: 200,
        retry_after: None,
        body: Some(body.to_string().into_bytes()),
        timed_out: false,
    }
}

pub fn coverage() -> Value {
    json!({"activityHistoryTruncated": false, "finalizedOnly": true, "gapIntervals": [],
        "lagBlocks": 0, "lastSuccessfulRunAtIso": "2026-11-06T00:00:00Z",
        "materializedThroughBlock": 100, "outstandingMaterializationIntervals": [],
        "reasonCode": "ok", "sourceLatestFinalizedBlock": 100, "status": "prepared"})
}

pub fn positions(address: &str, validators: &[&str]) -> Value {
    let positions: Vec<Value> = validators
        .iter()
        .map(|validator| {
            json!({"address": address, "delegatedRaw": "5000000000000000000",
                "pendingExitRaw": null, "poolAddress": POOL, "status": "active",
                "tokenAddress": STRK, "validatorAddress": validator})
        })
        .collect();
    json!({"address": address, "chainId": "SN_MAIN", "coverage": coverage(), "history": [],
        "nextHistoryCursor": null, "positions": positions, "positionsTruncated": false,
        "source": "finalized_prepared_staking_snapshot"})
}
