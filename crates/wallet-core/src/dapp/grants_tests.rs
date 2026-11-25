//! Ported from the extension's `packages/platform/vault/tests/service/origin-grants.test.ts`,
//! plus `grantOrigin`'s merge.

use super::*;

const ORIGIN: &str = "https://example.com";
const FIRST: &str = "SN_MAIN:0xa1";
const SECOND: &str = "SN_MAIN:0xb2";
const THIRD: &str = "SN_MAIN:0xc3";

fn grant(origin: &str, ids: &[&str]) -> OriginGrant {
    OriginGrant {
        account_ids: ids.iter().map(|id| (*id).to_owned()).collect(),
        origin: origin.to_owned(),
    }
}

// "grantedAccountIdsFor"
#[test]
fn a_site_sees_at_most_the_active_account() {
    let one = [grant(ORIGIN, &[FIRST])];
    // "returns nothing for an origin with no grant"
    assert_eq!(granted_account(&[], Some(FIRST), ORIGIN), None);
    assert_eq!(
        granted_account(&one, Some(FIRST), "https://other.example"),
        None
    );
    // "exposes a single-account grant only while that account is active"
    assert_eq!(granted_account(&one, Some(FIRST), ORIGIN), Some(FIRST));
    assert_eq!(granted_account(&one, Some(SECOND), ORIGIN), None);
    // "exposes only the active account from a multi-account grant"
    let all = [grant(ORIGIN, &[FIRST, SECOND, THIRD])];
    assert_eq!(granted_account(&all, Some(FIRST), ORIGIN), Some(FIRST));
    assert_eq!(granted_account(&all, Some(THIRD), ORIGIN), Some(THIRD));
    // "hides a grant whose accounts exclude the active account"
    let two = [grant(ORIGIN, &[FIRST, SECOND])];
    assert_eq!(granted_account(&two, Some(THIRD), ORIGIN), None);
    // "hides every grant when no account is selected"
    assert_eq!(granted_account(&two, None, ORIGIN), None);
}

// "connectedSitesFromVault"
#[test]
fn connected_sites_list_every_grant_in_code_unit_order() {
    let grants = [
        grant("https://zeta.example", &[FIRST, SECOND]),
        grant("https://alpha.example", &[FIRST]),
    ];
    assert_eq!(
        connected_sites(&grants),
        vec![
            grant("https://alpha.example", &[FIRST]),
            grant("https://zeta.example", &[FIRST, SECOND]),
        ]
    );
    // "orders by code unit and stays total on equal origins": "Z" before "a", ties in place.
    let grants = [
        grant("https://alpha.example", &[FIRST]),
        grant("https://alpha.example", &[SECOND]),
        grant("https://Zeta.example", &[FIRST]),
    ];
    assert_eq!(
        connected_sites(&grants),
        vec![
            grant("https://Zeta.example", &[FIRST]),
            grant("https://alpha.example", &[FIRST]),
            grant("https://alpha.example", &[SECOND]),
        ]
    );
}

// "withoutOriginAccess"
#[test]
fn revoking_access() {
    let grants = [
        grant(ORIGIN, &[FIRST, SECOND]),
        grant("https://other.example", &[FIRST]),
    ];
    // "removes one id and keeps the site while another remains"
    assert_eq!(
        without_origin_access(&grants, ORIGIN, Some(FIRST)),
        Some(vec![
            grant(ORIGIN, &[SECOND]),
            grant("https://other.example", &[FIRST])
        ])
    );
    // "drops the site with its last id"
    assert_eq!(
        without_origin_access(&grants, "https://other.example", Some(FIRST)),
        Some(vec![grant(ORIGIN, &[FIRST, SECOND])])
    );
    // "drops the whole site without an id"
    assert_eq!(
        without_origin_access(&grants, ORIGIN, None),
        Some(vec![grant("https://other.example", &[FIRST])])
    );
    // "returns the same list for an id or origin that is not granted": nothing to write.
    assert_eq!(without_origin_access(&grants, ORIGIN, Some(THIRD)), None);
    assert_eq!(
        without_origin_access(&grants, "https://other.example", Some(SECOND)),
        None
    );
    assert_eq!(
        without_origin_access(&grants, "https://absent.example", None),
        None
    );
    assert_eq!(
        without_origin_access(&grants, "https://absent.example", Some(FIRST)),
        None
    );
    // Only the first grant for an origin changes, as the extension's identity check has it.
    let doubled = [grant(ORIGIN, &[FIRST]), grant(ORIGIN, &[FIRST, SECOND])];
    assert_eq!(
        without_origin_access(&doubled, ORIGIN, Some(FIRST)),
        Some(vec![grant(ORIGIN, &[FIRST, SECOND])])
    );
}

// `grantOrigin`: the site's ids gain the account once, and its grant moves to the end.
#[test]
fn granting_merges_and_moves_the_grant_last() {
    let grants = [
        grant(ORIGIN, &[FIRST]),
        grant("https://other.example", &[FIRST]),
    ];
    assert_eq!(
        with_origin_grant(&grants, ORIGIN, SECOND),
        vec![
            grant("https://other.example", &[FIRST]),
            grant(ORIGIN, &[FIRST, SECOND])
        ]
    );
    assert_eq!(
        with_origin_grant(&grants, ORIGIN, FIRST),
        vec![
            grant("https://other.example", &[FIRST]),
            grant(ORIGIN, &[FIRST])
        ]
    );
    assert_eq!(
        with_origin_grant(&[], ORIGIN, FIRST),
        vec![grant(ORIGIN, &[FIRST])]
    );
}

// The vault's own spelling round-trips, and an unknown field is refused as the vault refuses it.
#[test]
fn the_record_is_the_vaults() {
    let text = r#"{"accountIds":["SN_MAIN:0xa1"],"origin":"https://example.com"}"#;
    let parsed: OriginGrant = serde_json::from_str(text).unwrap();
    assert_eq!(parsed, grant(ORIGIN, &[FIRST]));
    assert_eq!(serde_json::to_string(&parsed).unwrap(), text);
    assert!(serde_json::from_str::<OriginGrant>(
        r#"{"accountIds":[],"origin":"https://example.com","extra":1}"#
    )
    .is_err());
}
