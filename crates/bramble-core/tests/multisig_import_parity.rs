//! `extension-multisig-import.jsonl` (G7c): the extension's own import handlers, replayed through
//! the core's rules. `import` lines go through `IntentPackage::parse` and `import_intent`,
//! `signature` lines through `require_current` and `SigningIntent::with_signature` in
//! `import_multisig_signature`'s order, `replace` lines through `is_valid` and `can_replace`, and
//! `review` lines through `review_calls`, each with the wallet the `setup` line describes: the
//! Sepolia multisig, the vault's accounts and the saved contact as the trusted set, the
//! network's catalog tokens as the known ones.
//!
//! Cases that belong to the wallet (lock, active network, the message envelope) are
//! `WalletCore`'s, not these rules', and lines the extension marks `divergence` are where the
//! core answers differently on purpose; both are listed below and asserted as departures.

use crate::multisig_import_support::{parse, respond};
use crate::multisig_support::{class, summary_length};

use bramble_core::catalogs::networks;
use serde_json::{json, Value};
use starknet_wallet_core::accounts::multisig_read::MultisigAuthorityRead;
use starknet_wallet_core::accounts::multisig_records::{PublicConfig, Record};
use starknet_wallet_core::send::call_decode::Known;
use starknet_wallet_core::send::multisig_import::{
    import_intent, require_current, ImportContext, ImportError,
};
use starknet_wallet_core::send::multisig_intent::{IntentState, SigningIntent, StaleReason};
use starknet_wallet_core::send::multisig_invoke::{MultisigCall, MultisigSignature};
use starknet_wallet_core::send::multisig_review::{review_calls, CallReviewError};
use starknet_wallet_core::{felt, Felt, NetworkId};

/// Cases decided before the core's rules run: `WalletCore` refuses a locked wallet itself, and
/// mobile has no active-network gate (a multisig's network is its record's).
const WALLET_LEVEL: &[(&str, &str)] = &[
    ("a locked wallet", "wallet locked"),
    (
        "another active network",
        "switch to the multisig network first",
    ),
];

/// Where the core answers differently, with its answer: the extension's `divergence` lines, and
/// "another account class", where the core's authority read refuses a class it does not know
/// before any snapshot is compared, so the import is refused as unavailable, not stale.
const DIVERGENCES: &[(&str, &str)] = &[
    (
        "another account class",
        "Argent multisig config unavailable",
    ),
    (
        "a stored intent with another config snapshot",
        "Argent multisig intent conflict",
    ),
    (
        "an approve to a contact's lookalike",
        "The address this allows closely mimics an address you saved or paid but is a different \
         account. Re-check every character before approving.",
    ),
];

/// Where the extension shows what its read threw, the core has its fixed fallback sentence
/// (`ImportError::AuthorityUnavailable`): the host's error text never reaches it.
const THROWN_READ: &str = "RPC unavailable";

fn lines() -> Vec<Value> {
    let path = format!(
        "{}/tests/fixtures/extension-multisig-import.jsonl",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn block(name: &str) -> Vec<Value> {
    lines()
        .into_iter()
        .filter(|line| line["block"] == name)
        .collect()
}

fn of<T: serde::de::DeserializeOwned>(value: &Value) -> T {
    serde_json::from_value(value.clone()).unwrap()
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap()
}

struct Wallet {
    record: Record,
    trusted: Vec<Felt>,
}

/// The `setup` line: the multisig, and what the person trusts on Sepolia (the extension's
/// `trusted-recipients.ts`: the vault's accounts and the saved contact; nothing paid yet).
fn wallet() -> Wallet {
    let setup = &block("setup")[0];
    let mut trusted = vec![felt::parse(text(&setup["contact"])).unwrap()];
    for account in setup["vaultAccounts"].as_array().unwrap() {
        if account["networkId"] == "SN_SEPOLIA" {
            trusted.push(felt::parse(text(&account["address"])).unwrap());
        }
    }
    Wallet {
        record: of(&setup["record"]),
        trusted,
    }
}

fn known() -> Known<'static> {
    Known {
        tokens: &networks::info(NetworkId::Sepolia).tokens,
        nft_contracts: &[],
    }
}

/// The authority read the chain answered, driven to its end; a read that threw is left
/// unanswered, so it is never `Available`.
fn read(record: &Record, authority: &Value) -> MultisigAuthorityRead {
    let mut read =
        MultisigAuthorityRead::new(NetworkId::Sepolia, &record.address, &class()).unwrap();
    if authority.get("error").is_some() {
        return read;
    }
    respond(&mut read, json!("0x534e5f5345504f4c4941"));
    respond(&mut read, json!({"block_hash": "0x1", "block_number": 1}));
    respond(&mut read, authority["classHash"].clone());
    if read.next_request().is_none() {
        return read;
    }
    respond(
        &mut read,
        json!([format!("0x{:x}", authority["threshold"].as_u64().unwrap())]),
    );
    let guids = authority["signerGuids"].as_array().unwrap();
    let mut reply = vec![json!(format!("0x{:x}", guids.len()))];
    reply.extend(guids.iter().cloned());
    respond(&mut read, Value::Array(reply));
    read
}

fn nonce(case: &Value) -> Felt {
    felt::parse(text(&case["nonce"])).unwrap()
}

/// The answer the extension gave: the stored intent, or its error sentence.
fn expected(case: &Value) -> Result<Value, String> {
    match case["response"]["error"].as_str() {
        Some(error) => Err(error.to_owned()),
        None => Ok(case["response"]["intent"].clone()),
    }
}

fn departure(name: &str, table: &[(&str, &str)]) -> Option<String> {
    table
        .iter()
        .find(|(case, _)| *case == name)
        .map(|(_, answer)| (*answer).to_owned())
}

fn check(name: &str, case: &Value, got: Result<SigningIntent, String>) {
    let want = expected(case);
    if let Some(answer) = departure(name, DIVERGENCES) {
        assert_ne!(
            Err(answer.clone()),
            expected(case),
            "{name}: no longer a divergence"
        );
        assert_eq!(got.err(), Some(answer), "{name}");
        return;
    }
    match (got, want) {
        (Ok(intent), Ok(want)) => {
            assert_eq!(serde_json::to_value(&intent).unwrap(), want, "{name}");
            assert_eq!(case["storedAfter"], want, "{name}");
            let package = &case["response"]["intentPackage"];
            let sent: Value = serde_json::from_str(&intent.package().to_json()).unwrap();
            assert_eq!(&sent, package, "{name}");
        }
        (Err(error), Err(want)) if want == THROWN_READ => {
            assert_eq!(error, ImportError::AuthorityUnavailable.message(), "{name}");
        }
        (got, want) => assert_eq!(got.map(|_| ()), want.map(|_| ()), "{name}"),
    }
}

/// The handler's envelope: exactly `type` and `package`, then the package's own checks.
fn import_case(wallet: &Wallet, case: &Value) -> Result<SigningIntent, String> {
    let invalid = || "invalid Argent multisig intent package".to_owned();
    let message = case["message"].as_object().unwrap();
    if message.len() != 2 || !message.contains_key("package") {
        return Err(invalid());
    }
    let package = parse(&message["package"].to_string()).ok_or_else(invalid)?;
    let record = match text(&case["wallet"]["record"]) {
        "none" => return Err(ImportError::UnknownAccount.message().into()),
        "watched" => Record {
            current_config: None,
            deployment: None,
            ..wallet.record.clone()
        },
        _ => wallet.record.clone(),
    };
    let stored: Option<SigningIntent> = if case["wallet"]["storeAnswersAnotherTransaction"] == true
    {
        // The extension's store answers with another transaction under the id; any other
        // frozen transaction will do: the signer change's.
        let other = block("import")
            .into_iter()
            .find(|line| line["name"] == "a signer change")
            .unwrap();
        let other: SigningIntent = of(&other["response"]["intent"]);
        Some(SigningIntent {
            id: package.intent.id.clone(),
            ..other
        })
    } else {
        (!case["stored"].is_null()).then(|| of(&case["stored"]))
    };
    let known = known();
    let context = ImportContext {
        record: &record,
        stored: stored.as_ref(),
        class: &class(),
        known: &known,
        trusted: &wallet.trusted,
        summary_length: summary_length(),
    };
    let read = read(&record, &case["authority"]);
    import_intent(&package, &read, &nonce(case), &context).map_err(|error| error.message().into())
}

#[test]
fn imports_answer_as_the_extension_does() {
    let wallet = wallet();
    let cases = block("import");
    assert_eq!(cases.len(), 39);
    for case in &cases {
        let name = text(&case["name"]);
        if let Some(answer) = departure(name, WALLET_LEVEL) {
            assert_eq!(expected(case), Err(answer), "{name}");
            continue;
        }
        check(name, case, import_case(&wallet, case));
    }
}

/// `import_multisig_signature`'s order: the message, the stored intent, collecting, the chain
/// (`require_chain`, which marks the intent stale on a config or nonce refusal), then the
/// signature itself. Returns the answer and what the store holds after it.
fn signature_case(wallet: &Wallet, case: &Value) -> (Result<SigningIntent, String>, SigningIntent) {
    let stored: SigningIntent = of(&case["stored"]);
    let refuse = |error: &str| (Err(error.to_owned()), stored.clone());
    let message = case["message"].as_object().unwrap();
    let signature = message["signature"].as_object().unwrap();
    let canonical = |key: &str| {
        signature
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|value| felt::parse_canonical_non_zero(value).is_some())
    };
    if message.len() != 3 || !["publicKey", "r", "s"].iter().all(|key| canonical(key)) {
        return refuse("invalid Argent multisig signature import");
    }
    if stored.id != text(&message["intentId"]) {
        return refuse("intent not found");
    }
    if stored.state != IntentState::Collecting {
        return refuse("Argent multisig intent is not collecting signatures");
    }
    let read = read(&wallet.record, &case["authority"]);
    if let Err(error) = require_current(&stored, &wallet.record, &read, &nonce(case), &class()) {
        let after = match error {
            ImportError::ConfigChanged => stored.marked_stale(StaleReason::Config),
            ImportError::NonceNotCurrent => stored.marked_stale(StaleReason::Nonce),
            _ => stored.clone(),
        };
        return (Err(error.message().into()), after);
    }
    match stored.with_signature(of::<MultisigSignature>(&message["signature"])) {
        Ok(next) => (Ok(next.clone()), next),
        Err(error) => refuse(error.message()),
    }
}

#[test]
fn signature_imports_answer_as_the_extension_does() {
    let wallet = wallet();
    let cases = block("signature");
    assert_eq!(cases.len(), 15);
    for case in &cases {
        let name = text(&case["name"]);
        if let Some(answer) = departure(name, WALLET_LEVEL) {
            assert_eq!(expected(case), Err(answer), "{name}");
            continue;
        }
        let (got, after) = signature_case(&wallet, case);
        assert_eq!(
            serde_json::to_value(&after).unwrap(),
            case["storedAfter"],
            "{name}"
        );
        check(name, case, got);
    }
}

#[test]
fn replacements_answer_as_the_extension_does() {
    let cases = block("replace");
    assert_eq!(cases.len(), 14);
    let (class, length) = (class(), summary_length());
    for case in &cases {
        let name = text(&case["name"]);
        let current: SigningIntent = serde_json::from_str(text(&case["current"])).unwrap();
        let next: Option<SigningIntent> = serde_json::from_str(text(&case["next"])).ok();
        let next_valid = next
            .as_ref()
            .is_some_and(|next| next.is_valid(&class, length));
        assert_eq!(
            current.is_valid(&class, length),
            case["currentValid"],
            "{name}"
        );
        assert_eq!(next_valid, case["nextValid"], "{name}");
        let can = next_valid && next.is_some_and(|next| current.can_replace(&next, &class, length));
        assert_eq!(can, case["canReplace"], "{name}");
    }
}

#[test]
fn call_reviews_answer_as_the_extension_does() {
    let wallet = wallet();
    let known = known();
    let cases = block("review");
    assert_eq!(cases.len(), 24);
    for case in &cases {
        let name = text(&case["name"]);
        let calls: Vec<MultisigCall> = of(&case["calls"]);
        let current: Option<PublicConfig> =
            (!case["currentConfig"].is_null()).then(|| of(&case["currentConfig"]));
        let got = review_calls(
            &wallet.record.address,
            &calls,
            current.as_ref(),
            &class(),
            &known,
            &wallet.trusted,
            summary_length(),
        );
        if let Some(answer) = departure(name, DIVERGENCES) {
            assert!(
                case["divergence"].is_string(),
                "{name}: no longer a divergence"
            );
            assert_eq!(got, Err(CallReviewError::PoisonedSpender), "{name}");
            assert_eq!(CallReviewError::PoisonedSpender.message(), answer, "{name}");
            continue;
        }
        let refusal = case["error"].as_str().or(case["thrown"].as_str());
        match (got, refusal) {
            (Ok(review), None) => {
                assert_eq!(review.summary, text(&case["summary"]), "{name}");
                if !case["config"].is_null() {
                    assert_eq!(
                        serde_json::to_value(&review).unwrap()["config"],
                        case["config"],
                        "{name}"
                    );
                }
            }
            (Err(error), Some(refusal)) => assert_eq!(error.message(), refusal, "{name}"),
            (got, refusal) => panic!("{name}: core {got:?}, extension {refusal:?}"),
        }
    }
}
