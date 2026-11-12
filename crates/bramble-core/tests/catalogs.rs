//! Bramble's bundled catalogs (`generated/`) are the files the fixtures were generated against:
//! each equals its copy under `tests/fixtures/` byte for byte, so the parity suites, which load
//! the catalogs through `bramble_core::catalogs`, test what ships.

#[test]
fn generated_catalogs_are_the_fixture_copies() {
    let dir = env!("CARGO_MANIFEST_DIR");
    for name in [
        "accounts.json",
        "multisig.json",
        "networks.json",
        "token-registry.json",
    ] {
        let generated = std::fs::read(format!("{dir}/generated/{name}")).unwrap();
        let fixture = std::fs::read(format!("{dir}/tests/fixtures/{name}")).unwrap();
        assert!(
            generated == fixture,
            "generated/{name} differs from its fixture"
        );
    }
}

/// Every loader parses and validates its file (each panics on a bad one).
#[test]
fn every_catalog_loads() {
    use bramble_core::catalogs::{accounts, multisig, networks};
    assert_eq!(networks::all().len(), 2);
    assert!(!networks::registry().tokens.is_empty());
    assert!(!accounts::catalog().classes.is_empty());
    assert_eq!(
        multisig::argent_multisig_class().class_hash(),
        multisig::get().argent_multisig.class_hash
    );
}
