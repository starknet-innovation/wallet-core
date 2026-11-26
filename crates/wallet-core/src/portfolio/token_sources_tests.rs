//! Ported from the extension's `settings/tests/preferences-dapp-upsert-keeps-user.test.ts` and
//! `transactions/tests/routing/verification-context.test.ts` (audit 171).

use super::*;
use crate::catalog::testing::{self, USDC};
use crate::catalog::Catalogs;
use crate::js::felt::same_felt;
use crate::portfolio::TEST_POLICY;

const SITE_TOKEN: &str = "0x5678";

fn token(address: &str, symbol: &str, decimals: u8) -> TokenConfig {
    with_token_id(NetworkId::Mainnet, address, symbol, "Some Token", decimals)
}

fn addresses(tokens: Vec<TokenConfig>) -> Vec<String> {
    tokens.into_iter().map(|token| token.address).collect()
}

fn trusts(prefs: &TokenPreferences<'_>, address: &str) -> bool {
    prefs
        .trusted_token_addresses(NetworkId::Mainnet)
        .iter()
        .any(|trusted| same_felt(trusted, address))
}

#[test]
fn a_sites_token_is_tracked_but_not_trusted_until_the_person_adds_it() {
    let (networks, registry) = (testing::networks(), testing::registry());
    let catalogs = Catalogs {
        networks: &networks,
        registry: &registry,
    };
    let mut prefs = TokenPreferences::new(None, catalogs, TEST_POLICY);
    let before = addresses(prefs.trusted(NetworkId::Mainnet));

    let added = prefs.add_from_dapp(token(SITE_TOKEN, "ZZT", 18)).unwrap();
    assert!(added.is_some_and(|outcome| !outcome.hidden_as_spam));
    assert!(prefs.is_tracked(NetworkId::Mainnet, "0x0000005678"));
    assert_eq!(addresses(prefs.trusted(NetworkId::Mainnet)), before);
    // The same address again, from a site, under another symbol: left as it is.
    assert_eq!(prefs.add_from_dapp(token(SITE_TOKEN, "ZZZ", 18)), Ok(None));
    assert_eq!(prefs.tracked(NetworkId::Mainnet).len(), before.len() + 1);

    // Survives a restart, and the field is written only once a site has added a token.
    let doc = prefs.persisted().unwrap();
    assert!(doc.contains(r#""dappTokenIds":["#), "{doc}");
    let mut prefs = TokenPreferences::new(Some(doc), catalogs, TEST_POLICY);
    assert_eq!(addresses(prefs.trusted(NetworkId::Mainnet)), before);

    // The person's own add replaces the site's record (one per address) and trusts it.
    prefs.add(token(SITE_TOKEN, "ZZT", 18)).unwrap();
    assert_eq!(prefs.tracked(NetworkId::Mainnet).len(), before.len() + 1);
    assert!(trusts(&prefs, SITE_TOKEN));
    assert!(!prefs.persisted().unwrap().contains("dappTokenIds"));
    // And a site can no longer replace it.
    assert_eq!(prefs.add_from_dapp(token(SITE_TOKEN, "ZZT", 18)), Ok(None));
    assert!(trusts(&prefs, SITE_TOKEN));
}

#[test]
fn a_site_adding_exactly_a_curated_token_is_trusted_and_a_lookalike_is_hidden() {
    let (networks, registry) = (testing::networks(), testing::registry());
    let catalogs = Catalogs {
        networks: &networks,
        registry: &registry,
    };
    let mut prefs = TokenPreferences::new(None, catalogs, TEST_POLICY);
    prefs.add_from_dapp(token(USDC, "USDC", 6)).unwrap();
    assert!(trusts(&prefs, USDC));
    // A curated ticker at another address is spam: stored hidden, never trusted.
    let fake = prefs
        .add_from_dapp(token("0x9999", "USDC", 6))
        .unwrap()
        .unwrap();
    assert!(fake.hidden_as_spam);
    assert!(!trusts(&prefs, "0x9999"));
}

#[test]
fn the_persons_add_keeps_one_record_per_address_and_its_hidden_flag() {
    let (networks, registry) = (testing::networks(), testing::registry());
    let catalogs = Catalogs {
        networks: &networks,
        registry: &registry,
    };
    let mut prefs = TokenPreferences::new(None, catalogs, TEST_POLICY);
    let first = prefs.add(token(SITE_TOKEN, "OLD", 18)).unwrap().token;
    prefs.set_hidden(first.id.clone(), true).unwrap();
    let second = prefs.add(token(SITE_TOKEN, "NEW", 18)).unwrap().token;
    let tracked = prefs.tracked(NetworkId::Mainnet);
    let mine: Vec<_> = tracked
        .iter()
        .filter(|t| same_felt(&t.token.address, SITE_TOKEN))
        .collect();
    assert_eq!(mine.len(), 1);
    assert_eq!(mine[0].token.id, second.id);
    assert!(mine[0].hidden_from_home);
}

#[test]
fn added_tokens_stop_at_the_policys_cap_whoever_adds_them() {
    let (networks, registry) = (testing::networks(), testing::registry());
    let catalogs = Catalogs {
        networks: &networks,
        registry: &registry,
    };
    let mut prefs = TokenPreferences::new(None, catalogs, TEST_POLICY);
    // TEST_POLICY allows three.
    prefs.add(token("0x1001", "AAA", 18)).unwrap();
    prefs.add_from_dapp(token("0x1002", "BBB", 18)).unwrap();
    prefs.add(token("0x1003", "CCC", 18)).unwrap();
    assert!(prefs.add(token("0x1004", "DDD", 18)).is_err());
    assert!(prefs.add_from_dapp(token("0x1004", "DDD", 18)).is_err());
    // Replacing a tracked address is not one more.
    prefs.add(token("0x1002", "BBB", 18)).unwrap();
    assert_eq!(prefs.tracked(NetworkId::Mainnet).len(), 2 + 3);
}
