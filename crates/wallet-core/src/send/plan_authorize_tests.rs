//! `requireLiveAuthorizableClass` through the send review's reads, against a scripted node:
//! `account-compatibility.test.ts` and `braavos-compatibility.test.ts`'s cases for a signature an
//! account contract verifies.

use serde_json::{json, Value};

use super::*;
use crate::send::fees::Headroom;
use crate::send::plan::Plan;
use crate::send::plan_probe::{
    selector, GET_DEFERRED_REMOVE_SIGNERS, GET_GUARDIAN, GET_OWNER, GET_OWNER_TYPE, GET_SIGNERS,
};
use crate::send::sign::Deployment;
use crate::send::types::{Account, ArgentClass, Policy};
use crate::NetworkId;

const POLICY: Policy = Policy {
    fee_margin_percent: 50,
    reviewed_fee_headroom: Headroom {
        numerator: 3,
        denominator: 2,
    },
    confirmation_delays_ms: &[0],
    max_invoke_calls: 16,
    max_call_calldata_felts: 4_096,
};

const SEPOLIA: &str = "0x534e5f5345504f4c4941";

fn felt(value: u64) -> Felt {
    Felt::from(value)
}

/// What the scripted node holds for the account.
struct Chain {
    chain_id: &'static str,
    /// `None`: contract not found.
    class: Option<Felt>,
    /// A view's result, by selector.
    views: Vec<(Felt, Vec<Felt>)>,
    timestamp: u64,
}

impl Chain {
    fn answer(&self, body: &str) -> Value {
        let request: Value = serde_json::from_str(body).unwrap();
        let hex = |felts: &[Felt]| felts.iter().map(|f| format!("{f:#x}")).collect::<Vec<_>>();
        match request["method"].as_str().unwrap() {
            "starknet_chainId" => json!({ "result": self.chain_id }),
            "starknet_blockHashAndNumber" => {
                json!({ "result": { "block_hash": "0xb10c", "block_number": 7 } })
            }
            "starknet_getClassHashAt" => match self.class {
                Some(class) => json!({ "result": format!("{class:#x}") }),
                None => json!({ "error": { "code": 20, "message": "Contract not found" } }),
            },
            "starknet_call" => {
                let wanted = request["params"]["request"]["entry_point_selector"]
                    .as_str()
                    .and_then(|s| Felt::from_hex(s).ok())
                    .unwrap();
                let result = self
                    .views
                    .iter()
                    .find(|(view, _)| *view == wanted)
                    .map(|(_, result)| hex(result))
                    .unwrap_or_default();
                json!({ "result": result })
            }
            "starknet_getBlockWithTxHashes" => json!({ "result": { "timestamp": self.timestamp } }),
            other => panic!("unexpected read {other}"),
        }
    }
}

fn run(account: Account, chain: &Chain) -> AuthorizeStep {
    let mut plan = Plan::new(account, Kind::Authorize, POLICY);
    let mut step = plan.authorize(StepReply::Start);
    while let AuthorizeStep::Read(SendStep::Rpc { id, body, .. }) = &step {
        let mut reply = chain.answer(body);
        reply["jsonrpc"] = json!("2.0");
        reply["id"] = json!(id);
        step = plan.authorize(StepReply::Rpc {
            id: *id,
            body: reply.to_string(),
            now_ms: 0,
        });
    }
    assert_eq!(plan.signatures_made(), 0);
    step
}

fn refused(step: AuthorizeStep) -> Code {
    match step {
        AuthorizeStep::Refused(failure) => failure.code,
        other => panic!("not refused: {other:?}"),
    }
}

const KEY: u64 = 0xbeef;
const ARGENT_04: u64 = 0x0400;
const ARGENT_03: u64 = 0x0300;

fn ready(deployment: Option<Deployment>) -> Account {
    let class = |hash, signer_array| ArgentClass {
        class_hash: felt(hash),
        label: format!("argent {hash:#x}"),
        signer_array,
    };
    Account {
        network: NetworkId::Sepolia,
        address: felt(0xacc),
        public_key: felt(KEY),
        authorization: Authorization::ArgentSignerArray {
            classes: vec![class(ARGENT_04, true), class(ARGENT_03, false)],
        },
        deployment,
    }
}

fn ready_chain(class: Option<u64>, owner: u64, guardian: u64) -> Chain {
    Chain {
        chain_id: SEPOLIA,
        class: class.map(felt),
        views: vec![
            (selector(GET_OWNER), vec![felt(owner)]),
            (selector(GET_OWNER_TYPE), vec![felt(0)]),
            (selector(GET_GUARDIAN), vec![felt(guardian)]),
        ],
        timestamp: 0,
    }
}

#[test]
fn a_ready_account_authorizes_by_its_live_class_not_its_stored_one() {
    // Stored at 0.3.x or 0.4, what counts is the class it runs now.
    assert_eq!(
        run(ready(None), &ready_chain(Some(ARGENT_04), KEY, 0)),
        AuthorizeStep::Authorized
    );
    assert_eq!(
        refused(run(ready(None), &ready_chain(Some(ARGENT_03), KEY, 0))),
        Code::AccountClassUnsupported
    );
    assert_eq!(
        refused(run(ready(None), &ready_chain(Some(0x999), KEY, 0))),
        Code::AccountClassUnrecognised
    );
    // Another owner, or a guardian, cannot verify this wallet's lone signature.
    assert_eq!(
        refused(run(ready(None), &ready_chain(Some(ARGENT_04), 0x1, 0))),
        Code::AccountOwnerMismatch
    );
    assert_eq!(
        refused(run(ready(None), &ready_chain(Some(ARGENT_04), KEY, 0x9))),
        Code::AccountGuardianUnsupported
    );
}

#[test]
fn an_undeployed_ready_account_authorizes_only_through_the_class_it_would_deploy() {
    let deployment = |class| Deployment {
        class_hash: felt(class),
        salt: Felt::ZERO,
        constructor_calldata: Vec::new(),
    };
    // `Plan::new` keeps a deployment only when it reproduces the address.
    let deployable = |class| {
        let deployment = deployment(class);
        Account {
            address: deployment.address().unwrap(),
            ..ready(Some(deployment))
        }
    };
    assert_eq!(
        run(deployable(ARGENT_04), &ready_chain(None, KEY, 0)),
        AuthorizeStep::Authorized
    );
    assert_eq!(
        refused(run(deployable(ARGENT_03), &ready_chain(None, KEY, 0))),
        Code::AccountClassUnsupported
    );
    assert_eq!(
        refused(run(ready(None), &ready_chain(None, KEY, 0))),
        Code::AccountClassUnsupported
    );
}

const BRAAVOS_IMPL: u64 = 0xb4a;

fn braavos() -> Account {
    Account {
        network: NetworkId::Sepolia,
        address: felt(0xacc),
        public_key: felt(KEY),
        authorization: Authorization::Braavos {
            signing_classes: vec![felt(BRAAVOS_IMPL)],
            implementation: felt(BRAAVOS_IMPL),
        },
        deployment: None,
    }
}

/// `get_signers`: `[stark...]`, then `secp256r1` and webauthn lists.
fn braavos_chain(stark: &[u64], strong: u64, removal: u64, timestamp: u64) -> Chain {
    let mut signers = vec![felt(stark.len() as u64)];
    signers.extend(stark.iter().map(|key| felt(*key)));
    signers.push(felt(strong));
    signers.extend((0..strong).map(|i| felt(0x5000 + i)));
    signers.push(Felt::ZERO);
    Chain {
        chain_id: SEPOLIA,
        class: Some(felt(BRAAVOS_IMPL)),
        views: vec![
            (selector(GET_SIGNERS), signers),
            (selector(GET_DEFERRED_REMOVE_SIGNERS), vec![felt(removal)]),
        ],
        timestamp,
    }
}

#[test]
fn a_braavos_account_needs_its_key_first_and_no_active_2fa() {
    assert_eq!(
        run(braavos(), &braavos_chain(&[KEY], 0, 0, 0)),
        AuthorizeStep::Authorized
    );
    assert_eq!(
        refused(run(braavos(), &braavos_chain(&[0x1, KEY], 0, 0, 0))),
        Code::BraavosOwnerMismatch
    );
    // A 2FA signer with no removal requested, or one not yet expired at the block.
    assert_eq!(
        refused(run(braavos(), &braavos_chain(&[KEY], 1, 0, 0))),
        Code::BraavosStrongSigner
    );
    assert_eq!(
        refused(run(braavos(), &braavos_chain(&[KEY], 1, 100, 50))),
        Code::BraavosStrongSigner
    );
    assert_eq!(
        run(braavos(), &braavos_chain(&[KEY], 1, 100, 150)),
        AuthorizeStep::Authorized
    );
    let mut other = braavos_chain(&[KEY], 0, 0, 0);
    other.class = Some(felt(0x999));
    assert_eq!(
        refused(run(braavos(), &other)),
        Code::BraavosClassUnrecognised
    );
    // No contract verifies a signature for an undeployed Braavos account.
    other.class = None;
    assert_eq!(refused(run(braavos(), &other)), Code::BraavosDeployFirst);
}

#[test]
fn another_chain_or_kind_is_refused_before_any_account_read() {
    let mut chain = ready_chain(Some(ARGENT_04), KEY, 0);
    chain.chain_id = "0x534e5f4d41494e";
    assert_eq!(refused(run(ready(None), &chain)), Code::WrongChain);
    let mut plan = Plan::new(ready(None), Kind::Deployment, POLICY);
    assert_eq!(
        refused(plan.authorize(StepReply::Start)),
        Code::PreparationUnavailable
    );
}
