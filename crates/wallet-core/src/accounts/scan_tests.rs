use super::*;
use crate::accounts::{AccountFamily, ArgentLayout, ArgentShape};

const PHRASE: &str = "habit hope tip crystal because grunt nation idea electric witness alert like";
/// Literal classes, as A1's tests use them.
const OZ: Felt =
    Felt::from_hex_unchecked("0x1d1777db36cdd06dd62cfde77b1b6ae06412af95d57a13dc40ac77b8a702381");
const ARGENT_0_4: Felt =
    Felt::from_hex_unchecked("0x36078334509b514626504edc9fb252328d1a240e4e948bef8d0c08dff45927f");

const LIMITS: ScanLimits = ScanLimits {
    batch_size: 4,
    default_count: 20,
    max_index: 0x7fff_ffff,
    max_scanned_indexes: 200,
};

fn shape(family: AccountFamily, ladder: Ladder, class_hash: Felt) -> ScanShape {
    let oz = family == AccountFamily::OpenZeppelin;
    ScanShape {
        family,
        ladder,
        class_hash,
        argent_shape: (family == AccountFamily::Argent)
            .then_some(ArgentShape::Listed(ArgentLayout::SignerOwnerGuardianOption)),
        salt: if oz {
            SaltRule::Zero
        } else {
            SaltRule::PublicKey
        },
        creatable: oz,
    }
}

fn config(shapes: Vec<ScanShape>) -> ScanConfig {
    ScanConfig {
        network: NetworkId::Sepolia,
        shapes,
        limits: LIMITS,
        reserved_indexes: vec![0x4147_4e54],
        tokens: vec![Felt::from(0x7u8)],
        privacy_pool: None,
        authorizations: Vec::new(),
    }
}

#[test]
fn probe_judging_matches_the_extension() {
    let ok = |body: &str| judge_class_hash(body, 3);
    assert!(ok(r#"{"jsonrpc":"2.0","id":3,"result":"0x1a2b"}"#)
        .unwrap()
        .is_some());
    assert!(
        ok(r#"{"jsonrpc":"2.0","id":3,"error":{"code":20,"message":"x"}}"#)
            .unwrap()
            .is_none()
    );
    assert!(
        ok(r#"{"jsonrpc":"2.0","id":3,"error":{"code":-1,"message":"Contract NOT found"}}"#)
            .unwrap()
            .is_none()
    );
    assert!(
        ok(r#"{"jsonrpc":"2.0","id":3,"error":{"code":24,"message":"Block not found"}}"#).is_err()
    );
    assert!(ok(r#"{"jsonrpc":"2.0","id":4,"result":"0x1"}"#).is_err());
    assert!(ok(r#"{"jsonrpc":"2.0","id":3,"result":"0xZZ"}"#).is_err());
    assert!(ok("not json").is_err());
}

fn at(account_index: u32, deployed: bool, funded: bool) -> Found {
    Found {
        family: AccountFamily::OpenZeppelin,
        ladder: Ladder::Account,
        class_hash: OZ,
        account_index,
        address: Felt::from(u64::from(account_index) + 1),
        deployed,
        funded,
        restriction: None,
        privacy_elsewhere: false,
    }
}

// Mirrors `recoveryScanTarget` in wallet-model/tests/accounts/recovery.test.ts.
#[test]
fn scan_target_is_the_extensions_gap_limit() {
    let target = |found: &[Found]| scan_target(found, &LIMITS);
    assert_eq!(target(&[]), LIMITS.default_count);
    assert_eq!(target(&[at(19, true, false)]), 20 + LIMITS.default_count);
    assert_eq!(target(&[at(25, false, true)]), 26 + LIMITS.default_count);
    assert_eq!(target(&[at(17, false, false)]), LIMITS.default_count);
    assert_eq!(target(&[at(5000, true, false)]), LIMITS.max_scanned_indexes);
    assert_eq!(
        target(&[at(u32::MAX, true, false)]),
        LIMITS.max_scanned_indexes
    );
}

#[test]
fn the_keep_rule_lists_creatable_shapes_at_zero_or_by_hand() {
    assert!(keep(&at(0, false, false), true, false));
    assert!(!keep(&at(3, false, false), true, false));
    assert!(keep(&at(3, false, false), true, true));
    assert!(!keep(&at(0, false, false), false, true));
    assert!(keep(&at(7, false, true), false, false));
    assert!(keep(&at(7, true, false), false, false));
}

#[test]
fn reserved_indexes_are_never_usable() {
    let config = config(vec![shape(
        AccountFamily::OpenZeppelin,
        Ladder::Account,
        OZ,
    )]);
    let reserved = config.reserved_indexes[0];
    assert!(!config.usable(reserved));
    assert!(config.usable(reserved - 1));
    assert!(!config.usable(LIMITS.max_index + 1));
    assert!(LIMITS.max_scanned_indexes < reserved);
    let mut scan = Scan::new();
    for index in [reserved, LIMITS.max_index + 1] {
        assert!(matches!(
            scan.next(PHRASE, None, &config, Some(index)),
            Err(Error::InvalidInput { .. })
        ));
    }
}

/// Ready derives no key under a BIP-39 passphrase: a caller that scans a Ready shape with one is
/// refused before any request, never answered with a partial walk.
#[test]
fn a_ready_shape_under_a_passphrase_fails_closed() {
    let ready = shape(AccountFamily::Argent, Ladder::ArgentX, ARGENT_0_4);
    let oz = shape(AccountFamily::OpenZeppelin, Ladder::Account, OZ);
    let mut scan = Scan::new();
    let refused = scan.next(PHRASE, Some("p"), &config(vec![oz, ready]), None);
    assert!(matches!(refused, Err(Error::InvalidInput { .. })));
    assert!(scan
        .submit(PHRASE, Some("p"), &config(vec![oz]), &[])
        .is_err());
    let step = scan
        .next(PHRASE, Some("p"), &config(vec![oz]), None)
        .unwrap();
    assert_eq!(step.requests.len(), 4);
}

#[test]
fn limits_and_the_phrase_are_held_to_the_scan() {
    let oz = shape(AccountFamily::OpenZeppelin, Ladder::Account, OZ);
    let mut config = config(vec![oz]);
    let mut scan = Scan::new();
    config.limits.batch_size = 0;
    assert!(scan.next(PHRASE, None, &config, None).is_err());
    config.limits.batch_size = u32::try_from(MAX_CANDIDATES).unwrap() + 1;
    assert!(scan.next(PHRASE, None, &config, None).is_err());
    config.limits.batch_size = 4;
    let step = scan.next(PHRASE, None, &config, None).unwrap();
    assert_eq!((step.requests.len(), step.checked, step.target), (4, 0, 20));
    // A batch is judged only for the phrase and network it was started with.
    let other = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
    assert!(matches!(
        scan.submit(other, None, &config, &[]),
        Err(Error::InvalidInput { .. })
    ));
    config.network = NetworkId::Mainnet;
    assert!(scan.submit(PHRASE, None, &config, &[]).is_err());
}

/// A host submits only what it was asked to POST, so a batch with nothing to probe would never
/// finish: no shapes is refused, and an index the wallet never derives is passed over.
#[test]
fn a_batch_always_has_something_to_probe() {
    let oz = shape(AccountFamily::OpenZeppelin, Ladder::Account, OZ);
    let mut scan = Scan::new();
    assert!(matches!(
        scan.next(PHRASE, None, &config(vec![]), None),
        Err(Error::InvalidInput { .. })
    ));
    let mut reserved = config(vec![oz]);
    reserved.limits.batch_size = 1;
    reserved.reserved_indexes = vec![0, 1];
    let step = scan.next(PHRASE, None, &reserved, None).unwrap();
    assert_eq!(
        (step.requests.len(), step.checked, step.complete),
        (1, 2, false)
    );
    // Nothing above `max_index` is usable: the walk ends there instead of issuing empty batches.
    let mut short = config(vec![oz]);
    short.limits.max_index = 3;
    let mut scan = Scan::new();
    let step = scan.next(PHRASE, None, &short, None).unwrap();
    assert_eq!(step.requests.len(), 4);
    let none = r#"{"jsonrpc":"2.0","id":ID,"error":{"code":20,"message":"Contract not found"}}"#;
    let zero = r#"{"jsonrpc":"2.0","id":ID,"result":["0x0","0x0"]}"#;
    let answer = |step: &ScanStep, body: &str| -> Vec<Response> {
        step.requests
            .iter()
            .map(|request| Response {
                id: request.id,
                body: body.replace("ID", &request.id.to_string()),
            })
            .collect()
    };
    let step = scan
        .submit(PHRASE, None, &short, &answer(&step, none))
        .unwrap();
    let step = scan
        .submit(PHRASE, None, &short, &answer(&step, zero))
        .unwrap();
    assert_eq!(
        (step.requests.len(), step.checked, step.complete),
        (0, 4, false)
    );
    let step = scan.next(PHRASE, None, &short, None).unwrap();
    assert_eq!(
        (step.requests.len(), step.checked, step.complete),
        (0, 20, true)
    );
}
