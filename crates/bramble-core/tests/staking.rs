//! Native staking through mobile's exports, end to end against a canned Mainnet node: the
//! directory and positions requests and judges, the pinned chain read and its rows, and each
//! action from the ids and amount the host names to the calldata the plan signs, with a stake's
//! STRK approval filed in the allowance log before its activity row. The mechanism is
//! wallet-core's `staking` (its unit tests carry the extension's cases); the chain here is the
//! layout of its `test_chain`, behind the real Mainnet staking contract.
#![cfg(feature = "testing")] // Mobile's suite: `--features mobile,testing`.

mod staking_support;

use bramble_core::{
    staking_directory_request, staking_directory_view, CallKind, DirectoryView, FeeAffordability,
    HttpOutcome, NetworkId, PositionState, SendStep, StakingAction, StakingError, StepReply,
    TokenPreferences,
};
use serde_json::json;
use staking_support::export::*;

#[test]
fn a_stake_is_read_reviewed_and_signed_with_its_approval_filed_before_the_row() {
    let wallet = mainnet();
    let chain = Chain::new(&wallet.address);
    // The overview: the verified pool, not yet a member.
    let read = wallet
        .core
        .staking_overview(wallet.id.clone(), STAKER.into(), wallet.prefs.clone())
        .unwrap();
    run(&chain, || read.next_calls(), |r| read.accept(r));
    let view = read.view(NOW_S).unwrap();
    assert_eq!((view.exit_window_seconds, view.complete), (604_800, true));
    let row = view.rows.iter().find(|row| row.symbol == "STRK").unwrap();
    let figures = row.figures.as_ref().unwrap();
    assert_eq!((row.staker.as_str(), row.refusal.as_ref()), (STAKER, None));
    assert_eq!(
        (figures.staked.as_str(), figures.state),
        ("0", PositionState::NotStaked)
    );
    assert!(figures.accepting);

    let staking = wallet
        .plan(&chain, StakingAction::Stake, Some("1.5"))
        .unwrap();
    assert!(staking.review.contains("1.5 STRK"), "{}", staking.review);
    assert!(staking.review.contains(STAKER), "{}", staking.review);
    let plan = staking.plan;
    let amount = 3 * E18 / 2;
    let expected = execute(&[
        (
            STRK,
            "approve",
            vec![POOL.into(), hex(amount), "0x0".into()],
        ),
        (
            POOL,
            "enter_delegation_pool",
            vec![wallet.address.clone(), hex(amount)],
        ),
    ]);
    let first = plan.advance(StepReply::Start).unwrap();
    assert_eq!(wallet.drive(&plan, first), SendStep::Review);
    let review = plan.current_review().unwrap();
    assert!(review.can_confirm);
    assert_eq!(review.affordability, FeeAffordability::Sufficient);
    let calls = plan.reviewed_calls();
    let shape: Vec<_> = calls
        .iter()
        .map(|c| (c.kind, canon(&c.contract_address), c.entrypoint.as_str()))
        .collect();
    assert_eq!(shape[0], (CallKind::Approve, canon(STRK), "approve"));
    assert_eq!(
        (shape[1].1.as_str(), shape[1].2),
        (POOL, "enter_delegation_pool")
    );
    assert!(plan.risk_flags().is_empty(), "{:?}", plan.risk_flags());

    // The activity file cannot be written: the approval is still filed, before the write left.
    std::fs::create_dir(wallet.dir.path().join("activity.v1.json.tmp")).unwrap();
    let mut step = plan.advance(StepReply::Confirm).unwrap();
    let mut node = Node::new(NetworkId::Mainnet, true);
    loop {
        let (id, method, params, is_write) = rpc_parts(&step);
        if is_write {
            let filed = wallet.allowance_log();
            assert_eq!(filed.len(), 1, "filed before the write left: {filed:?}");
            let grant = &filed[0];
            assert_eq!(canon(grant["spender"].as_str().unwrap()), POOL);
            assert_eq!(canon(grant["token"].as_str().unwrap()), canon(STRK));
            assert_eq!(grant["amount"], json!(amount.to_string()));
            assert_eq!(grant["accountId"], json!(wallet.id));
            let written = felts(&params["invoke_transaction"]["calldata"]);
            assert_eq!(written, expected, "the signed calldata");
            break;
        }
        let answer = node.reply(&method, &params, None);
        step = plan.advance(rpc_reply(id, &answer)).unwrap();
    }
    assert!(wallet.core.activity(wallet.id.clone()).unwrap().is_empty());
}

#[test]
fn a_submitted_stake_names_its_row() {
    let wallet = mainnet();
    let chain = Chain::new(&wallet.address);
    let plan = wallet
        .plan(&chain, StakingAction::Stake, Some("1.5"))
        .unwrap()
        .plan;
    let first = plan.advance(StepReply::Start).unwrap();
    assert_eq!(wallet.drive(&plan, first), SendStep::Review);
    let step = plan.advance(StepReply::Confirm).unwrap();
    assert!(matches!(
        wallet.drive(&plan, step),
        SendStep::Broadcast { .. }
    ));
    let row = &wallet.core.activity(wallet.id.clone()).unwrap()[0];
    assert_eq!(row.summary, "Stake 1.5 STRK with validator 0x5a20…beef");
    assert_eq!(plan.signatures_made(), 1);
}

#[test]
fn each_action_signs_the_calls_its_position_allows() {
    let wallet = mainnet();
    let me = wallet.address.clone();
    let mut chain = Chain::new(&me);
    chain.member = Some(Member {
        amount: 5 * E18,
        unclaimed: E18,
        unpool: None,
    });
    let signed = |chain: &Chain, action, amount| {
        let plan = wallet.plan(chain, action, amount).unwrap().plan;
        signed_calldata(&wallet, &plan)
    };
    let stake = signed(&chain, StakingAction::Stake, Some("1"));
    let approve = (STRK, "approve", vec![POOL.into(), hex(E18), "0x0".into()]);
    let add = (POOL, "add_to_delegation_pool", vec![me.clone(), hex(E18)]);
    assert_eq!(stake, execute(&[approve, add]));
    let claim = signed(&chain, StakingAction::Claim, None);
    assert_eq!(claim, execute(&[(POOL, "claim_rewards", vec![me.clone()])]));
    let unstake = signed(&chain, StakingAction::Unstake, Some("2"));
    let intent = (POOL, "exit_delegation_pool_intent", vec![hex(2 * E18)]);
    assert_eq!(unstake, execute(&[intent]));
    // Past its window, the pending exit can be withdrawn or cancelled.
    chain.member = Some(Member {
        amount: 3 * E18,
        unclaimed: 0,
        unpool: Some((2 * E18, NOW_S - 1)),
    });
    let withdraw = signed(&chain, StakingAction::Withdraw, None);
    let action = (POOL, "exit_delegation_pool_action", vec![me.clone()]);
    assert_eq!(withdraw, execute(&[action]));
    let cancel = signed(&chain, StakingAction::CancelUnstake, None);
    let keep = (POOL, "exit_delegation_pool_intent", vec!["0x0".into()]);
    assert_eq!(cancel, execute(&[keep]));
    let refusal = wallet
        .plan(&chain, StakingAction::Claim, None)
        .err()
        .map(refused);
    assert_eq!(refusal.as_deref(), Some("No rewards to claim."));
}

#[test]
fn staking_is_refused_off_mainnet_and_for_unknown_input() {
    let wallet = mainnet();
    let sepolia = oz_id(&wallet.core, NetworkId::Sepolia, 0);
    let off = Some("Staking isn't available on this network.".to_string());
    let core = wallet.core.clone();
    let prefs = || wallet.prefs.clone();
    let stake = core.clone().prepare_staking(
        sepolia.clone(),
        StakingAction::Claim,
        STAKER.into(),
        wallet.strk_id(),
        None,
        prefs(),
    );
    assert_eq!(stake.err().map(refused), off);
    let overview = core.staking_overview(sepolia.clone(), STAKER.into(), prefs());
    assert_eq!(overview.err().map(refused), off);
    assert_eq!(
        core.staking_positions_request(sepolia, prefs())
            .err()
            .map(refused),
        off
    );
    assert_eq!(
        staking_directory_request(NetworkId::Sepolia, None)
            .err()
            .map(refused),
        off
    );

    let unknown = core.clone().prepare_staking(
        format!("SN_MAIN:{ACCOUNT_1}"),
        StakingAction::Claim,
        STAKER.into(),
        wallet.strk_id(),
        None,
        prefs(),
    );
    assert_eq!(unknown.err(), Some(StakingError::UnknownAccount));
    let untracked = core.clone().prepare_staking(
        wallet.id.clone(),
        StakingAction::Claim,
        STAKER.into(),
        "SN_SEPOLIA:strk".into(),
        None,
        prefs(),
    );
    let option = Some("Staking option unavailable.".to_string());
    assert_eq!(untracked.err().map(refused), option);
    let invalid = Some("invalid staking request".to_string());
    for (action, amount) in [
        (StakingAction::Stake, None),
        (StakingAction::Claim, Some("1")),
        (StakingAction::Unstake, Some("-1")),
    ] {
        assert_eq!(wallet.prepare(action, amount).err().map(refused), invalid);
    }
    let pasted = core.staking_overview(wallet.id.clone(), "0x0".into(), prefs());
    assert_eq!(pasted.err().map(refused), invalid);
}

#[test]
fn a_plan_needs_a_finished_verified_read() {
    let wallet = mainnet();
    let mut chain = Chain::new(&wallet.address);
    let unreadable = Some("Couldn't read staking on this network. Try again.".to_string());
    // Before any reply, and after a restart mid-read: no reading.
    let prepared = wallet.prepare(StakingAction::Stake, Some("1")).unwrap();
    let plan = |p: &bramble_core::StakingPreparation| p.plan(NOW_S, None).err().map(refused);
    assert_eq!(plan(&prepared), unreadable);
    for call in prepared.next_calls() {
        prepared.accept(chain.reply(&call));
    }
    for call in prepared.next_calls() {
        prepared.accept(chain.reply(&call));
    }
    prepared.restart();
    assert_eq!(plan(&prepared), unreadable);
    // A restart after a finished read keeps none of its facts, for the plan or the rows.
    run(&chain, || prepared.next_calls(), |r| prepared.accept(r));
    assert!(prepared.plan(NOW_S, None).is_ok());
    prepared.restart();
    assert_eq!(plan(&prepared), unreadable);
    let read = wallet
        .core
        .staking_overview(wallet.id.clone(), STAKER.into(), wallet.prefs.clone())
        .unwrap();
    run(&chain, || read.next_calls(), |r| read.accept(r));
    assert!(read.view(NOW_S).is_ok());
    read.restart();
    assert_eq!(read.view(NOW_S).err().map(refused), unreadable);
    // A pool whose own parameters name another staker is never staked into.
    chain.pool_staker = "0x5a4";
    let forged = wallet
        .plan(&chain, StakingAction::Stake, Some("1"))
        .err()
        .map(refused);
    assert_eq!(
        forged.as_deref(),
        Some("Couldn't verify this validator on chain.")
    );
    let read = wallet
        .core
        .staking_overview(wallet.id.clone(), STAKER.into(), wallet.prefs.clone())
        .unwrap();
    run(&chain, || read.next_calls(), |r| read.accept(r));
    let row = read.view(NOW_S).unwrap().rows.remove(0);
    assert_eq!(row.figures, None);
    assert_eq!(row.refusal.as_deref(), forged.as_deref());
    // Nothing staked: there is nothing to unstake.
    chain.pool_staker = STAKER;
    let unstake = wallet
        .plan(&chain, StakingAction::Unstake, Some("1"))
        .err()
        .map(refused);
    assert_eq!(unstake.as_deref(), Some("Nothing staked to unstake."));
}

// A plan holds the session it was prepared in: a lock and a fresh unlock void it.
#[test]
fn a_lock_voids_a_prepared_stake() {
    let wallet = mainnet();
    let chain = Chain::new(&wallet.address);
    let prepared = wallet.prepare(StakingAction::Stake, Some("1")).unwrap();
    run(&chain, || prepared.next_calls(), |r| prepared.accept(r));
    wallet.core.lock();
    assert_eq!(prepared.plan(NOW_S, None).err(), Some(StakingError::Locked));
    wallet.core.unlock(PASSWORD.into()).unwrap();
    let plan = prepared.plan(NOW_S, None).unwrap().plan;
    assert_eq!(
        plan.current_review(),
        None,
        "not reviewed before it is started"
    );
    let first = plan.advance(StepReply::Start).unwrap();
    assert_eq!(wallet.drive(&plan, first), SendStep::Review);
    assert_eq!(
        plan.current_review().unwrap().affordability,
        FeeAffordability::Unknown
    );
    let step = plan.advance(StepReply::Confirm).unwrap();
    let failure = failure(wallet.drive(&plan, step));
    assert_eq!(failure.code, bramble_core::SendFailureCode::Locked);
    assert_eq!(plan.signatures_made(), 0);
    assert!(wallet.allowance_log().is_empty());
}

#[test]
fn your_stakes_are_starkscans_candidates_the_chain_confirms() {
    let wallet = mainnet();
    let mut chain = Chain::new(&wallet.address);
    chain.member = Some(Member {
        amount: 5 * E18,
        unclaimed: 0,
        unpool: None,
    });
    let (core, id, prefs) = (&wallet.core, wallet.id.clone(), wallet.prefs.clone());
    let request = core
        .staking_positions_request(id.clone(), prefs.clone())
        .unwrap()
        .unwrap();
    let url = format!(
        "https://starkscan.co/v1/SN_MAIN/staking/address/{}?limit=1",
        wallet.address
    );
    assert_eq!(request.url, url);
    let views = |validators: &[&str]| {
        let answer = http(&positions(&wallet.address, validators));
        let read = core
            .staking_positions(id.clone(), answer, prefs.clone())
            .unwrap();
        run(&chain, || read.next_calls(), |r| read.accept(r));
        read.view(NOW_S).unwrap()
    };
    let view = views(&[STAKER]);
    assert!(view.complete);
    assert_eq!(view.rows.len(), 1);
    let figures = view.rows[0].figures.clone().unwrap();
    assert_eq!(
        (figures.staked.as_str(), figures.state),
        ("5", PositionState::Staked)
    );
    // A candidate the staking contract lists no pool for is not shown, and the list says so.
    let view = views(&[STAKER, "0x5a4"]);
    assert_eq!((view.complete, view.rows.len()), (false, 1));
    // An answer for another address is refused whole.
    let other = http(&positions(&canon(ACCOUNT_1), &[STAKER]));
    let refusal = core
        .staking_positions(id.clone(), other, prefs.clone())
        .err()
        .map(refused);
    assert_eq!(refusal.as_deref(), Some("Couldn't load your stakes."));
    // The person's own endpoint keeps the address from Starkscan: no request at all.
    core.set_rpc_override(NetworkId::Mainnet, "https://rpc.example.org".into())
        .unwrap();
    assert_eq!(core.staking_positions_request(id, prefs).unwrap(), None);
}

#[test]
fn the_directory_lists_validators_with_a_pool_for_a_tracked_token() {
    let request = staking_directory_request(NetworkId::Mainnet, Some("c/1".into()))
        .unwrap()
        .unwrap();
    assert_eq!(
        request.url,
        "https://starkscan.co/v1/SN_MAIN/staking/validators?limit=50&cursor=c%2F1"
    );
    let ratio = |n: &str, d: &str| json!({"denominatorRaw": d, "numeratorRaw": n});
    let validator = json!({"address": STAKER, "attestationDelay30d": null,
        "commission": ratio("500", "10000"), "delegatorCount": 12,
        "lastAttestationEpochRaw": "100", "liveness30d": ratio("995", "1000"),
        "liveness30dFirstEpoch": 1, "metricValueReason": null, "missedStreak": 0,
        "name": "Validator One", "operationalAddress": null,
        "pools": [{"address": POOL, "commission": ratio("1000", "10000"), "generationRaw": "1",
            "hhi": null, "metricValueReason": null, "status": "active", "tokenAddress": STRK,
            "top1Share": null, "top5Share": null}],
        "protocolSlug": null, "rewardAddress": null, "rewards": [], "stake": [],
        "status": "active", "validatorGenerationId": format!("sha256:{}", "a".repeat(64)),
        "website": "https://validator.example/about"});
    let page = json!({"chainId": "SN_MAIN", "coverage": coverage(), "items": [validator],
        "nextCursor": null, "source": "finalized_prepared_staking_snapshot"});
    let prefs = TokenPreferences::new(None);
    let view = staking_directory_view(NetworkId::Mainnet, http(&page), prefs.clone()).unwrap();
    let DirectoryView::Ready {
        entries,
        next_cursor,
    } = view
    else {
        panic!("{view:?}");
    };
    assert_eq!(next_cursor, None);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].staker, STAKER);
    assert_eq!(entries[0].pools[0].symbol, "STRK");
    let broken = HttpOutcome {
        status: 500,
        ..http(&page)
    };
    let refusal = staking_directory_view(NetworkId::Mainnet, broken, prefs)
        .err()
        .map(refused);
    assert_eq!(
        refusal.as_deref(),
        Some("Couldn't load the validator list. Paste a validator address instead.")
    );
}
