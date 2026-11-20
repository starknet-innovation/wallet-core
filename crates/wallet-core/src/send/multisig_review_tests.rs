//! One case per rule in `requireSafeArgentMultisigCalls` and `reviewArgentMultisigCalls` that the
//! fixtures do not reach.

use super::*;
use crate::accounts::multisig::tests::class;
use crate::accounts::multisig_change::ConfigChange;

const ACCOUNT: &str = "0x28417e292e102205acc710013171cc42ff60afde90193b1c57977fdec5501c0";

fn call(contract: &str, entrypoint: &str, calldata: &[&str]) -> ArgentMultisigCall {
    ArgentMultisigCall {
        calldata: calldata.iter().map(|v| v.to_string()).collect(),
        contract_address: contract.into(),
        entrypoint: entrypoint.into(),
    }
}

fn current() -> PublicConfig {
    PublicConfig {
        signer_public_keys: vec!["0x1".into(), "0x2".into()],
        threshold: 1,
    }
}

fn review(
    calls: &[ArgentMultisigCall],
    trusted: &[Felt],
    length: usize,
) -> Result<CallReview, CallReviewError> {
    review_calls(
        ACCOUNT,
        calls,
        Some(&current()),
        &class(),
        &Known::default(),
        trusted,
        length,
    )
}

#[test]
fn self_calls_are_alone_and_never_upgrade_callbacks() {
    use CallReviewError::*;
    let safe = |calls: &[ArgentMultisigCall]| require_safe_calls(ACCOUNT, calls, &class());
    let transfer = call("0x123", "transfer", &["0x456", "0x1", "0x0"]);
    assert_eq!(safe(&[]), Err(InvalidCalls));
    assert_eq!(safe(std::slice::from_ref(&transfer)), Ok(()));
    let threshold = call(ACCOUNT, "change_threshold", &["0x2"]);
    assert_eq!(
        safe(&[transfer.clone(), threshold.clone()]),
        Err(BatchSelfCall)
    );
    assert_eq!(
        safe(&[threshold.clone(), threshold.clone()]),
        Err(BatchSelfCall)
    );
    for callback in ["execute_after_upgrade", "perform_upgrade"] {
        assert_eq!(safe(&[call(ACCOUNT, callback, &[])]), Err(UpgradeCallback));
    }
    assert_eq!(
        safe(&[call(ACCOUNT, "upgrade", &["0x1"])]),
        Err(Change(ChangeError::UnsupportedSelfCall))
    );
    // `sameFelt` matches a padded spelling of the account too.
    assert_eq!(
        require_safe_calls(
            "0x028417e292e102205acc710013171cc42ff60afde90193b1c57977fdec5501c0",
            &[call(ACCOUNT, "perform_upgrade", &[])],
            &class()
        ),
        Err(UpgradeCallback)
    );
}

#[test]
fn a_self_call_needs_the_current_configuration() {
    let threshold = call(ACCOUNT, "change_threshold", &["0x2"]);
    let none = review_calls(
        ACCOUNT,
        std::slice::from_ref(&threshold),
        None,
        &class(),
        &Known::default(),
        &[],
        120,
    );
    assert_eq!(none, Err(CallReviewError::MissingCurrentConfig));
    let reviewed = review(&[threshold], &[], 120).unwrap();
    assert_eq!(reviewed.summary, "Change Argent multisig threshold to 2");
    let config = reviewed.config.unwrap();
    assert_eq!(
        config.change,
        ConfigChange::ChangeThreshold { threshold: 2 }
    );
    assert_eq!(config.next_config.threshold, 2);
    let unchanged = review(&[call(ACCOUNT, "change_threshold", &["0x1"])], &[], 120);
    assert_eq!(
        unchanged.map_err(|e| e.to_string()),
        Err("Argent multisig threshold is unchanged".to_string())
    );
}

#[test]
fn ordinary_batches_summarise_the_first_call_and_count_the_rest() {
    let transfer = call("0x123", "transfer", &["0x456", "0x1", "0x0"]);
    let other = call("0x123", "poke", &[]);
    let one = review(std::slice::from_ref(&transfer), &[], 120).unwrap();
    assert_eq!(one.summary, "Send 1 units to 0x456…x456");
    assert_eq!(one.config, None);
    let three = review(&[transfer.clone(), other.clone(), other], &[], 120).unwrap();
    assert_eq!(three.summary, "Send 1 units to 0x456…x456 (+2 more)");
    // Cut to the wallet's length in UTF-16 units, then trimmed.
    assert_eq!(
        review(std::slice::from_ref(&transfer), &[], 5)
            .unwrap()
            .summary,
        "Send"
    );
}

#[test]
fn a_payee_mimicking_a_trusted_address_is_refused() {
    let trusted =
        Felt::from_hex("0x0555aa7890abcdef1234567890abcdef1234567890abcdef1234567890aabbcc")
            .unwrap();
    let twin = "0x555af000000000000000000000000000000000000000000000000000000bbcc";
    for (entrypoint, calldata) in [
        ("transfer", vec![twin, "0x1", "0x0"]),
        ("transfer_from", vec!["0x1", twin, "0x1", "0x0"]),
        // Recipient enforcement does not wait for a summary: this span decodes as opaque.
        ("safe_transfer_from", vec!["0x1", twin, "0x1"]),
    ] {
        let calls = [call("0x123", entrypoint, &calldata)];
        assert_eq!(
            review(&calls, &[trusted], 120),
            Err(CallReviewError::PoisonedRecipient),
            "{entrypoint}"
        );
        assert!(review(&calls, &[], 120).is_ok(), "{entrypoint}");
    }
    // A spender is not a payee.
    let approve = [call("0x123", "approve", &[twin, "0x1", "0x0"])];
    assert!(review(&approve, &[trusted], 120).is_ok());
    // The multisig itself is always trusted.
    let account_twin = "0x2841000000000000000000000000000000000000000000000000000000001c0";
    let near_self = [call("0x123", "transfer", &[account_twin, "0x1", "0x0"])];
    assert_eq!(
        review(&near_self, &[], 120),
        Err(CallReviewError::PoisonedRecipient)
    );
}
