//! Coordination sync flows over the helpers in `ffi_nostr_multisig_tests.rs`: the trust judgement
//! (including a saved contact forging a snapshot), the STRK20 wraps a sync leaves for later, and
//! the checkpoint rules (`docs/nostr-multisig.md` §4, §5). Imports over real packages are
//! `tests/multisig_sync_import.rs`.

use super::*;

struct Coordination {
    _dirs: Vec<tempfile::TempDir>,
    ours: Arc<WalletCore>,
    cosigner: Arc<WalletCore>,
    stranger: Arc<WalletCore>,
    record: MultisigRecord,
}

/// Our 2-of-2 setup with a saved co-signer, and a stranger saved as an ordinary payee whose card
/// carries a Nostr key.
fn coordination() -> Coordination {
    let (dir_a, ours) = wallet(PHRASE);
    let (dir_b, cosigner) = fresh_wallet();
    let (dir_c, stranger) = fresh_wallet();
    let config = ArgentMultisigConfig {
        threshold: 2,
        signer_public_keys: vec![key(&ours, 0), key(&cosigner, 0)],
    };
    let record = ours
        .save_multisig_setup(
            NetworkId::Sepolia,
            None,
            config,
            "0x0".into(),
            account_id(&ours, 0),
        )
        .unwrap();
    for (label, wallet) in [("Bob", &cosigner), ("Payee", &stranger)] {
        let account = &wallet.accounts(NetworkId::Sepolia).unwrap()[0];
        ours.save_contact(
            NetworkId::Sepolia,
            label.into(),
            account.address.clone(),
            Some(account.public_key.clone()),
            Some(wallet.nostr_public_key().unwrap()),
        )
        .unwrap();
    }
    Coordination {
        _dirs: vec![dir_a, dir_b, dir_c],
        ours,
        cosigner,
        stranger,
        record,
    }
}

fn package(record_id: &str, guids: Vec<String>, threshold: u32) -> String {
    json!({"intent":{"accountId":record_id,"configSnapshot":{"classHash":argent_multisig::class_hash(),"signerGuids":guids,"threshold":threshold},"id":"x"},
           "kind":"bramble.argent-multisig-intent","version":1})
    .to_string()
}

fn wrap_from(sender: &WalletCore, recipient: &WalletCore, content: &str) -> Value {
    wrap_as(sender, recipient, "bramble.multisig", content, NOW)
}

fn wrap_as(
    sender: &WalletCore,
    recipient: &WalletCore,
    identifier: &str,
    content: &str,
    at: u64,
) -> Value {
    let to = recipient.nostr_public_key().unwrap();
    let wrapped = sender
        .with_phrase(|m| nostr::wrap(m, 0, &to, identifier, content, at))
        .unwrap();
    serde_json::from_str(&wrapped).unwrap()
}

/// A sync with nothing to import: the relays return `events` and the open step ends it.
fn sync(ours: &WalletCore, record_id: &str, now: u64, events: &[Value]) -> CoordinationSyncOutcome {
    let plan = ours.coordination_sync_begin(record_id.into(), now).unwrap();
    for event in events {
        plan.accept_frame(0, json!(["EVENT", "mcw-1", event]).to_string());
    }
    plan.accept_frame(0, json!(["EOSE", "mcw-1"]).to_string());
    plan.relay_ended(1);
    match ours
        .coordination_sync_open(plan, TokenPreferences::new(None))
        .unwrap()
    {
        CoordinationSyncStep::Done { outcome } => outcome,
        CoordinationSyncStep::Read { .. } => panic!("nothing here should be imported"),
    }
}

fn coordination_file(core: &WalletCore) -> coordination::File {
    coordination::load(core.data_dir()).unwrap()
}

#[test]
fn a_saved_contact_forging_a_snapshot_is_authenticated_but_untrusted() {
    let c = coordination();
    let own_guid = argent_multisig::signer_guid_hex(&key(&c.stranger, 0)).unwrap();
    let forged = wrap_from(
        &c.stranger,
        &c.ours,
        &package(&c.record.id, vec![own_guid], 1),
    );
    let outcome = sync(&c.ours, &c.record.id, NOW, std::slice::from_ref(&forged));
    assert_eq!((outcome.received, outcome.imported), (1, 0));
    let file = coordination_file(&c.ours);
    assert!(
        file.checkpoints[0]
            .event_ids
            .contains(&forged["id"].as_str().unwrap().to_string()),
        "refused for good, so marked seen"
    );
}

#[test]
fn a_finished_wrap_is_receipted_and_the_next_sync_skips_it() {
    let c = coordination();
    let authority = c.record.deployment.as_ref().unwrap().signer_guids.clone();
    // Not a valid intent (no frozen transaction), so refused for good before any read.
    let invalid = wrap_from(&c.cosigner, &c.ours, &package(&c.record.id, authority, 2));
    let outcome = sync(&c.ours, &c.record.id, NOW, std::slice::from_ref(&invalid));
    assert_eq!((outcome.received, outcome.imported), (1, 0));
    let file = coordination_file(&c.ours);
    assert_eq!(file.checkpoints[0].synced_at, NOW);
    assert_eq!(
        file.checkpoints[0].event_ids,
        [invalid["id"].as_str().unwrap()]
    );

    // The next sync starts three days behind the new checkpoint and skips what it has seen.
    let plan = c
        .ours
        .coordination_sync_begin(c.record.id.clone(), NOW + 60)
        .unwrap();
    let request: Value = serde_json::from_str(&plan.open_frame(0)).unwrap();
    assert_eq!(request[2]["since"], json!(NOW - 3 * 24 * 60 * 60));
    let again = sync(&c.ours, &c.record.id, NOW + 60, &[invalid]);
    assert_eq!(again.received, 0);
    assert_eq!(
        coordination_file(&c.ours).checkpoints[0].synced_at,
        NOW + 60
    );
}

/// Multisig STRK20 is not ported, so a key or an authorization from a saved contact is left for a
/// later sync, as the extension leaves one it cannot accept; it holds `syncedAt` three days past
/// it, until it is older than the first-sync lookback. One from anyone else is finished.
#[test]
fn a_strk20_wrap_from_a_saved_contact_is_left_for_later() {
    const DAY: u64 = 24 * 60 * 60;
    let c = coordination();
    let (_dir, unknown) = fresh_wallet();
    let mut later = Vec::new();
    let mut finished = Vec::new();
    for identifier in [
        coordination::MULTISIG_STRK20_KEY_IDENTIFIER,
        coordination::MULTISIG_STRK20_AUTHORIZATION_IDENTIFIER,
    ] {
        later.push(wrap_as(
            &c.cosigner,
            &c.ours,
            identifier,
            "{}",
            NOW - 5 * DAY,
        ));
        finished.push(wrap_as(&unknown, &c.ours, identifier, "{}", NOW));
    }
    let events: Vec<Value> = later.iter().chain(&finished).cloned().collect();
    let outcome = sync(&c.ours, &c.record.id, NOW, &events);
    assert_eq!((outcome.received, outcome.imported), (2, 0));
    let checkpoint = coordination_file(&c.ours).checkpoints[0].clone();
    let oldest = later
        .iter()
        .map(|event| event["created_at"].as_u64().unwrap())
        .min()
        .unwrap();
    assert_eq!(checkpoint.synced_at, oldest + 3 * DAY);
    for event in &later {
        let id = event["id"].as_str().unwrap().to_string();
        assert!(!checkpoint.event_ids.contains(&id), "left for later");
    }

    // Past the first-sync lookback the same wraps are finished too.
    let late = NOW + 31 * DAY;
    let outcome = sync(&c.ours, &c.record.id, late, &later);
    assert_eq!(outcome.received, 2);
    assert_eq!(coordination_file(&c.ours).checkpoints[0].synced_at, late);
}

#[test]
fn a_rejoin_replaces_current_config_with_the_fresh_resolved_authority() {
    let (dir, core) = wallet(PHRASE);
    core.add_account(NetworkId::Sepolia).unwrap();
    let guids: Vec<String> = [key(&core, 0), key(&core, 1)]
        .iter()
        .map(|k| argent_multisig::signer_guid_hex(k).unwrap())
        .collect();
    core.add_multisig(available_read("0x123", 2, &guids))
        .unwrap();
    // Rotation: signer 1 removed, threshold lowered. The stored authority must follow.
    let rotated = core
        .add_multisig(available_read("0x123", 1, &guids[..1]))
        .unwrap();
    assert_eq!((rotated.threshold, rotated.signer_count), (Some(1), 1));
    assert_eq!(stored_record(&dir)["currentConfig"]["threshold"], json!(1));
    // An unresolved read keeps the stored config, as the extension's addRecord does.
    let unknown = argent_multisig::signer_guid_hex("0x777").unwrap();
    core.add_multisig(available_read("0x123", 2, &[guids[0].clone(), unknown]))
        .unwrap();
    let kept = &stored_record(&dir)["currentConfig"];
    assert_eq!(kept["threshold"], json!(1));
    assert_eq!(kept["signerPublicKeys"], json!([key(&core, 0)]));
}

#[test]
fn deleting_the_wallet_removes_the_metadata_files() {
    let c = coordination();
    let dir = c.ours.data_dir().to_path_buf();
    c.ours
        .set_nostr_relay_url("wss://relay.team.example".into())
        .unwrap();
    assert!(c.ours.relay_policy().unwrap().is_custom);
    assert_eq!(
        c.ours.relay_policy().unwrap().relay_urls,
        ["wss://relay.team.example"]
    );
    for file in ["contacts.json", "multisigs.json", "coordination.json"] {
        assert!(dir.join(file).exists(), "{file}");
    }
    c.ours.delete_wallet(PASSWORD.into()).unwrap();
    for file in ["contacts.json", "multisigs.json", "coordination.json"] {
        assert!(!dir.join(file).exists(), "{file}");
    }
}

#[test]
fn syncing_stops_when_the_wallet_locks() {
    let c = coordination();
    let authority = c.record.deployment.as_ref().unwrap().signer_guids.clone();
    let genuine = wrap_from(&c.cosigner, &c.ours, &package(&c.record.id, authority, 2));
    let plan = c
        .ours
        .coordination_sync_begin(c.record.id.clone(), NOW)
        .unwrap();
    plan.accept_frame(0, json!(["EVENT", "mcw-1", genuine]).to_string());
    plan.relay_ended(0);
    plan.relay_ended(1);
    c.ours.lock();
    assert!(matches!(
        c.ours
            .coordination_sync_open(plan, TokenPreferences::new(None)),
        Err(CoreError::Locked)
    ));
}
