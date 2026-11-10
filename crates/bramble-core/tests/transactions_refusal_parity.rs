//! The `*-refusals.json` Ready and Braavos accounts the extension refuses must be refused by the
//! plan before anything is signed, with the extension's copy (`extension-transactions-copy.json`).
//! Moved from mobile's `transactions_{argent_,braavos_}parity.rs` in Phase 2.3.

mod plan_support;

use plan_support::*;
use starknet_wallet_core::send::types::{SendError, SendFailureCode as Code, SendStep, StepReply};

fn code(name: &str) -> Code {
    match name {
        "accountClassUnsupported" => Code::AccountClassUnsupported,
        "accountClassUnrecognised" => Code::AccountClassUnrecognised,
        "accountNotDeployable" => Code::AccountNotDeployable,
        "accountOwnerMismatch" => Code::AccountOwnerMismatch,
        "accountOwnerTypeUnsupported" => Code::AccountOwnerTypeUnsupported,
        "accountGuardianUnsupported" => Code::AccountGuardianUnsupported,
        "braavosDeployFirst" => Code::BraavosDeployFirst,
        "braavosNotDeployable" => Code::BraavosNotDeployable,
        "braavosClassUnrecognised" => Code::BraavosClassUnrecognised,
        "braavosOwnerMismatch" => Code::BraavosOwnerMismatch,
        "braavosSignerListMalformed" => Code::BraavosSignerListMalformed,
        "braavosStrongSigner" => Code::BraavosStrongSigner,
        other => panic!("unmapped refusal {other}"),
    }
}

#[test]
fn accounts_the_extension_refuses_are_refused_before_signing() {
    let copy = capture("copy")["copy"]["failures"].clone();
    let mut count = 0;
    for family in ["argent", "braavos"] {
        for refusal in capture(&format!("{family}-refusals"))["refusals"]
            .as_array()
            .unwrap()
        {
            let name = refusal["name"].as_str().unwrap();
            let expected = code(refusal["code"].as_str().unwrap());
            let message = copy[refusal["code"].as_str().unwrap()].as_str().unwrap();
            let host = Host::new();
            let mut plan = plan(refusal, account(refusal, family, host.public_key()));
            let step = plan.advance(&host, StepReply::Start).unwrap();
            let step = drive(&mut plan, &host, step, &review_calls(refusal), refusal);
            let failure = match (refusal["reviewMessage"].as_str(), family) {
                // Ready's review turns the refusal into a failed fee estimate, so Confirm is
                // out of order; Braavos's review step itself fails with the code.
                (Some(text), "argent") => {
                    let review = plan.current_review().unwrap();
                    assert!(review.fee_estimate_failed && !review.can_confirm, "{name}");
                    let label = refusal["recordClass"].as_str().unwrap();
                    assert_eq!(text, message.replace("{label}", label), "{name}");
                    assert_eq!(
                        plan.advance(&host, StepReply::Confirm),
                        Err(SendError::OutOfOrder)
                    );
                    None
                }
                (Some(text), _) => {
                    assert_eq!(text, message, "{name}");
                    let SendStep::Failed { failure } = step else {
                        panic!("{name}: not refused, {step:?}");
                    };
                    Some(failure)
                }
                (None, _) => {
                    assert_eq!(step, SendStep::Review, "{name}");
                    let step = plan.advance(&host, StepReply::Confirm).unwrap();
                    let step = drive(&mut plan, &host, step, &phase(refusal, "submit"), refusal);
                    let SendStep::Failed { failure } = step else {
                        panic!("{name}: not refused, {step:?}");
                    };
                    let rendered = match &failure.detail {
                        Some(label) => message.replace("{label}", label),
                        None => message.to_string(),
                    };
                    assert_eq!(
                        rendered,
                        refusal["submitMessage"].as_str().unwrap(),
                        "{name}"
                    );
                    Some(failure)
                }
            };
            if let Some(failure) = failure {
                assert_eq!(failure.code, expected, "{name}");
            }
            assert_eq!(plan.signatures_made(), 0, "{name}");
            assert!(host.records.borrow().is_empty(), "{name}");
            count += 1;
        }
    }
    assert_eq!(count, 18);
}
