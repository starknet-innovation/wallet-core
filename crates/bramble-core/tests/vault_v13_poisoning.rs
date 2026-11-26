//! P7: the address-poisoning check trusts every address the vault holds on the network, kept
//! records included, as the extension's does: `withTrustedRecipients`
//! (`apps/extension/src/background/transactions/trusted-recipients.ts:52-65`) puts every vault
//! account into `ownAccounts`, read by `vaultAccountAddresses`
//! (`background/runtime/runtime-state.ts:410-422`, `vault.accounts` of every kind), and
//! `verificationContext` keeps the ones on the sending account's network
//! (`features/transactions/src/routing/verification-context.ts:93`) for `addressPoisoningFlag`
//! (`routing/verify.ts:340-359`). The vault is `vault_v13_support::full()`, whose kept Ledger
//! account sits on Sepolia at G4's Argent address, which no Sepolia record mobile operates holds.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

use crate::send_support;
use crate::vault_v13_support as support;

use std::sync::Arc;

use bramble_core::testing::seal_container_vault;
use bramble_core::{ReviewFlag, SendFailureCode, StepReply, WalletCore};
use send_support::{failure, reviewed, sepolia, FakeClock, PASSWORD};
use serde_json::{json, Value};
use support::{full, kept_ids};

/// A lookalike of the kept Ledger address `0x26bc8d…87a1`: the same leading and trailing
/// windows, another account.
const TWIN: &str = "0x26bc000000000000000000000000000000000000000000000000000000087a1";

fn unlocked(payload: &Value) -> (tempfile::TempDir, Arc<WalletCore>) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_string_lossy().to_string();
    seal_container_vault(&path, PASSWORD, 13, &payload.to_string()).unwrap();
    let core = WalletCore::new(path, FakeClock::new()).unwrap();
    core.unlock(PASSWORD.into()).unwrap();
    (dir, core)
}

fn ledger_address(payload: &Value) -> String {
    let ledger = kept_ids(payload).ledger;
    ledger.split_once(':').unwrap().1.to_string()
}

#[test]
fn a_lookalike_of_a_kept_address_is_refused() {
    let payload = full();
    let ledger = ledger_address(&payload);
    assert_eq!(&ledger[ledger.len() - 4..], &TWIN[TWIN.len() - 4..]);
    let (_dir, core) = unlocked(&payload);
    let mut node = sepolia(true);
    let plan = reviewed(&core, &mut node, TWIN);
    let review = plan.current_review().unwrap();
    assert!(review.flags.contains(&ReviewFlag::AddressPoisoning) && !review.can_confirm);
    assert_eq!(
        failure(plan.advance(StepReply::Confirm).unwrap()).code,
        SendFailureCode::PoisonedRecipient
    );
}

/// A send to the person's own other account is ordinary: `isLookalikeAddress` is false for the
/// address itself, and `self-transfer` is only the sending account's own address.
#[test]
fn a_send_to_the_kept_address_itself_is_not_poisoned() {
    let payload = full();
    let (_dir, core) = unlocked(&payload);
    let mut node = sepolia(true);
    let review = reviewed(&core, &mut node, &ledger_address(&payload))
        .current_review()
        .unwrap();
    assert!(review.flags.is_empty() && review.can_confirm, "{review:?}");
}

/// Only the sending account's network counts: the same Ledger record on the extension's devnet
/// leaves the Sepolia lookalike unflagged.
#[test]
fn a_kept_address_on_another_network_is_not_trusted_here() {
    let mut payload = full();
    let ledger = kept_ids(&payload).ledger;
    let devnet_id = format!("SN_DEVNET:{}", ledger_address(&payload));
    for account in payload["accounts"].as_array_mut().unwrap() {
        if account["id"] == ledger {
            account["networkId"] = "SN_DEVNET".into();
            account["id"] = devnet_id.clone().into();
        }
    }
    // The Sepolia-bound records naming the Ledger account go with it.
    payload["accountSecrets"] = json!([]);
    payload["shadowAccountScopes"]
        .as_array_mut()
        .unwrap()
        .retain(|scope| scope["accountId"] != ledger);
    payload["originGrants"][0]["accountIds"][1] = devnet_id.into();
    let (_dir, core) = unlocked(&payload);
    let mut node = sepolia(true);
    let review = reviewed(&core, &mut node, TWIN).current_review().unwrap();
    assert!(
        !review.flags.contains(&ReviewFlag::AddressPoisoning) && review.can_confirm,
        "{review:?}"
    );
}
