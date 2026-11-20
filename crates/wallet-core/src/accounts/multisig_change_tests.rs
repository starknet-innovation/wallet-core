//! One case per refusal in `config-change.ts`, including the ones the fixture does not reach
//! (`isArgentMultisigCalls` is tested beside it in `send::multisig_invoke`). The fixture cases
//! are in `bramble-core`'s `tests/multisig_signers_parity.rs`.

use super::*;
use crate::accounts::multisig::tests::class;
use crate::send::multisig_invoke::MultisigCall;

const ACCOUNT: &str = "0xabc";

fn call(entrypoint: &str, calldata: &[&str]) -> MultisigCall {
    MultisigCall {
        calldata: calldata.iter().map(|v| v.to_string()).collect(),
        contract_address: ACCOUNT.into(),
        entrypoint: entrypoint.into(),
    }
}

fn parse(entrypoint: &str, calldata: &[&str]) -> Result<ConfigChange, ChangeError> {
    parse_config_change(ACCOUNT, &call(entrypoint, calldata), &class())
}

fn config(threshold: u32, keys: &[&str]) -> PublicConfig {
    PublicConfig {
        signer_public_keys: keys.iter().map(|k| k.to_string()).collect(),
        threshold,
    }
}

fn keys(keys: &[&str]) -> Vec<String> {
    keys.iter().map(|k| k.to_string()).collect()
}

#[test]
fn the_parser_refuses_every_malformed_self_call() {
    use ChangeError::*;
    let cases: [(&str, &[&str], ChangeError); 13] = [
        ("change_threshold", &[], InvalidCalldata),
        ("change_threshold", &["0x1", "0x0"], InvalidCalldata),
        ("change_threshold", &["0x0"], InvalidThreshold),
        ("change_threshold", &["0x21"], InvalidThreshold),
        (
            "change_threshold",
            &["0x10000000000000000"],
            InvalidThreshold,
        ),
        ("replace_signer", &["0x0", "0x1", "0x0"], InvalidCalldata),
        (
            "replace_signer",
            &["0x0", "0x1", "0x2", "0x3"],
            NonStarknetSigner,
        ),
        (
            "replace_signer",
            &["0x0", "0x0", "0x0", "0x3"],
            InvalidSignerPublicKey,
        ),
        ("add_signers", &["0x1"], InvalidCalldata),
        ("add_signers", &["0x1", "0x0"], InvalidSignerArray),
        ("add_signers", &["0x1", "0x21"], InvalidSignerArray),
        ("remove_signers", &["0x1", "0x1", "0x0"], InvalidSignerArray),
        ("upgrade", &["0x1"], UnsupportedSelfCall),
    ];
    for (entrypoint, calldata, error) in cases {
        assert_eq!(
            parse(entrypoint, calldata),
            Err(error),
            "{entrypoint} {calldata:?}"
        );
    }
    // The array is read before the threshold.
    assert_eq!(
        parse("add_signers", &["0x0", "0x1", "0x1", "0x2"]),
        Err(NonStarknetSigner)
    );
    assert_eq!(
        parse("add_signers", &["0x0", "0x1", "0x0", "0x2"]),
        Err(InvalidThreshold)
    );
    let other = MultisigCall {
        contract_address: "0xabd".into(),
        ..call("change_threshold", &["0x1"])
    };
    assert_eq!(
        parse_config_change(ACCOUNT, &other, &class()),
        Err(InvalidSelfCall)
    );
    assert_eq!(parse("Change", &["0x1"]), Err(InvalidSelfCall));
    // `sameFelt` compares values, so a padded account address still matches.
    let padded = parse_config_change("0x0abc", &call("change_threshold", &["0x2"]), &class());
    assert_eq!(padded, Ok(ConfigChange::ChangeThreshold { threshold: 2 }));
}

#[test]
fn the_builder_canonicalises_signers_and_parses_its_own_call() {
    let change = ConfigChange::AddSigners {
        signer_public_keys: keys(&["0x00AB", "205"]),
        threshold: 2,
    };
    let built = config_change_call(ACCOUNT, &change, &class()).unwrap();
    assert_eq!(built.calldata, ["0x2", "0x2", "0x0", "0xab", "0x0", "0xcd"]);
    for bad in ["0x0", "xyz", "-5"] {
        let change = ConfigChange::ReplaceSigner {
            add_signer_public_key: bad.into(),
            remove_signer_public_key: "0x1".into(),
        };
        assert_eq!(
            config_change_call(ACCOUNT, &change, &class()),
            Err(ChangeError::InvalidSignerPublicKey)
        );
    }
    let empty = ConfigChange::RemoveSigners {
        signer_public_keys: vec![],
        threshold: 1,
    };
    assert_eq!(
        config_change_call(ACCOUNT, &empty, &class()),
        Err(ChangeError::InvalidSignerArray)
    );
    let zero = ConfigChange::ChangeThreshold { threshold: 0 };
    assert_eq!(
        config_change_call(ACCOUNT, &zero, &class()),
        Err(ChangeError::InvalidThreshold)
    );
    let unpadded = config_change_call(
        "0x0abc",
        &ConfigChange::ChangeThreshold { threshold: 1 },
        &class(),
    );
    assert_eq!(unpadded, Err(ChangeError::InvalidSelfCall));
}

#[test]
fn transitions_refuse_no_ops_and_membership_errors() {
    use ChangeError::*;
    let current = config(1, &["0x1", "0x2"]);
    let next = |change: ConfigChange| next_public_config(&current, &change, &class());
    assert_eq!(
        next(ConfigChange::ChangeThreshold { threshold: 1 }),
        Err(ThresholdUnchanged)
    );
    assert_eq!(
        next(ConfigChange::ChangeThreshold { threshold: 3 }),
        Err(ThresholdOrSignerCount)
    );
    let add = |signers: &[&str], threshold| ConfigChange::AddSigners {
        signer_public_keys: keys(signers),
        threshold,
    };
    assert_eq!(next(add(&["0x2"], 1)), Err(AlreadyConfigured));
    assert_eq!(next(add(&["0x3", "0x3"], 1)), Err(SignersNotUnique));
    // Membership compares spellings, as the extension does; normalisation then catches it.
    assert_eq!(next(add(&["0x02"], 1)), Err(SignersNotUnique));
    assert_eq!(next(add(&["0x0"], 1)), Err(ConfigSignerPublicKey));
    let remove = |signers: &[&str], threshold| ConfigChange::RemoveSigners {
        signer_public_keys: keys(signers),
        threshold,
    };
    assert_eq!(next(remove(&["0x1", "0x1"], 1)), Err(RemovalNotUnique));
    assert_eq!(next(remove(&["0x3"], 1)), Err(NotConfigured));
    assert_eq!(
        next(remove(&["0x1", "0x2"], 1)),
        Err(ThresholdOrSignerCount)
    );
    let replace = |remove: &str, add: &str| ConfigChange::ReplaceSigner {
        add_signer_public_key: add.into(),
        remove_signer_public_key: remove.into(),
    };
    assert_eq!(next(replace("0x3", "0x4")), Err(NotConfigured));
    assert_eq!(next(replace("0x1", "0x2")), Err(AlreadyConfigured));
    let replaced = next(replace("0x1", "0x4")).unwrap();
    assert_eq!(replaced.threshold, 1);
    assert!(replaced.signer_public_keys.contains(&"0x4".to_string()));
    assert!(!replaced.signer_public_keys.contains(&"0x1".to_string()));
}

#[test]
fn the_current_configuration_is_normalised_first() {
    use ChangeError::*;
    let change = ConfigChange::ChangeThreshold { threshold: 1 };
    let next = |current: PublicConfig| next_public_config(&current, &change, &class());
    assert_eq!(next(config(0, &["0x1"])), Err(ThresholdOrSignerCount));
    assert_eq!(next(config(1, &[])), Err(ThresholdOrSignerCount));
    assert_eq!(next(config(2, &["0x1", "xyz"])), Err(ConfigSignerPublicKey));
    assert_eq!(next(config(2, &["0x1", "0x01"])), Err(SignersNotUnique));
    let many: Vec<String> = (1..=33).map(|n| format!("0x{n:x}")).collect();
    let refs: Vec<&str> = many.iter().map(String::as_str).collect();
    assert_eq!(next(config(2, &refs)), Err(ThresholdOrSignerCount));
    // Any `BigInt` spelling of a current key is canonicalised, then GUID-sorted.
    let sorted = next(config(2, &["0x02", "1"])).unwrap();
    assert_eq!(sorted.threshold, 1);
    let guid = |key: &String| felt::parse(&multisig::signer_guid_hex(key).unwrap()).unwrap();
    let mut expected = keys(&["0x1", "0x2"]);
    expected.sort_by_key(guid);
    assert_eq!(sorted.signer_public_keys, expected);
}

#[test]
fn summaries_follow_the_extension() {
    let review = review_config_change(
        ACCOUNT,
        &config(1, &["0x1", "0x2"]),
        &call("remove_signers", &["0x1", "0x1", "0x0", "0x2"]),
        &class(),
    )
    .unwrap();
    assert_eq!(
        review.summary,
        "Remove 1 Argent multisig signer(s) and set threshold to 1"
    );
    assert_eq!(review.next_config.signer_public_keys, ["0x1"]);
    assert_eq!(
        config_change_summary(&ConfigChange::ChangeThreshold { threshold: 7 }),
        "Change Argent multisig threshold to 7"
    );
    assert_eq!(
        ChangeError::ThresholdUnchanged.to_string(),
        "Argent multisig threshold is unchanged"
    );
}
