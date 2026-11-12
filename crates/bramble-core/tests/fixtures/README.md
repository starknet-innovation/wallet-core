# Golden fixtures

Written by the Bramble extension's real code paths, never by hand. Bramble
(`starknet-innovation/mc-wallet`) is the source: `bun run mobile:parity` writes every file below into
`mobile/core/tests/fixtures/`, except `accounts.json`, `networks.json` and `token-registry.json`,
which `bun run mobile:assets` renders from the extension's account, network and token tables into
`mobile/core/generated/`. Syncing is copying them here
unchanged, all at one Bramble commit. `tests/fixtures.rs` checks this table against the directory:
every file must be listed, and its sha256 must match.

All files: Bramble commit `7293d1881905d5bdee0b8157c13f6f4b054fd59c` (`mobile/g2-multisig-coordination-generator`,
2026-11-11: the G3, G1 and G2 generators stacked on `feature/mobile-wallet` at `01bd6517`; not
merged yet, so the commit line moves to the merged commit at the next copy), privacy SDK `vendor/starknet-privacy` at
`b40bf109` (Bramble's pin). Re-running `mobile:parity` and `mobile:assets` there leaves no diff.
Generators are under `scripts/` in Bramble; each fixture's is reached from
`generate-mobile-parity-vectors.ts`.

| File | Generator | sha256 |
| --- | --- | --- |
| `extension-parity.json` | `generate-mobile-parity-vectors.ts` | `a897813e2d532d86ed483bb3e909870ec30013dfc95f9c131279a603dfb98ef1` |
| `accounts-parity-families-mainnet.json` | `mobile-parity-accounts.ts` | `ca50194ee4bb565e0d5ab6c4d61f50369efa79f046ff10b4d5c54b1198710594` |
| `accounts-parity-families-sepolia.json` | `mobile-parity-accounts.ts` | `6cfb3ecc0490ec06c8a53b50fc795b749a818e11d3410b06f1a8c8ba14514634` |
| `accounts-parity-recovery.json` | `mobile-parity-accounts.ts` | `15fa78d7a9fa08a17e82585f83b1d8d6282c1e6c5414673b1d2d345c1f1f2c85` |
| `accounts-parity-rules.json` | `mobile-parity-accounts.ts` | `48f09827f5fd82119b01e7f5f39b17af30a42e4684031b817b40422966d8dc14` |
| `accounts-parity-scan.json` | `mobile-parity-scan.ts` | `1dec0dea47148d4f8b1f3d4ab3e527e99e1e2b344a70880cfb776aff0afa7b80` |
| `accounts-parity-scan-signing.json` | `mobile-parity-scan.ts` | `650589076793744ea3057610c2e7afb1985c265efac1c3dc05ea580f6ac1f01e` |
| `accounts.json` | `generate-mobile-assets.ts` (`mobile-assets/accounts.ts`) | `67bfdf2f70406e7e3f694ea97a66431d1d7e3ad33c153f86bf6780693eb259d3` |
| `networks.json` | `generate-mobile-assets.ts` | `7c143b575ddb42550bfe3e4caffaad3de22d9b1ae8b7acdfa71d5ee50744e296` |
| `token-registry.json` | `generate-mobile-assets.ts` | `83b8a9a6ae6fe98650daa28969828e29936d1e2c8f1e6cd130cd7a3560a88255` |
| `nostr-multisig-parity.json` | `mobile-assets/write-nostr-multisig-parity.ts` | `445cbbcbb0104350709ac3cfc93a74a6dc5af2286de674e0512ceb020ed6357e` |
| `nostr-coordination-parity-rules.json` | `mobile-assets/write-nostr-coordination-parity.ts` | `961cebd8144d12397dcfda445b30708d63156e528d7b3c7edd3d71ef18bf4c4b` |
| `nostr-coordination-parity-relays.json` | `mobile-assets/write-nostr-coordination-parity.ts` | `f5df889ec2e42ba1bf2271878f1af56572403f61e4c6875b97cec88ce125ca1b` |
| `nostr-coordination-parity-paging.json` | `mobile-assets/write-nostr-coordination-parity.ts` | `c57293ac4507bc310ffdb1510e508f829d589615e3757597d8e52a8d20a147a5` |
| `portfolio-parity.jsonl` | `mobile-parity-portfolio/` | `89348ba0e6b64d5662ae5860a084b10492288bdd2029a88a3e99131cf2a27c7f` |
| `extension-privacy-copy.json` | `generate-mobile-privacy-vectors.ts` | `62ef74e7e64fb79e01c00a5c95350d3ddd719340c5a50ee31e181ef7dff82838` |
| `extension-privacy-rules.json` | `generate-mobile-privacy-vectors.ts` | `fa7729d067c5d1bdfb57d9857075622b97effba165f029d69732552ccb59ae70` |
| `extension-privacy-wire.json` | `generate-mobile-privacy-vectors.ts` | `6274911bb8d13c31815cb1f32cc4d158be634d47180aad9d77cbb01cac717171` |
| `extension-transactions-argent-refusals.json` | `generate-mobile-transaction-vectors.ts` | `f2eb7492ca5425e618b9954b22fc666bf7a2a4645438ba8837a1c3f6b78d2997` |
| `extension-transactions-argent-upgraded.json` | `generate-mobile-transaction-vectors.ts` | `9d14e829c7c2dfccb940e150984036db18a528ddd65f134763371049981b8156` |
| `extension-transactions-argent.json` | `generate-mobile-transaction-vectors.ts` | `854103a684d841a7ed2949ac238283d17a934cd7cba9a9d29b522868959edb5f` |
| `extension-transactions-braavos-refusals.json` | `generate-mobile-transaction-vectors.ts` | `5c15ab334651870307d8259a6c75865f8a5e555fa53b2bf50d468879eeefa102` |
| `extension-transactions-braavos.json` | `generate-mobile-transaction-vectors.ts` | `fe2a1bdaf2420700b70396b7214fcb706f2f6aab88fa46ee6fa17f1ed3fc9f4f` |
| `extension-transactions-copy.json` | `generate-mobile-transaction-vectors.ts` | `7fdbd62e36e2e56696ec52a5600ea440dfbcbaee120c6243e7e68085e4fc93f8` |
| `extension-transactions-observation.json` | `generate-mobile-transaction-vectors.ts` | `bc02c237f67d1ac646a86df25305f4be0181a8f3e44d19cf5c748bc898c01541` |
| `extension-transactions-presentation.json` | `generate-mobile-transaction-vectors.ts` | `fcf33dbdc1d1273042910feae40525a10a73dca38f59808e8269ea4e61a4a4c5` |
| `extension-transactions-rules.json` | `generate-mobile-transaction-vectors.ts` | `65673321490171d26684a2e133c23e821d8ac185227f220e21ca383e719af825` |
| `extension-transactions-status.json` | `generate-mobile-transaction-vectors.ts` | `8e36ccc8e9a9752b5d2d704a43fbcb4325cf5a1b6d1bbfc01042a6f6277db671` |
| `extension-transactions-verification.json` | `generate-mobile-transaction-vectors.ts` | `44253f028a3caaf4ea896b6c09afec5c3b512cce729ccabe43e4422f712385be` |
| `extension-transactions-wire.json` | `generate-mobile-transaction-vectors.ts` | `a4e8085ad6b5e14f05c66f291b8198a167cae729e639a812c7af908d412d8e3d` |

Read today: `extension-parity.json` (`parity.rs`, `vault_parity.rs`),
`accounts-parity-families-sepolia.json` and `nostr-multisig-parity.json` (`parity.rs`), `extension-privacy-wire.json`'s viewing keys (`parity.rs`), both
`accounts-parity-families-*.json` and `accounts.json` (`accounts_parity.rs`), both
`accounts-parity-scan*.json` and `accounts.json` (`accounts_scan_parity.rs`; `accounts.json` and
the scan's shapes also feed `accounts_scan_mechanism.rs`), and every
`extension-transactions-*.json` except `-observation` (`transactions_*_parity.rs`), and
`portfolio-parity.jsonl`'s prices, fiat, token and balance blocks with `networks.json` and
`token-registry.json` as Bramble's catalogs (`portfolio_parity.rs`, `token_parity.rs`; the
catalogs also feed `portfolio_state.rs`). The rest sit
here unused until their Phase 2 step moves the matching mobile parity test in (see `docs/PLAN.md`,
Phase 1).

Not here, because they are not extension output: `vault-*.json` and `migration-v1-to-v5.golden.json`
(earlier mobile cores), `ui-coordination.json` (iOS UI suite) and `rust-gift-wrap.json` (Rust
writes it, the extension opens it). They move with their modules as ordinary regression fixtures.

`tests/conformance/mobile-*.test.ts` in Bramble fails when a committed fixture drifts from the
extension, so a behaviour change there forces a re-generate; re-copy, update the hashes and commit
in this table, and the core then fails here until it matches.
