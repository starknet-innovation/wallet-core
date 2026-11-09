//! What the extension put on the wire, rebuilt by `starknet_wallet_core::send`: every request
//! body in the OpenZeppelin, Ready (Argent) and Braavos captures
//! (`extension-transactions-{wire,argent,argent-upgraded,braavos}.json`, see
//! `fixtures/README.md`), every signed transaction's hash
//! recomputed from its own fields, its resource bounds re-derived from an estimate the node gave
//! under Bramble's margin, and every hash the extension signed accounted for.
//!
//! `transactions_plan_parity.rs` drives the same captures through the plan; this pins the
//! mechanism underneath it independently of the plan's step order.

use std::collections::BTreeSet;

use bramble_core::FEE_MARGIN_PERCENT;
use serde_json::{json, Value};
use starknet_wallet_core::send::fees::{self, Bound, Bounds};
use starknet_wallet_core::send::rpc_bodies::{self, DeployTx, InvokeTx};
use starknet_wallet_core::send::sign::{self, Deployment};
use starknet_wallet_core::{derive_starknet_keypair, felt, sign_stark_hash, Felt, NetworkId};

const CAPTURES: [&str; 4] = ["wire", "argent", "argent-upgraded", "braavos"];
const QUERY_VERSION: &str = "0x100000000000000000000000000000003";

fn capture(name: &str) -> Value {
    let path = format!(
        "{}/tests/fixtures/extension-transactions-{name}.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn exchanges(scenario: &Value) -> Vec<Value> {
    ["reviewInvoke", "reviewDeployment", "submit"]
        .iter()
        .flat_map(|phase| scenario[phase].as_array().unwrap())
        .map(|text| serde_json::from_str(text.as_str().unwrap()).unwrap())
        .collect()
}

fn one(value: &Value) -> Felt {
    felt::parse(value.as_str().unwrap()).unwrap_or_else(|| panic!("not a felt: {value}"))
}

fn felts(value: &Value) -> Vec<Felt> {
    value.as_array().unwrap().iter().map(one).collect()
}

fn hex(value: &Value) -> u128 {
    u128::from_str_radix(value.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
}

fn bounds(value: &Value) -> Bounds {
    let bound = |name: &str| Bound {
        max_amount: hex(&value[name]["max_amount"]),
        max_price_per_unit: hex(&value[name]["max_price_per_unit"]),
    };
    Bounds {
        l1_gas: bound("l1_gas"),
        l2_gas: bound("l2_gas"),
        l1_data_gas: bound("l1_data_gas"),
    }
}

/// The transaction as the core describes it: bounds only on a signed one, as `rpc_bodies` sends.
struct Tx {
    fields: Value,
    sender: Felt,
    nonce: Felt,
    bounds: Option<Bounds>,
    calldata: Vec<Felt>,
    signature: Vec<Felt>,
}

impl Tx {
    fn new(fields: &Value) -> Self {
        let query = fields["version"] == QUERY_VERSION;
        if query {
            assert_eq!(bounds(&fields["resource_bounds"]), zero_bounds());
        } else {
            assert_eq!(fields["version"], "0x3");
        }
        let calldata = match fields["type"].as_str().unwrap() {
            "INVOKE" => &fields["calldata"],
            _ => &fields["constructor_calldata"],
        };
        Self {
            fields: fields.clone(),
            sender: fields.get("sender_address").map_or(Felt::ZERO, one),
            nonce: one(&fields["nonce"]),
            bounds: (!query).then(|| bounds(&fields["resource_bounds"])),
            calldata: felts(calldata),
            signature: felts(&fields["signature"]),
        }
    }

    fn felt(&self, name: &str) -> Felt {
        one(&self.fields[name])
    }

    fn deployment(&self) -> Deployment {
        Deployment {
            class_hash: self.felt("class_hash"),
            salt: self.felt("contract_address_salt"),
            constructor_calldata: self.calldata.clone(),
        }
    }
}

fn zero_bounds() -> Bounds {
    let zero = Bound {
        max_amount: 0,
        max_price_per_unit: 0,
    };
    Bounds {
        l1_gas: zero,
        l2_gas: zero,
        l1_data_gas: zero,
    }
}

fn invoke(tx: &Tx) -> InvokeTx<'_> {
    InvokeTx {
        sender: &tx.sender,
        calldata: &tx.calldata,
        nonce: &tx.nonce,
        bounds: tx.bounds.as_ref(),
        signature: &tx.signature,
    }
}

fn deploy<'a>(tx: &'a Tx, deployment: &'a Deployment) -> DeployTx<'a> {
    DeployTx {
        class_hash: &deployment.class_hash,
        salt: &deployment.salt,
        constructor_calldata: &deployment.constructor_calldata,
        nonce: &tx.nonce,
        bounds: tx.bounds.as_ref(),
        signature: &tx.signature,
    }
}

/// `body`'s method and params equal the recorded exchange's. starknet.js sends a no-argument
/// method without params; the core sends `[]`.
fn assert_body(body: &str, exchange: &Value, scenario: &str) {
    let body: Value = serde_json::from_str(body).unwrap();
    assert_eq!(body["method"], exchange["method"], "{scenario}");
    let expected = match &exchange["params"] {
        Value::Null => json!([]),
        params => params.clone(),
    };
    assert_eq!(
        body["params"], expected,
        "{scenario}: {}",
        exchange["method"]
    );
}

/// Every signed hash the scenario records, with the signature's `r`.
fn signed(scenario: &Value) -> Vec<(Felt, Felt)> {
    let records = scenario
        .get("signatures")
        .or_else(|| scenario.get("signed"))
        .unwrap();
    records
        .as_array()
        .unwrap()
        .iter()
        .map(|record| (one(&record["hash"]), one(&record["r"])))
        .collect()
}

/// The hash was signed, and the signature in `signature` is that signing.
fn assert_signed(scenario: &Value, hash: Felt, signature: &[Felt]) {
    let name = &scenario["name"];
    assert!(
        signed(scenario)
            .iter()
            .any(|(h, r)| *h == hash && signature.contains(r)),
        "{name}: {} unsigned or signature not carried",
        felt::canonical(&hash)
    );
}

fn account_address(scenario: &Value, exchanges: &[Value]) -> Felt {
    if let Some(address) = scenario["account"].get("address") {
        return one(address);
    }
    exchanges
        .iter()
        .find_map(|e| match e["method"].as_str().unwrap() {
            "starknet_getClassHashAt" | "starknet_getNonce" => {
                Some(one(&e["params"]["contract_address"]))
            }
            "starknet_addDeployAccountTransaction" => {
                Some(one(&e["reply"]["result"]["contract_address"]))
            }
            _ => None,
        })
        .unwrap()
}

/// Braavos carries its deployment parameters inside the deploy signature:
/// `[r, s, params (11), params_r, params_s]`, the parameters signed over `poseidon(params)`.
fn check_braavos_params(scenario: &Value, network: NetworkId, tx: &Tx) -> Option<Felt> {
    if tx.signature.len() != 15 {
        return None;
    }
    let params = &tx.signature[2..13];
    assert_eq!(params, sign::braavos_deploy_params(network, params[0]));
    let hash = sign::braavos_params_hash(params);
    assert_signed(scenario, hash, &tx.signature[13..]);
    Some(hash)
}

fn check_scenario(scenario: &Value) -> BTreeSet<Felt> {
    let name = scenario["name"].as_str().unwrap();
    let network = match scenario["networkId"].as_str().unwrap() {
        "SN_MAIN" => NetworkId::Mainnet,
        _ => NetworkId::Sepolia,
    };
    let exchanges = exchanges(scenario);
    let address = account_address(scenario, &exchanges);
    let mut quoted = Vec::new();
    let mut hashes = BTreeSet::new();
    for exchange in &exchanges {
        let params = &exchange["params"];
        let reply = &exchange["reply"]["result"];
        let body = match exchange["method"].as_str().unwrap() {
            "starknet_chainId" => starknet_wallet_core::rpc::chain_id_request(1),
            "starknet_blockHashAndNumber" => rpc_bodies::block_hash_and_number(1),
            "starknet_getNonce" => rpc_bodies::nonce(1, &one(&params["contract_address"])),
            "starknet_getClassHashAt" => {
                let address = one(&params["contract_address"]);
                match params["block_id"].get("block_hash") {
                    Some(block) => rpc_bodies::class_hash_at_block(1, &address, &one(block)),
                    None => rpc_bodies::class_hash_at(1, &address),
                }
            }
            "starknet_call" => {
                let request = &params["request"];
                assert_eq!(request["calldata"], json!([]), "{name}");
                rpc_bodies::call_at_block(
                    1,
                    &one(&request["contract_address"]),
                    &one(&request["entry_point_selector"]),
                    &one(&params["block_id"]["block_hash"]),
                )
            }
            "starknet_getBlockWithTxHashes" => {
                rpc_bodies::block_at(1, &one(&params["block_id"]["block_hash"]))
            }
            "starknet_getTransactionStatus" => {
                rpc_bodies::transaction_status(1, &one(&params["transaction_hash"]))
            }
            "starknet_getTransactionReceipt" => {
                rpc_bodies::transaction_receipt(1, &one(&params["transaction_hash"]))
            }
            "starknet_estimateFee" => {
                let txs: Vec<Tx> = params["request"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(Tx::new)
                    .collect();
                let invoke_tx = txs.iter().find(|tx| tx.fields["type"] == "INVOKE");
                let deploy_tx = txs.iter().find(|tx| tx.fields["type"] == "DEPLOY_ACCOUNT");
                let deployment = deploy_tx.map(Tx::deployment);
                if let (Some(tx), Some(deployment)) = (deploy_tx, &deployment) {
                    // Ready validates a placeholder of the right shape, `[1, 0, owner, 1, 1]`;
                    // Braavos's constructor checks the signature, so it signs the query-version
                    // hash for real. OpenZeppelin skips validation and sends none.
                    let placeholder = matches!(
                        tx.signature.as_slice(),
                        [one, zero, _, r, s] if *one == Felt::ONE && *zero == Felt::ZERO
                            && *r == Felt::ONE && *s == Felt::ONE
                    );
                    if !tx.signature.is_empty() && !placeholder {
                        let hash =
                            sign::deploy_query_hash(network, &address, deployment, &tx.nonce);
                        assert_signed(scenario, hash, &tx.signature);
                        hashes.insert(hash);
                        hashes.extend(check_braavos_params(scenario, network, tx));
                    }
                }
                let skip_validate = params["simulation_flags"] == json!(["SKIP_VALIDATE"]);
                for entry in reply.as_array().into_iter().flatten() {
                    quoted.extend(fees::quote(entry, FEE_MARGIN_PERCENT).ok().map(|q| q.0));
                }
                rpc_bodies::estimate(
                    1,
                    invoke_tx.map(invoke).as_ref(),
                    deploy_tx
                        .zip(deployment.as_ref())
                        .map(|(tx, d)| deploy(tx, d))
                        .as_ref(),
                    skip_validate,
                )
            }
            "starknet_addInvokeTransaction" => {
                let tx = Tx::new(&params["invoke_transaction"]);
                let signed_bounds = tx.bounds.unwrap();
                let hash =
                    sign::invoke_hash(network, &tx.sender, &tx.calldata, &tx.nonce, &signed_bounds)
                        .unwrap();
                assert_eq!(hash, one(&reply["transaction_hash"]), "{name}");
                assert_signed(scenario, hash, &tx.signature);
                assert!(quoted.contains(&signed_bounds), "{name}: bounds not quoted");
                hashes.insert(hash);
                rpc_bodies::add_invoke(1, &invoke(&tx))
            }
            "starknet_addDeployAccountTransaction" => {
                let tx = Tx::new(&params["deploy_account_transaction"]);
                let signed_bounds = tx.bounds.unwrap();
                let deployment = tx.deployment();
                assert_eq!(tx.nonce, Felt::ZERO, "{name}");
                let hash =
                    sign::deploy_hash(network, &address, &deployment, &signed_bounds).unwrap();
                assert_eq!(hash, one(&reply["transaction_hash"]), "{name}");
                assert_signed(scenario, hash, &tx.signature);
                assert!(quoted.contains(&signed_bounds), "{name}: bounds not quoted");
                hashes.insert(hash);
                hashes.extend(check_braavos_params(scenario, network, &tx));
                rpc_bodies::add_deploy(1, &deploy(&tx, &deployment))
            }
            other => panic!("{name}: no builder for {other}"),
        };
        assert_body(&body, exchange, name);
    }
    if let Some(hash) = scenario.get("transactionHash") {
        assert!(
            hashes.contains(&one(hash)),
            "{name}: final hash not rebuilt"
        );
    }
    hashes
}

#[test]
fn every_request_and_signed_hash_matches_the_extension() {
    let mut scenarios = 0;
    for name in CAPTURES {
        for scenario in capture(name)["scenarios"].as_array().unwrap() {
            let hashes = check_scenario(scenario);
            // Nothing the extension signed is left unexplained.
            for (hash, _) in signed(scenario) {
                assert!(
                    hashes.contains(&hash),
                    "{}: signed {} not rebuilt",
                    scenario["name"],
                    felt::canonical(&hash)
                );
            }
            scenarios += 1;
        }
    }
    assert_eq!(scenarios, 20, "every capture scenario ran");
}

#[test]
fn signatures_are_the_extensions_under_the_derived_key() {
    let wire = capture("wire");
    let mnemonic = wire["mnemonic"].as_str().unwrap();
    for scenario in wire["scenarios"].as_array().unwrap() {
        let index = u32::try_from(scenario["accountIndex"].as_u64().unwrap()).unwrap();
        let pair = derive_starknet_keypair(mnemonic, 0, index, None).unwrap();
        for record in scenario["signatures"].as_array().unwrap() {
            let key = &pair.private_key;
            let signature = sign_stark_hash(key, record["hash"].as_str().unwrap()).unwrap();
            assert_eq!(signature.r, record["r"].as_str().unwrap());
            assert_eq!(signature.s, record["s"].as_str().unwrap());
        }
    }
}
