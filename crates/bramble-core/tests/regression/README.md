# Regression fixtures

Files the mobile wallet's own code wrote, kept so a later core still reads them. They are not the
extension's testimony (that is `../fixtures/`, with its sha256 manifest); they pin what Bramble's
earlier mobile cores put on disk, and two files Rust writes for the apps and the extension to
read. Copied byte for byte from Bramble's `mobile/core/tests/fixtures/` at commit
`8d1f4b8ccbb347b021f9e8ffa8176b412d0bb3b5` (2.7); never edited. A file is replaced only by
rewriting it the way its row says, and the rewrite is reviewed as a change to stored bytes.

| File | What it pins | Written by | Read by |
| --- | --- | --- | --- |
| `vault-v1.json` | A vault from the original core (version 1, bare envelope, 310k iterations), two accounts | Mobile core at `660a538e`, once | `tests/migration.rs` |
| `vault-v2-container.json` | A version-2 DEK container with an account count and a biometric slot (wrap secret `[7; 32]`) | Mobile core at `2785e57f` (vault-security), once | `tests/migration.rs` |
| `vault-v2-records.json` | A version-2 records payload in a bare envelope: OZ, Argent 0.4.0 and Braavos 1.0.0 on Sepolia, a label and a group | Mobile core at `e94b8ef2` (accounts branch), once | `tests/migration.rs` |
| `vault-v3-container.json` | A version-3 records container with a biometric slot | Mobile core at `88666b49`, once | `tests/backup_pending.rs` |
| `migration-v1-to-v5.golden.json` | The exact file the v1 → v5 chain seals for a fixed input and fixed randomness | `src/migrations_tests.rs` (`sealing_a_migrated_vault_is_deterministic_given_its_randomness`); compared, never rewritten by the test | `src/migrations_tests.rs` |
| `rust-gift-wrap.json` | A gift wrap Rust sealed, which the extension must open | `BRAMBLE_WRITE_FIXTURES=1 cargo test -p bramble-core --features mobile,testing --test nostr_multisig_parity`; every run checks the committed one still opens | `tests/nostr_multisig_parity.rs`; Bramble's `tests/conformance/mobile-nostr-parity.test.ts` opens its own committed copy, refreshed by copying this file |
| `ui-coordination.json` | The co-signer cards, predicted 2-of-2 address and one gift wrap the iOS UI suite drives coordination with | `BRAMBLE_WRITE_FIXTURES=1 cargo test -p bramble-core --features mobile,testing --test ui_coordination_fixture`; every run checks it | `tests/ui_coordination_fixture.rs`; Bramble's iOS UI tests (`mobile/ios/project.yml`) |

| File | sha256 |
| --- | --- |
| `migration-v1-to-v5.golden.json` | `54d60ad57da642639cfc051a00e6e678668a94bcd7da1da7ffb884e64cca2cfb` |
| `rust-gift-wrap.json` | `23c73a7eeb297ce574bc3b6ccb3405d0999a97720866bdcba584418dbb7cb2a6` |
| `ui-coordination.json` | `709955770216b8a9f83eb4798749ab97211f22215b01d3a6595f3e9b92a0e769` |
| `vault-v1.json` | `56459a1a668d45ef81b4c7c994701246c78e0dcec0b9d3aec9798d100c4eb9bb` |
| `vault-v2-container.json` | `9cde1f35bb86dda9bef4189d78ee44c5622ba43c2e135ca97f76bc0b259ee552` |
| `vault-v2-records.json` | `1a0b24bf937798853187872002796bcf82cc25c3fc1f4441c9b87459822aab3a` |
| `vault-v3-container.json` | `90e50cc54361b57a47d9918d0fbf377e739399d092e84236b0e2c0dca9d59b11` |
