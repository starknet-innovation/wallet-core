//! Shared by the account tests: Bramble's class table (`bramble_core::catalogs`, from
//! `accounts.json`) as its choices for `starknet_wallet_core::accounts`, and the scan fixture's
//! shapes as a `ScanConfig`.
//! `accounts_scan_parity.rs` and `accounts_scan_mechanism.rs` both build `scan_config`, which
//! reaches every helper here.

use bramble_core::catalogs::account_address;
use bramble_core::catalogs::accounts::{self, send_authorization};
use bramble_core::OZ_ACCOUNT_VERSION;
use serde_json::Value;
use starknet_wallet_core::accounts::scan::{SaltRule, ScanConfig, ScanLimits, ScanShape};
use starknet_wallet_core::accounts::{AccountFamily, ArgentShape, Ladder};
use starknet_wallet_core::send::plan::Authorization;
use starknet_wallet_core::{felt, oz_account_class_hash, Felt, NetworkId};

/// The published E2E phrase every account fixture is derived from.
pub const PHRASE: &str =
    "habit hope tip crystal because grunt nation idea electric witness alert like";

pub fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

pub fn felt_at(value: &Value) -> Felt {
    felt::parse(value.as_str().unwrap()).unwrap()
}

/// The layout Bramble's catalog (`constructorShape`) gives an Argent class; Xverse's build is the
/// one Krusty does not list.
pub fn argent_shape(class_hash: &Felt) -> ArgentShape {
    account_address::argent_shape(AccountFamily::Argent, class_hash)
        .unwrap()
        .expect("a class in the extension's Argent table")
}

/// The ladder a vector was derived on: the extension's kind, with Braavos's `starknet-bip44`
/// on its vendor's address field (`bip44Axes`).
pub fn ladder(kind: &str, family: AccountFamily) -> Ladder {
    let ladder = match (kind, family) {
        ("argentx-bip44", AccountFamily::Argent) => Ladder::ArgentX,
        ("starknet-bip44", AccountFamily::Braavos) => Ladder::Address,
        ("starknet-bip44", _) => Ladder::Account,
        other => panic!("unexpected derivation {other:?}"),
    };
    assert_eq!(ladder.kind(), kind);
    assert!(ladder.fits(family));
    ladder
}

/// Bramble's send authorizations, from the class table (`send_authorization`).
pub fn authorizations() -> Vec<Authorization> {
    vec![
        send_authorization(AccountFamily::Argent),
        send_authorization(AccountFamily::Braavos),
    ]
}

/// One fixture shape as a `ScanShape`: the class, ladder and layout it names, Bramble's salt
/// (zero for OpenZeppelin, the public key otherwise) and the fixture's `creatable`.
pub fn scan_shape(shape: &Value) -> ScanShape {
    let family = AccountFamily::from_wire(shape["accountContractType"].as_str().unwrap()).unwrap();
    let class_hash = felt_at(&shape["classHash"]);
    let oz = family == AccountFamily::OpenZeppelin;
    ScanShape {
        family,
        ladder: ladder(shape["derivation"].as_str().unwrap(), family),
        class_hash,
        argent_shape: (family == AccountFamily::Argent).then(|| argent_shape(&class_hash)),
        salt: if oz {
            SaltRule::Zero
        } else {
            SaltRule::PublicKey
        },
        creatable: shape["creatable"].as_bool().unwrap(),
    }
}

/// The fixture's `RECOVERY_SCAN_LIMITS`.
pub fn limits(scan: &Value) -> ScanLimits {
    let limit = |name: &str| u32::try_from(scan["limits"][name].as_u64().unwrap()).unwrap();
    ScanLimits {
        batch_size: limit("batchSize"),
        default_count: limit("defaultCount"),
        max_index: limit("maxIndex"),
        max_scanned_indexes: limit("maxScannedIndexes"),
    }
}

/// Bramble's scan on `network`, with the fixture's shapes in its order. The OpenZeppelin shape
/// must name the class Krusty publishes at `OZ_ACCOUNT_VERSION`.
pub fn scan_config(
    network: NetworkId,
    tokens: Vec<Felt>,
    privacy_pool: Option<Felt>,
) -> ScanConfig {
    let scan = fixture("accounts-parity-scan.json");
    let shapes: Vec<ScanShape> = scan["shapes"]
        .as_array()
        .unwrap()
        .iter()
        .map(scan_shape)
        .collect();
    let oz = felt::parse(&oz_account_class_hash(network, OZ_ACCOUNT_VERSION).unwrap()).unwrap();
    assert_eq!(shapes[0].class_hash, oz);
    ScanConfig {
        network,
        shapes,
        limits: limits(&scan),
        // strkd's agent index (`RESERVED_AGENT_ACCOUNT_INDEX`), never derived or scanned.
        reserved_indexes: vec![accounts::catalog().reserved_agent_index],
        tokens,
        privacy_pool,
        authorizations: authorizations(),
    }
}
