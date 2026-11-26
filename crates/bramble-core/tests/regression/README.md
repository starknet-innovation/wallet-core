# Regression fixtures

Files the mobile wallet's own code wrote. They are not the extension's testimony (that is
`../fixtures/`, with its sha256 manifest); they pin a vault an earlier mobile core put on disk,
which this core must refuse rather than misread, and two files Rust writes for the apps and the
extension to read. Copied byte for byte from Bramble's `mobile/core/tests/fixtures/` at commit
`8d1f4b8ccbb347b021f9e8ffa8176b412d0bb3b5` (2.7); never edited. A file is replaced only by
rewriting it the way its row says, and the rewrite is reviewed as a change to stored bytes.

| File | What it pins | Written by | Read by |
| --- | --- | --- | --- |
| `vault-v3-container.json` | A version-3 records container with a biometric slot (wrap secret `[7; 32]`): an earlier build's vault, refused once its password is proven, still deletable | Mobile core at `88666b49`, once | `tests/backup_pending.rs` |
| `rust-gift-wrap.json` | A gift wrap Rust sealed, which the extension must open | `BRAMBLE_WRITE_FIXTURES=1 cargo test -p bramble-core --features mobile,testing --test integration nostr_multisig_parity::`; every run checks the committed one still opens | `tests/nostr_multisig_parity.rs`; Bramble's `tests/conformance/mobile-nostr-parity.test.ts` opens its own committed copy, refreshed by copying this file |
| `ui-coordination.json` | The co-signer cards, predicted 2-of-2 address and one gift wrap the iOS UI suite drives coordination with | `BRAMBLE_WRITE_FIXTURES=1 cargo test -p bramble-core --features mobile,testing --test integration ui_coordination_fixture::`; every run checks it | `tests/ui_coordination_fixture.rs`; Bramble's iOS UI tests (`mobile/ios/project.yml`) |

| File | sha256 |
| --- | --- |
| `rust-gift-wrap.json` | `23c73a7eeb297ce574bc3b6ccb3405d0999a97720866bdcba584418dbb7cb2a6` |
| `ui-coordination.json` | `709955770216b8a9f83eb4798749ab97211f22215b01d3a6595f3e9b92a0e769` |
| `vault-v3-container.json` | `90e50cc54361b57a47d9918d0fbf377e739399d092e84236b0e2c0dca9d59b11` |
