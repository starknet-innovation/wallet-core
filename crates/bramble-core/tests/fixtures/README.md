# Golden fixtures

Written by the Bramble extension's real code paths, never by hand. Bramble
(`starknet-innovation/mc-wallet`) is the source: `bun run mobile:parity` writes every file below into
`mobile/core/tests/fixtures/`, and syncing is copying them here unchanged. `tests/fixtures.rs` checks
this table against the directory: every file must be listed, and its sha256 must match.

All files: Bramble commit `0a4bd84c3ed838ffe56ce2a702652dd9a7c3bca0` (`feature/mobile-wallet`,
2026-11-06), privacy SDK `vendor/starknet-privacy` at `b40bf109` (Bramble's pin). Re-running
`mobile:parity` there leaves no diff. Generators are under `scripts/` in Bramble; each is reached
from `generate-mobile-parity-vectors.ts`.

| File | Generator | sha256 |
| --- | --- | --- |
| `extension-parity.json` | `generate-mobile-parity-vectors.ts` | `a897813e2d532d86ed483bb3e909870ec30013dfc95f9c131279a603dfb98ef1` |
| `accounts-parity-families-mainnet.json` | `mobile-parity-accounts.ts` | `ca50194ee4bb565e0d5ab6c4d61f50369efa79f046ff10b4d5c54b1198710594` |
| `accounts-parity-families-sepolia.json` | `mobile-parity-accounts.ts` | `6cfb3ecc0490ec06c8a53b50fc795b749a818e11d3410b06f1a8c8ba14514634` |
| `accounts-parity-recovery.json` | `mobile-parity-accounts.ts` | `15fa78d7a9fa08a17e82585f83b1d8d6282c1e6c5414673b1d2d345c1f1f2c85` |
| `accounts-parity-rules.json` | `mobile-parity-accounts.ts` | `48f09827f5fd82119b01e7f5f39b17af30a42e4684031b817b40422966d8dc14` |
| `nostr-multisig-parity.json` | `mobile-assets/write-nostr-multisig-parity.ts` | `445cbbcbb0104350709ac3cfc93a74a6dc5af2286de674e0512ceb020ed6357e` |
| `portfolio-parity.jsonl` | `mobile-parity-portfolio/` | `1cb005dc0c747b89f3ebdd354f8e71ad52de7c378338e191214d483514f06260` |
| `extension-privacy-copy.json` | `generate-mobile-privacy-vectors.ts` | `115f913227bf95dce1f749172077fecffa0462b17b9bd649815c1429875ddfd2` |
| `extension-privacy-rules.json` | `generate-mobile-privacy-vectors.ts` | `7fd6f242c6e55a9e14cebb596daa6052d9836cbbcebe5619b1a2d6824ccc8345` |
| `extension-privacy-wire.json` | `generate-mobile-privacy-vectors.ts` | `a74cc8d4a80a03309a6528436ff3d2962198860033ccca4d33b51d770bdfdc11` |
| `extension-transactions-argent-refusals.json` | `generate-mobile-transaction-vectors.ts` | `f2eb7492ca5425e618b9954b22fc666bf7a2a4645438ba8837a1c3f6b78d2997` |
| `extension-transactions-argent-upgraded.json` | `generate-mobile-transaction-vectors.ts` | `9d14e829c7c2dfccb940e150984036db18a528ddd65f134763371049981b8156` |
| `extension-transactions-argent.json` | `generate-mobile-transaction-vectors.ts` | `854103a684d841a7ed2949ac238283d17a934cd7cba9a9d29b522868959edb5f` |
| `extension-transactions-braavos-refusals.json` | `generate-mobile-transaction-vectors.ts` | `5c15ab334651870307d8259a6c75865f8a5e555fa53b2bf50d468879eeefa102` |
| `extension-transactions-braavos.json` | `generate-mobile-transaction-vectors.ts` | `fe2a1bdaf2420700b70396b7214fcb706f2f6aab88fa46ee6fa17f1ed3fc9f4f` |
| `extension-transactions-copy.json` | `generate-mobile-transaction-vectors.ts` | `fbb47bf42852b708f89ce9cf4be1fad0a6926fb571ad21817e3a536957849292` |
| `extension-transactions-observation.json` | `generate-mobile-transaction-vectors.ts` | `bc02c237f67d1ac646a86df25305f4be0181a8f3e44d19cf5c748bc898c01541` |
| `extension-transactions-presentation.json` | `generate-mobile-transaction-vectors.ts` | `fcf33dbdc1d1273042910feae40525a10a73dca38f59808e8269ea4e61a4a4c5` |
| `extension-transactions-rules.json` | `generate-mobile-transaction-vectors.ts` | `65673321490171d26684a2e133c23e821d8ac185227f220e21ca383e719af825` |
| `extension-transactions-status.json` | `generate-mobile-transaction-vectors.ts` | `8e36ccc8e9a9752b5d2d704a43fbcb4325cf5a1b6d1bbfc01042a6f6277db671` |
| `extension-transactions-verification.json` | `generate-mobile-transaction-vectors.ts` | `44253f028a3caaf4ea896b6c09afec5c3b512cce729ccabe43e4422f712385be` |
| `extension-transactions-wire.json` | `generate-mobile-transaction-vectors.ts` | `a4e8085ad6b5e14f05c66f291b8198a167cae729e639a812c7af908d412d8e3d` |

Read today: `extension-parity.json` (`parity.rs`, `vault_parity.rs`),
`accounts-parity-families-sepolia.json` and `nostr-multisig-parity.json` (`parity.rs`), both
`accounts-parity-families-*.json` (`accounts_parity.rs`), and every
`extension-transactions-*.json` except `-observation` (`transactions_*_parity.rs`). The rest sit
here unused until their Phase 2 step moves the matching mobile parity test in (see `docs/PLAN.md`,
Phase 1).

Not here, because they are not extension output: `vault-*.json` and `migration-v1-to-v5.golden.json`
(earlier mobile cores), `ui-coordination.json` (iOS UI suite) and `rust-gift-wrap.json` (Rust
writes it, the extension opens it). They move with their modules as ordinary regression fixtures.

`tests/conformance/mobile-*.test.ts` in Bramble fails when a committed fixture drifts from the
extension, so a behaviour change there forces a re-generate; re-copy, update the hashes and commit
in this table, and the core then fails here until it matches.
