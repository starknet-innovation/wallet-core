//! The extension's swap test harness (`tests/support/swap-policy-harness.ts`,
//! `testkit/src/avnu-route.ts`) in one place: the reviewed swap (0.01 ETH for 12.5 STRK at 0.5%
//! slippage on mainnet) every hostile case is measured against.

use super::{AvnuDeployment, PendingQuote, SwapPolicy, SwapQuote};
use crate::catalog::TokenInfo;
use crate::portfolio::wire::HttpOutcome;
use crate::send::call_decode::Known;
use crate::send::calls::Call;
use crate::send::fees::Headroom;
use crate::send::multisig_invoke::StarkPrimitives;
use crate::send::types::{Account, Authorization, Policy};
use crate::send::u256::U256;
use crate::{Felt, NetworkId};

pub const ETH: &str = "0x049d36570d4e46f48e99674bd3fcc84644ddd6b96f7c741b1562b82f9e004dc7";
pub const STRK: &str = "0x04718f5a0fc34cc1af16a1cdee98ffb20c31f5cd61d6ab07201858f4287c938d";
pub const ACCOUNT: &str = "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcde";
/// AVNU's published mainnet exchange, the extension's `AVNU_DEPLOYMENTS.SN_MAIN.router`.
pub const ROUTER: &str = "0x04270219d365d6b017231b52e92b3fb5d7c8378b05e9abc97724537a80e93b0f";
pub const ATTACKER: &str = "0x6666666666666666666666666666666666666666666666666666666666666";
pub const AGGREGATOR_CHOSEN: &str =
    "0x7777777777777777777777777777777777777777777777777777777777777";
pub const QUOTE_ID: &str = "dae4e180-e2cb-49f4-bd91-6f3de3a0c3ca";
pub const SELL_AMOUNT: u128 = 10_000_000_000_000_000;
pub const BUY_AMOUNT: u128 = 12_500_000_000_000_000_000;
/// What an honest 0.5%-slippage build carries under that quote.
pub const MIN_BUY_AMOUNT: u128 = 12_437_500_000_000_000_000;

pub const DEPLOYMENTS: [AvnuDeployment; 1] = [AvnuDeployment {
    network: NetworkId::Mainnet,
    base_url: "https://starknet.api.avnu.fi",
    router: Felt::from_hex_unchecked(ROUTER),
}];

/// The extension's numbers, as `bramble-core`'s `SWAP_POLICY` sets them; mainnet only here, so
/// Sepolia stands in for a network with no aggregator.
pub const POLICY: SwapPolicy = SwapPolicy {
    deployments: &DEPLOYMENTS,
    default_slippage: 0.005,
    max_slippage: 0.05,
    slippage_rounding_tolerance_bps: 1,
    max_integrator_fee_bps: 100,
    max_build_calls: 4,
    max_call_calldata: 2_000,
    max_quote_id_length: 120,
};

/// Bramble's `SEND_POLICY` batch numbers.
pub const SEND_POLICY: Policy = Policy {
    fee_margin_percent: 50,
    reviewed_fee_headroom: Headroom {
        numerator: 3,
        denominator: 2,
    },
    confirmation_delays_ms: &[0],
    max_invoke_calls: 16,
    max_call_calldata_felts: 4_096,
};

/// Test-only: the selector is sha3's Keccak (a dev-dependency); nothing here verifies.
pub struct Keccak;

impl StarkPrimitives for Keccak {
    fn selector(&self, entrypoint: &str) -> Option<Felt> {
        use sha3::{Digest, Keccak256};
        let mut digest: [u8; 32] = Keccak256::digest(entrypoint.as_bytes()).into();
        digest[0] &= 0x03;
        Some(Felt::from_bytes_be(&digest))
    }

    fn verify(&self, _: &Felt, _: &Felt, _: &Felt, _: &Felt) -> bool {
        false
    }
}

pub fn felt(value: &str) -> Felt {
    Felt::from_hex(value).unwrap()
}

pub fn hex(value: u128) -> String {
    format!("{value:#x}")
}

pub fn account() -> Account {
    Account {
        network: NetworkId::Mainnet,
        address: felt(ACCOUNT),
        public_key: Felt::from(3u8),
        authorization: Authorization::StarkPair,
        deployment: None,
    }
}

pub fn token(address: &str, symbol: &str) -> TokenInfo {
    TokenInfo {
        address: address.into(),
        decimals: 18,
        name: symbol.into(),
        symbol: symbol.into(),
    }
}

pub fn tokens() -> [TokenInfo; 2] {
    [token(ETH, "ETH"), token(STRK, "STRK")]
}

pub fn known(tokens: &[TokenInfo]) -> Known<'_> {
    Known {
        tokens,
        nft_contracts: &[],
    }
}

pub fn outcome(status: u16, body: &str) -> HttpOutcome {
    HttpOutcome {
        status,
        retry_after: None,
        body: Some(body.as_bytes().to_vec()),
        timed_out: false,
    }
}

pub fn quote_json() -> serde_json::Value {
    serde_json::json!({
        "buyAmount": hex(BUY_AMOUNT),
        "buyAmountInUsd": 18.8,
        "buyTokenAddress": STRK,
        "gasFeesInUsd": 0.025,
        "quoteId": QUOTE_ID,
        "sellAmount": hex(SELL_AMOUNT),
        "sellAmountInUsd": 18.79,
        "sellTokenAddress": ETH,
    })
}

/// The pending request for the reviewed swap.
pub fn pending_quote() -> PendingQuote {
    let [eth, strk] = tokens();
    PendingQuote::new(&account(), &eth, &strk, "0.01", &POLICY)
        .unwrap()
        .0
}

/// The reviewed quote, judged from `reply`.
pub fn quote_from(reply: serde_json::Value) -> SwapQuote {
    pending_quote()
        .judge(&outcome(200, &serde_json::json!([reply]).to_string()))
        .unwrap()
}

pub fn quote() -> SwapQuote {
    quote_from(quote_json())
}

pub fn call(contract: &str, entrypoint: &str, calldata: &[&str]) -> Call {
    Call {
        contract_address: felt(contract),
        entrypoint: entrypoint.into(),
        calldata: calldata.iter().map(|value| felt(value)).collect(),
    }
}

/// The exact approval the reviewed swap admits.
pub fn approve_router() -> Call {
    call(ETH, "approve", &[ROUTER, &hex(SELL_AMOUNT), "0x0"])
}

/// `AvnuRouteFixture`: every field of the route, so a case changes exactly one.
#[derive(Clone)]
pub struct Route {
    pub sell_token: Felt,
    pub sell_amount: U256,
    pub buy_token: Felt,
    pub buy_amount: U256,
    pub min_buy_amount: U256,
    pub beneficiary: Felt,
    pub fee_bps: Felt,
    pub fee_recipient: Felt,
    pub routes: Vec<Vec<Felt>>,
}

/// `avnuDirectRoute` with its default exchange, percent and parameters.
pub fn direct_route(params: &[Felt]) -> Vec<Felt> {
    let mut route = vec![
        felt(ETH),
        felt(STRK),
        felt("0x1114c7103e12c2b2ecbd3a2472ba9c48ddcbf702b1c242dd570057e26212111"),
        felt("0x2540be400"),
        Felt::from(params.len()),
    ];
    route.extend_from_slice(params);
    route
}

impl Route {
    pub fn honest() -> Self {
        Self {
            sell_token: felt(ETH),
            sell_amount: U256::from_u128(SELL_AMOUNT),
            buy_token: felt(STRK),
            buy_amount: U256::from_u128(BUY_AMOUNT),
            min_buy_amount: U256::from_u128(MIN_BUY_AMOUNT),
            beneficiary: felt(ACCOUNT),
            fee_bps: Felt::ZERO,
            fee_recipient: Felt::ZERO,
            routes: vec![direct_route(&[Felt::from(0x1f4u16), Felt::ZERO])],
        }
    }

    /// `avnuRouteCalldata`.
    pub fn calldata(&self) -> Vec<Felt> {
        let limbs = |v: U256| [Felt::from(v.low), Felt::from(v.high)];
        let mut out = vec![self.sell_token];
        out.extend(limbs(self.sell_amount));
        out.push(self.buy_token);
        out.extend(limbs(self.buy_amount));
        out.extend(limbs(self.min_buy_amount));
        out.extend([
            self.beneficiary,
            self.fee_bps,
            self.fee_recipient,
            Felt::from(self.routes.len()),
        ]);
        out.extend(self.routes.iter().flatten());
        out
    }

    pub fn call(&self) -> Call {
        Call {
            contract_address: felt(ROUTER),
            entrypoint: "multi_route_swap".into(),
            calldata: self.calldata(),
        }
    }
}

pub fn route_call() -> Call {
    Route::honest().call()
}

/// `{ calls, chainId }` as AVNU's build replies, over `calls`.
pub fn build_reply(calls: &[Call]) -> String {
    let calls: Vec<serde_json::Value> = calls
        .iter()
        .map(|call| {
            serde_json::json!({
                "contractAddress": crate::felt::canonical(&call.contract_address),
                "entrypoint": call.entrypoint,
                "calldata": call.calldata.iter().map(crate::felt::canonical).collect::<Vec<_>>(),
            })
        })
        .collect();
    serde_json::json!({
        "calls": calls,
        "chainId": crate::felt::canonical(&NetworkId::Mainnet.chain_id()),
    })
    .to_string()
}
