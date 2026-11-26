//! Every recorded SDK job's signed calldata binds to the actions it was built from.

use super::*;

/// The fixture's calldata is ours (bramble-core's `strk20_compiler_parity` proves it felt for
/// felt), so binding it binds our output: every job the SDK signed passes `pool_actions` against
/// the actions it was built from.
#[test]
fn every_recorded_job_binds_to_its_review() {
    let jsonl = include_str!("../../../bramble-core/tests/fixtures/extension-strk20-jobs.jsonl");
    let hex = |value: &serde_json::Value| felt::parse(value.as_str().unwrap()).unwrap();
    let decimal = |value: &serde_json::Value| Felt::from_dec_str(value.as_str().unwrap()).unwrap();
    let mut bound = 0;
    for line in jsonl.lines() {
        let job: serde_json::Value = serde_json::from_str(line).unwrap();
        let inputs = &job["inputs"];
        let Some(call) = job["signing"].get(0).map(|signing| &signing["calls"][0]) else {
            continue;
        };
        // The invoke phase's placeholders are Bramble's grammar: bramble-core's
        // `strk20_job_parity` binds those jobs through the whole proof job.
        let invokes = inputs["actions"].as_array().unwrap().iter().any(|action| {
            matches!(
                action["type"].as_str(),
                Some("invoke" | "shadow_account_invoke")
            )
        });
        if invokes {
            continue;
        }
        let mut reviewed = Vec::new();
        if inputs["builder"] != "actions" {
            reviewed.push(ReviewedAction::Register);
        }
        for action in inputs["actions"].as_array().unwrap() {
            let token = hex(&action["token"]);
            let recipient = || hex(&action["recipient"]);
            reviewed.push(match action["type"].as_str().unwrap() {
                "deposit" => ReviewedAction::Deposit {
                    token,
                    amount: decimal(&action["amount"]),
                    recipient: action.get("recipient").map(hex),
                },
                "transfer" if action["amount"] == "OPEN" => ReviewedAction::OpenTransfer {
                    token,
                    recipient: recipient(),
                },
                "transfer" => ReviewedAction::Transfer {
                    token,
                    recipient: recipient(),
                    amount: decimal(&action["amount"]),
                },
                _ => ReviewedAction::Withdraw {
                    token,
                    recipient: recipient(),
                    amount: decimal(&action["amount"]),
                },
            });
        }
        let calldata: Vec<SecretFelt> = call["calldata"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| SecretFelt::new(decimal(value)))
            .collect();
        let (account, key) = (hex(&inputs["account"]), hex(&inputs["viewingKey"]));
        let decoded = decode_compile_actions(&calldata, &account, &SecretFelt::new(key)).unwrap();
        let contracts = Contracts {
            pool: hex(&inputs["pool"]),
            anonymizer: None,
        };
        assert!(
            matches_review(&decoded, &reviewed, &account, &contracts),
            "{}",
            job["id"]
        );
        bound += 1;
    }
    // Tracks the fixture: 39 lines, of which 3 are reads (no `signing`), 6 refused by the SDK
    // before signing and 6 signed invoke jobs bound in bramble-core.
    assert_eq!(bound, 24);
}
