// The extension's `tests/background/approvals/outside-execution-trusted-recipients.test.ts`.
// It gathers the trusted set itself (contacts, vault accounts, watched multisigs, paid
// recipients); here the caller passes it as `ReviewContext::trusted`, as on the invoke route.

use super::*;

const SECOND: &str = "0x07aa11111111111111111111111111111111111111111111111111111111c0de";
const SECOND_TWIN: &str = "0x07aa12222222222222222222222222222222222222222222222222222222c0de";
const PAID: &str = "0x049d3aa7890abcdef1234567890abcdef1234567890abcdef1234567890a9f1e";
const PAID_TWIN: &str = "0x049d3ff111111111111111111111111111111111111111111111111111119f1e";
const SHARED: &str = "0x05bb33333333333333333333333333333333333333333333333333333333f00d";
const SHARED_TWIN: &str = "0x05bb34444444444444444444444444444444444444444444444444444444f00d";

fn transfer_to(recipient: &str) -> Value {
    outside(&[call(ETH, "transfer", &[recipient, "0x1", "0x0"])], 2)
}

/// `expectFlaggedThenRefused`: queued with its flag, refused at approval.
fn expect_flagged_then_refused(twin: &str, trusted: &[Felt]) {
    let typed = transfer_to(twin);
    assert!(queued_flags(&typed, trusted, &[]).contains(&"address-poisoning"));
    assert_eq!(review(&typed, trusted, &[], Stage::Sign), refused());
}

#[test]
fn flags_a_transfer_to_a_lookalike_of_a_recipient_this_wallet_paid_and_refuses_it_at_approval() {
    expect_flagged_then_refused(PAID_TWIN, &[felt(PAID)]);
}

#[test]
fn flags_a_transfer_to_a_lookalike_of_the_users_other_vault_account_and_refuses_it_at_approval() {
    expect_flagged_then_refused(SECOND_TWIN, &[felt(SECOND)]);
}

#[test]
fn flags_a_transfer_to_a_lookalike_of_a_watched_shared_account_and_refuses_it_at_approval() {
    expect_flagged_then_refused(SHARED_TWIN, &[felt(SHARED)]);
}

#[test]
fn still_queues_a_transfer_to_the_paid_recipient_itself() {
    let typed = transfer_to(PAID);
    let trusted = [felt(PAID)];
    assert!(!queued_flags(&typed, &trusted, &[]).contains(&"address-poisoning"));
    assert!(matches!(
        review(&typed, &trusted, &[], Stage::Sign),
        Ok(Some(_))
    ));
}

#[test]
fn refuses_at_approval_when_a_payment_made_meanwhile_makes_the_payee_a_lookalike() {
    let typed = transfer_to(PAID_TWIN);
    assert!(!queued_flags(&typed, &[], &[]).contains(&"address-poisoning"));
    assert_eq!(review(&typed, &[felt(PAID)], &[], Stage::Sign), refused());
}
