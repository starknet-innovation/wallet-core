use super::*;
use crate::send::types::ArgentClass;

const KEY: Felt = Felt::from_hex_unchecked("0x77");
/// Literal classes: an Argent class whose authorization is the signer array, one that is not,
/// and a Braavos implementation.
const ARGENT_0_4: Felt = Felt::from_hex_unchecked("0x40");
const ARGENT_0_3: Felt = Felt::from_hex_unchecked("0x31");
const BRAAVOS_CURRENT: Felt = Felt::from_hex_unchecked("0xb0");
const OZ: Felt = Felt::from_hex_unchecked("0x02");

fn authorizations() -> Vec<Authorization> {
    let class = |class_hash, signer_array| ArgentClass {
        class_hash,
        label: String::new(),
        signer_array,
    };
    vec![
        Authorization::ArgentSignerArray {
            classes: vec![class(ARGENT_0_3, false), class(ARGENT_0_4, true)],
        },
        Authorization::Braavos {
            signing_classes: vec![BRAAVOS_CURRENT],
            implementation: BRAAVOS_CURRENT,
        },
    ]
}

fn gathered(family: AccountFamily, reads: &[(Read, Option<Vec<Felt>>)]) -> Gathered {
    let live_class = match family {
        AccountFamily::Argent => ARGENT_0_4,
        AccountFamily::Braavos => BRAAVOS_CURRENT,
        AccountFamily::OpenZeppelin => OZ,
    };
    Gathered {
        live_class: Some(live_class),
        reads: reads.iter().cloned().collect(),
    }
}

fn verdict(family: AccountFamily, gathered: &Gathered, block: Option<u64>) -> Option<Restriction> {
    restriction(family, gathered, &KEY, block, &authorizations())
}

fn argent(owner: Felt, owner_type: Felt, guardian: Felt) -> Option<Restriction> {
    let reads = [
        (Read::Owner, Some(vec![owner])),
        (Read::OwnerType, Some(vec![owner_type])),
        (Read::Guardian, Some(vec![guardian])),
    ];
    verdict(
        AccountFamily::Argent,
        &gathered(AccountFamily::Argent, &reads),
        None,
    )
}

#[test]
fn argent_verdicts_follow_the_extensions_order() {
    let one = Felt::ONE;
    assert_eq!(argent(KEY, Felt::ZERO, Felt::ZERO), None);
    assert_eq!(argent(one, one, one), Some(Restriction::OwnerChanged));
    assert_eq!(argent(KEY, one, one), Some(Restriction::OwnerType));
    assert_eq!(argent(KEY, Felt::ZERO, one), Some(Restriction::Guardian));
    // A read that did not answer is no verdict, never a guess.
    let partial = gathered(AccountFamily::Argent, &[(Read::Owner, Some(vec![one]))]);
    assert_eq!(verdict(AccountFamily::Argent, &partial, None), None);
    // A live class this wallet cannot authorize for needs no read.
    let mut old = gathered(AccountFamily::Argent, &[]);
    old.live_class = Some(Felt::from(5u8));
    assert_eq!(
        verdict(AccountFamily::Argent, &old, None),
        Some(Restriction::Release)
    );
    assert!(reads(
        AccountFamily::Argent,
        &Felt::from(5u8),
        &authorizations(),
        false
    )
    .is_empty());
    // A known Argent class whose authorization is not the signer array is a release too.
    old.live_class = Some(ARGENT_0_3);
    assert_eq!(
        verdict(AccountFamily::Argent, &old, None),
        Some(Restriction::Release)
    );
}

fn braavos(signers: Vec<Felt>, deferred: Option<Felt>, block: Option<u64>) -> Option<Restriction> {
    let reads = [
        (Read::Signers, Some(signers)),
        (Read::Deferred, Some(deferred.into_iter().collect())),
    ];
    verdict(
        AccountFamily::Braavos,
        &gathered(AccountFamily::Braavos, &reads),
        block,
    )
}

#[test]
fn the_block_is_read_only_for_a_requested_2fa_removal() {
    let needs = |signers: Vec<Felt>, deferred: Option<Felt>| {
        let reads = [
            (Read::Signers, Some(signers)),
            (Read::Deferred, Some(deferred.into_iter().collect())),
        ];
        needs_block(
            AccountFamily::Braavos,
            &gathered(AccountFamily::Braavos, &reads),
            &KEY,
            &authorizations(),
        )
    };
    let with_2fa = vec![Felt::ONE, KEY, Felt::ONE, Felt::from(9u8), Felt::ZERO];
    let expire = Some(Felt::from(100u8));
    assert!(needs(with_2fa.clone(), expire));
    assert!(!needs(with_2fa, None), "no removal requested");
    assert!(
        !needs(vec![Felt::ONE, KEY, Felt::ZERO, Felt::ZERO], expire),
        "no 2FA"
    );
    assert!(
        !needs(vec![Felt::ONE, Felt::TWO, Felt::ZERO, Felt::ZERO], expire),
        "not ours"
    );
}

#[test]
fn braavos_verdicts_follow_the_extensions_order() {
    let felts = |values: &[u64]| values.iter().map(|v| Felt::from(*v)).collect::<Vec<_>>();
    let stark_only = vec![Felt::ONE, KEY, Felt::ZERO, Felt::ZERO];
    let with_2fa = vec![Felt::ONE, KEY, Felt::ONE, Felt::from(9u8), Felt::ZERO];
    assert_eq!(braavos(stark_only, None, None), None);
    assert_eq!(
        braavos(felts(&[1, 5, 0, 0]), None, None),
        Some(Restriction::OwnerChanged)
    );
    assert_eq!(
        braavos(with_2fa.clone(), None, None),
        Some(Restriction::StrongSigner)
    );
    let expire = Some(Felt::from(100u8));
    assert_eq!(braavos(with_2fa.clone(), expire, Some(101)), None);
    assert_eq!(
        braavos(with_2fa.clone(), expire, Some(100)),
        Some(Restriction::StrongSigner)
    );
    assert_eq!(
        braavos(with_2fa, expire, None),
        None,
        "no block, no verdict"
    );
    assert_eq!(braavos(felts(&[3, 1]), None, None), None, "malformed list");
}

#[test]
fn the_pool_note_needs_a_foreign_non_zero_key() {
    let pool =
        |key: Option<Vec<Felt>>| gathered(AccountFamily::OpenZeppelin, &[(Read::PoolKey, key)]);
    assert!(registered_elsewhere(&pool(Some(vec![Felt::TWO])), || Some(
        KEY
    )));
    assert!(!registered_elsewhere(&pool(Some(vec![KEY])), || Some(KEY)));
    assert!(!registered_elsewhere(
        &pool(Some(vec![Felt::ZERO])),
        || panic!("not derived")
    ));
    assert!(!registered_elsewhere(&pool(None), || panic!("not derived")));
    assert!(!registered_elsewhere(&pool(Some(vec![Felt::TWO])), || None));
}

#[test]
fn the_own_viewing_key_is_the_public_half_of_the_scoped_key() {
    const PHRASE: &str =
        "habit hope tip crystal because grunt nation idea electric witness alert like";
    let pool = Felt::from(0x123u32);
    let own = own_viewing_public_key(PHRASE, None, Ladder::Account, 0, NetworkId::Sepolia, &pool)
        .unwrap();
    let key = kms::account_key(PHRASE, None, Ladder::Account, 0).unwrap();
    let viewing = kms::derive_strk20_viewing_key(&key, "0x534e5f5345504f4c4941", "0x123").unwrap();
    assert_eq!(
        felt::canonical(&own),
        kms::stark_public_key(&viewing).unwrap()
    );
    // Scoped: another network or pool is another key.
    let mainnet =
        own_viewing_public_key(PHRASE, None, Ladder::Account, 0, NetworkId::Mainnet, &pool);
    assert_ne!(mainnet.unwrap(), own);
    // A zero pool is not a scope, and a Ready key takes no passphrase.
    let zero = own_viewing_public_key(
        PHRASE,
        None,
        Ladder::Account,
        0,
        NetworkId::Sepolia,
        &Felt::ZERO,
    );
    assert!(zero.is_err());
    let ready = own_viewing_public_key(
        PHRASE,
        Some("p"),
        Ladder::ArgentX,
        0,
        NetworkId::Sepolia,
        &pool,
    );
    assert!(ready.is_err());
}
