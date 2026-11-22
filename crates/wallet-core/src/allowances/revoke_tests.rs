// The revoke as `allowance-ops.ts`'s handler makes it, from `background-allowance-messages`,
// `background-external-allowances`, `background-allowance-ops-branches` and
// `allowance-revoke-outcome` (the extension's root `tests/background/portfolio/`) and
// `allowance-recording.test.ts`'s target cases, without the browser glue (messages, the
// session, the fee review), plus the guards they leave out. The plan's submission is `send`'s.

use super::calls_tests::*;
use super::discovery::AllowanceReading;
use super::record::AllowanceRecord;
use super::revoke::*;
use crate::send::call_decode::Known;
use crate::send::calls::Call;
use crate::send::fees::Headroom;
use crate::send::invoke::{Invoke, InvokeRefusal};
use crate::send::multisig_invoke::StarkPrimitives;
use crate::send::review_flags::ReviewContext;
use crate::send::types::{Account, Authorization, Policy};
use crate::{Felt, NetworkId};

/// Test-only: the selector is sha3's Keccak (a dev-dependency); nothing here verifies.
struct Keccak;

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

/// Bramble's `INVOKE_LIMITS` numbers, as `SEND_POLICY` sets them.
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

// A full-length spender, so ids run to their real length (the extension once capped ids below
// it and refused most genuine revokes).
pub(super) const LONG_SPENDER: &str =
    "0x755555555555555555555555555555555555555555555555555555555555555";

fn account_on(network: NetworkId, address: &str) -> Account {
    Account {
        network,
        address: f(address),
        public_key: Felt::from(3u8),
        authorization: Authorization::StarkPair,
        deployment: None,
    }
}

fn account() -> Account {
    account_on(NetworkId::Mainnet, ADDRESS)
}

fn id_for(token: &str, spender: &str) -> String {
    format!("{ACCOUNT}:{token}:{spender}")
}

pub(super) fn resolve(
    id: &str,
    stored: &[AllowanceRecord],
    known: &Known,
) -> Result<RevokeTarget, RevokeRefusal> {
    RevokeTarget::resolve(id, &account(), stored, known)
}

pub(super) fn build(
    target: &RevokeTarget,
    known: &Known,
    readings: &[AllowanceReading],
) -> Result<Invoke, RevokeRefusal> {
    target.invoke(&account(), &context(known), readings, &POLICY, &Keccak)
}

/// What the wallet knows on the account's network: here only its trusted lists.
pub(super) fn context<'a>(known: &Known<'a>) -> ReviewContext<'a> {
    ReviewContext {
        known: *known,
        ..ReviewContext::default()
    }
}

pub(super) fn reading(id: &str, value: &str) -> AllowanceReading {
    AllowanceReading {
        id: id.into(),
        amount: amount(value),
    }
}

fn approve_zero(token: &str, spender: &str) -> Call {
    grant(token, "approve", spender, 0, 0)
}

#[test]
fn allowance_target_names_only_the_accounts_own_well_formed_pairs() {
    let id = id_for(CANONICAL_TOKEN, CANONICAL_SPENDER);
    assert_eq!(allowance_target(&id, ACCOUNT), Some((f(TOKEN), f(SPENDER))));
    // Padded and decimal spellings name the same pair (`canonicalFelt`).
    let padded = id_for(TOKEN, "85");
    assert_eq!(
        allowance_target(&padded, ACCOUNT),
        Some((f(TOKEN), f("0x55")))
    );
    for bad in [
        // "still rejects revoking an id that does not belong to the active account".
        "SN_MAIN:0x9999:0x1:0x2".to_string(),
        format!("{ACCOUNT}x:{CANONICAL_TOKEN}:0x5"),
        format!("{ACCOUNT}{CANONICAL_TOKEN}:0x5"),
        format!("{ACCOUNT}:{CANONICAL_TOKEN}"),
        format!("{ACCOUNT}:{CANONICAL_TOKEN}:0x5:0x6"),
        format!("{ACCOUNT}:0x0:0x5"),
        format!("{ACCOUNT}:{CANONICAL_TOKEN}:0x0"),
        format!("{ACCOUNT}:{CANONICAL_TOKEN}:"),
        format!("{ACCOUNT}:nope:0x5"),
        format!("{ACCOUNT}:{CANONICAL_TOKEN}:-5"),
        // At the field prime.
        format!("{ACCOUNT}:{CANONICAL_TOKEN}:0x800000000000011000000000000000000000000000000000000000000000001"),
    ] {
        assert_eq!(allowance_target(&bad, ACCOUNT), None, "{bad}");
    }
}

#[test]
fn revokes_a_tracked_allowance_with_a_zero_amount_approve() {
    // "revokes a tracked allowance with a zero-amount approve": `approve(spender, 0x0, 0x0)`.
    let lists = Lists::standard();
    let record = stored(ACCOUNT, NetworkId::Mainnet, CANONICAL_TOKEN, amount("1000"));
    let target = resolve(&record.id, std::slice::from_ref(&record), &lists.known()).unwrap();
    assert_eq!(target.id(), record.id);
    assert_eq!(target.confirmation_pair(&lists.known()), None);
    let invoke = build(&target, &lists.known(), &[]).unwrap();
    assert_eq!(invoke.calls, [approve_zero(TOKEN, "0x5")]);
    assert_eq!(
        (invoke.network, invoke.sender),
        (NetworkId::Mainnet, f(ADDRESS))
    );
    // The activity line the revoke records is the review's summary (`report.calls[0].summary`).
    assert_eq!(invoke.summary(), "Allow 0x5…0x5 to spend 0 ETH");
}

#[test]
fn revokes_an_externally_discovered_allowance_not_held_in_the_log() {
    let lists = Lists::standard();
    let id = id_for(CANONICAL_TOKEN, LONG_SPENDER);
    assert!(id.len() > 200);
    let target = resolve(&id, &[], &lists.known()).unwrap();
    let invoke = build(&target, &lists.known(), &[]).unwrap();
    assert_eq!(invoke.calls, [approve_zero(TOKEN, LONG_SPENDER)]);
}

#[test]
fn refuses_malformed_unknown_and_other_accounts_ids() {
    let lists = Lists::standard();
    let known = lists.known();
    // `readRevokeAllowanceInput`: a non-empty id of at most 240 characters.
    assert_eq!(resolve("", &[], &known), Err(RevokeRefusal::InvalidRequest));
    let prefix = format!("{ACCOUNT}:{CANONICAL_TOKEN}:0x");
    let long = format!("{prefix}{}5", "0".repeat(240 - prefix.len()));
    assert_eq!(long.len(), 241);
    assert_eq!(
        resolve(&long, &[], &known),
        Err(RevokeRefusal::InvalidRequest)
    );
    assert_eq!(
        resolve(&long[1..], &[], &known),
        Err(RevokeRefusal::NotFound)
    );
    // "rejects revoking an unknown allowance".
    assert_eq!(
        resolve("SN_MAIN:0x1:0x2", &[], &known),
        Err(RevokeRefusal::NotFound)
    );
    // "refuses to revoke an allowance recorded under a different account".
    let other = "SN_MAIN:0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdf";
    let theirs = stored(other, NetworkId::Mainnet, CANONICAL_TOKEN, amount("5"));
    assert_eq!(
        resolve(&theirs.id, std::slice::from_ref(&theirs), &known),
        Err(RevokeRefusal::OtherAccount)
    );
    // Beyond the extension's tests: a record with this account's id but another network, and a
    // record whose token is not a felt (neither loads from storage; both are refused here).
    let mut elsewhere = stored(ACCOUNT, NetworkId::Mainnet, CANONICAL_TOKEN, amount("5"));
    elsewhere.network = NetworkId::Sepolia;
    assert_eq!(
        resolve(&elsewhere.id, std::slice::from_ref(&elsewhere), &known),
        Err(RevokeRefusal::OtherAccount)
    );
    for (token, spender) in [("0x0", "0x5"), (CANONICAL_TOKEN, "0x0"), ("nope", "0x5")] {
        let mut broken = stored(ACCOUNT, NetworkId::Mainnet, CANONICAL_TOKEN, amount("5"));
        broken.token = token.into();
        broken.spender = spender.into();
        assert_eq!(
            resolve(&broken.id, std::slice::from_ref(&broken), &known),
            Err(RevokeRefusal::NotFound),
            "{token} {spender}"
        );
    }
}

#[test]
fn a_watched_collection_is_refused_before_and_at_the_build() {
    // "revoking a collection row is refused instead of broadcasting an approval".
    let lists = Lists::standard();
    let row = stored(ACCOUNT, NetworkId::Mainnet, COLLECTION, amount("7"));
    assert_eq!(
        resolve(&row.id, std::slice::from_ref(&row), &lists.known()),
        Err(RevokeRefusal::Collection)
    );
    assert_eq!(
        resolve(&id_for(COLLECTION, "0x5"), &[], &lists.known()),
        Err(RevokeRefusal::Collection)
    );
    // Watched between resolving and building: refused again, whatever a reading says.
    let target = resolve(&row.id, std::slice::from_ref(&row), &Known::default()).unwrap();
    assert_eq!(
        build(&target, &lists.known(), &[reading(&row.id, "7")]),
        Err(RevokeRefusal::Collection)
    );
}

#[test]
fn an_unconfirmed_contract_needs_the_chain_to_answer_for_the_pair() {
    // "an approve on an unwatched collection is listed unconfirmed and cannot be revoked", and
    // "the same approve on a dapp's own ERC20 stays listed and revokable".
    let lists = Lists::standard();
    let known = lists.known();
    let row = stored(ACCOUNT, NetworkId::Mainnet, UNKNOWN, amount("5"));
    let target = resolve(&row.id, std::slice::from_ref(&row), &known).unwrap();
    let pair = target.confirmation_pair(&known).unwrap();
    assert_eq!(pair.id, row.id);
    assert_eq!(
        (
            pair.owner.as_str(),
            pair.token.as_str(),
            pair.spender.as_str()
        ),
        (
            ADDRESS,
            "0x777777777777777777777777777777777777777777777777777777777777771",
            "0x5"
        )
    );
    assert_eq!(
        build(&target, &known, &[]),
        Err(RevokeRefusal::UnconfirmedToken)
    );
    let other_pair = reading(&id_for(UNKNOWN, "0x6"), "5000");
    assert_eq!(
        build(&target, &known, &[other_pair]),
        Err(RevokeRefusal::UnconfirmedToken)
    );
    // Any answer for the pair proves the `allowance` view exists, a zero one too.
    for value in ["5000", "0"] {
        let invoke = build(&target, &known, &[reading(&row.id, value)]).unwrap();
        assert_eq!(invoke.calls, [approve_zero(UNKNOWN, "0x5")]);
    }
}

#[test]
fn a_revoke_is_reviewed_without_the_hosts_pinned_contracts() {
    // `verifyCalls([call], trusted)`: a token known only from the read is an unknown contract,
    // even one the host pinned for another flow.
    let lists = Lists::standard();
    let row = stored(ACCOUNT, NetworkId::Mainnet, UNKNOWN, amount("5"));
    let target = resolve(&row.id, std::slice::from_ref(&row), &lists.known()).unwrap();
    let pinned = [f(UNKNOWN)];
    let context = ReviewContext {
        known_contracts: &pinned,
        ..context(&lists.known())
    };
    let invoke = target
        .invoke(
            &account(),
            &context,
            &[reading(&row.id, "5")],
            &POLICY,
            &Keccak,
        )
        .unwrap();
    let codes: Vec<_> = invoke.flags().iter().map(|flag| flag.code()).collect();
    assert_eq!(codes, ["unknown-contract"]);
}

#[test]
fn a_target_is_built_only_for_the_account_and_network_it_was_resolved_for() {
    let lists = Lists::standard();
    let known = lists.known();
    let id = id_for(CANONICAL_TOKEN, "0x5");
    let target = resolve(&id, &[], &known).unwrap();
    let other = "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdf";
    for account in [
        account_on(NetworkId::Sepolia, ADDRESS),
        account_on(NetworkId::Mainnet, other),
    ] {
        assert_eq!(
            target.invoke(&account, &context(&known), &[], &POLICY, &Keccak),
            Err(RevokeRefusal::OtherAccount)
        );
    }
    // Resolved on Sepolia, the same id names nothing.
    let sepolia = account_on(NetworkId::Sepolia, ADDRESS);
    assert_eq!(
        RevokeTarget::resolve(&id, &sepolia, &[], &known),
        Err(RevokeRefusal::NotFound)
    );
}

#[test]
fn the_batch_review_still_applies() {
    let lists = Lists::standard();
    let target = resolve(&id_for(CANONICAL_TOKEN, "0x5"), &[], &lists.known()).unwrap();
    let strict = Policy {
        max_invoke_calls: 0,
        ..POLICY
    };
    assert_eq!(
        target.invoke(&account(), &context(&lists.known()), &[], &strict, &Keccak),
        Err(RevokeRefusal::Invoke(InvokeRefusal::TooManyCalls))
    );
}

#[test]
fn refusals_carry_the_extensions_sentences() {
    for (refusal, sentence) in [
        (RevokeRefusal::InvalidRequest, "invalid revoke request"),
        (
            RevokeRefusal::OtherAccount,
            "switch to the account that granted this allowance",
        ),
        (RevokeRefusal::NotFound, "allowance not found"),
        (
            RevokeRefusal::Collection,
            "this contract is an NFT collection, not a token; revoke it from the dapp",
        ),
        (
            RevokeRefusal::UnconfirmedToken,
            "this wallet could not confirm that contract is a token, so it will not send a revoke that could approve an NFT instead; revoke it from the dapp",
        ),
        (RevokeRefusal::Invoke(InvokeRefusal::NoCalls), "No calls to submit"),
    ] {
        assert_eq!(refusal.to_string(), sentence);
    }
}
