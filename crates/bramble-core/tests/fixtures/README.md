# Golden fixtures

Written by the Bramble extension's real code paths, never by hand. Bramble
(`starknet-innovation/mc-wallet`) is the source: `bun run mobile:parity` writes every file below into
`mobile/core/tests/fixtures/`, except `accounts.json`, `multisig.json`, `networks.json`, `privacy.json` and
`token-registry.json`, which `bun run mobile:assets` renders from the extension's account, multisig,
network, privacy-routing and token tables into `mobile/core/generated/`. Syncing is copying them here
unchanged, all at one Bramble commit. `tests/fixtures.rs` checks this table against the directory:
every file must be listed, and its sha256 must match.

All files: Bramble commit `caea553003905ee5a947fcde1772a125d93c620f` (`feature/mobile-wallet`, 2026-11-25, merging
#1034). Against `aa9d9d1df` (the merge of `main` at v0.10.11) no file changed; four are new: G7i's
`vault-v13-full.json`, `vault-v14.json` and `vault-v13-to-v14.json` (#1033) and G7h's
`extension-gasless.jsonl` (#1034). Against the copy before that (`1771d951`) twelve files changed: the
padded address labels of #884 (`accounts-parity-rules.json`, `portfolio-parity.jsonl`,
`extension-transactions-presentation.json`, `extension-multisig-transactions.json`,
`extension-multisig-transactions-refusals.json`, `extension-staking.jsonl`, and the summaries in
`extension-call-review.jsonl`, `extension-swap.jsonl` and
`extension-transactions-verification.json`), the 56-call cap and the flags' `text`
(`extension-call-review.jsonl`, `extension-swap.jsonl`), the no-pool privacy copy
(`extension-privacy-rules.json`), the receipt fee's `throughProvider`
(`extension-activity-detail.jsonl`) and the review copy (`extension-transactions-copy.json`).
`transactions_rules_parity.rs` excuses the decimal recipients in `-verification.json`'s summaries,
an extension bug fixed on Bramble `fix/review-decimal-recipient`; that exception goes with the next
copy. One file is from another commit: `extension-dapp.jsonl` (P16's generator), Bramble `b4602f0598dca107f48e5f3b9ec647c1cbd86d8b`
(`fixtures/p16-dapp`, on `feature/mobile-wallet` at `25f66184d`), which adds
`scripts/mobile-parity-dapp/` and changes no other code (two `tests/support` stub headers gain a note); the next full copy brings every file to
one commit again. Every file is byte-identical to its copy at that commit; privacy SDK
`vendor/starknet-privacy` at `b40bf109` (Bramble's pin).
A second exception: `extension-ledger.jsonl` comes from Bramble commit
`cd1a18a10d84714ecbc92a37d65fbf75645a0e0f` (`feature/mobile-wallet`, 2026-11-26, merging
Bramble #1061); first copied from its branch at `7cb47788c18d723effcceddb7b13e0235dfef64e`, and
byte-identical at the merge. `bun run mobile:ledger` writes it by hand, not `mobile:parity`: it runs
the extension's Ledger client against app-starknet 2.4.3 (the pinned Nano S+ ELF, sha256
`8f15435c…`) in the digest-pinned Speculos image, seeded with the published parity phrase, so it
needs Docker and the vendored app and nothing checks it for drift. Its two `plan` signatures are
over hashes the core's plan printed (`ledger_parity.rs`'s ignored `print_plan_hashes`), not the
extension's; its second `version` line is a reply the generator wrote (`recorded: false`).
A third exception: `extension-nfts.jsonl` (P17's generator) is from Bramble
`dd85d9ae8d179fed0d47e3c8bf2df520918b5c8f` (`fixtures/g-nft`, on `feature/mobile-wallet` at
`232e21510`), which adds `scripts/mobile-parity-nfts/` and changes no other code (two test-support
headers gain a note); byte-identical to its copy there.
A fourth exception: `extension-multisig-import.jsonl` (G7c's import cases) comes from Bramble commit
`232e21510158f1c046885acbca882bfe941b00c9` (`feature/mobile-wallet`, 2026-11-27, merging #1074);
its generator, `scripts/mobile-parity-multisig-import/`, landed with #1042 (`edc94db3c`).
Generators are under `scripts/` in Bramble; each fixture's is reached from
`generate-mobile-parity-vectors.ts`.

| File | Generator | sha256 |
| --- | --- | --- |
| `extension-parity.json` | `generate-mobile-parity-vectors.ts` | `a897813e2d532d86ed483bb3e909870ec30013dfc95f9c131279a603dfb98ef1` |
| `accounts-parity-families-mainnet.json` | `mobile-parity-accounts.ts` | `ca50194ee4bb565e0d5ab6c4d61f50369efa79f046ff10b4d5c54b1198710594` |
| `accounts-parity-families-sepolia.json` | `mobile-parity-accounts.ts` | `6cfb3ecc0490ec06c8a53b50fc795b749a818e11d3410b06f1a8c8ba14514634` |
| `accounts-parity-recovery.json` | `mobile-parity-accounts.ts` | `15fa78d7a9fa08a17e82585f83b1d8d6282c1e6c5414673b1d2d345c1f1f2c85` |
| `accounts-parity-rules.json` | `mobile-parity-accounts.ts` | `27b8c6f75126972f1d52c370d13fe32f1a223a76d0fbfa3a45d903d022d483c2` |
| `accounts-parity-scan.json` | `mobile-parity-scan.ts` | `1dec0dea47148d4f8b1f3d4ab3e527e99e1e2b344a70880cfb776aff0afa7b80` |
| `accounts-parity-scan-signing.json` | `mobile-parity-scan.ts` | `650589076793744ea3057610c2e7afb1985c265efac1c3dc05ea580f6ac1f01e` |
| `accounts.json` | `generate-mobile-assets.ts` (`mobile-assets/accounts.ts`) | `67bfdf2f70406e7e3f694ea97a66431d1d7e3ad33c153f86bf6780693eb259d3` |
| `multisig.json` | `generate-mobile-assets.ts` | `70d6712f8cb5d7f3b09a87d7896919b835295f9c3e52a86cea9da85b4222676e` |
| `networks.json` | `generate-mobile-assets.ts` | `7c143b575ddb42550bfe3e4caffaad3de22d9b1ae8b7acdfa71d5ee50744e296` |
| `privacy.json` | `generate-mobile-assets.ts` | `0b006b43134c739688678d34c3f0327c7b2c4f4e735e3c3ab49af7bcd229262e` |
| `token-registry.json` | `generate-mobile-assets.ts` | `83b8a9a6ae6fe98650daa28969828e29936d1e2c8f1e6cd130cd7a3560a88255` |
| `nostr-multisig-parity.json` | `mobile-assets/write-nostr-multisig-parity.ts` | `445cbbcbb0104350709ac3cfc93a74a6dc5af2286de674e0512ceb020ed6357e` |
| `nostr-coordination-parity-rules.json` | `mobile-assets/write-nostr-coordination-parity.ts` | `961cebd8144d12397dcfda445b30708d63156e528d7b3c7edd3d71ef18bf4c4b` |
| `nostr-coordination-parity-relays.json` | `mobile-assets/write-nostr-coordination-parity.ts` | `f5df889ec2e42ba1bf2271878f1af56572403f61e4c6875b97cec88ce125ca1b` |
| `nostr-coordination-parity-paging.json` | `mobile-assets/write-nostr-coordination-parity.ts` | `c57293ac4507bc310ffdb1510e508f829d589615e3757597d8e52a8d20a147a5` |
| `portfolio-parity.jsonl` | `mobile-parity-portfolio/` | `dbfb6119814faba09197ef127c1f8e4e01413579d6624a4e2c2dcfcd15cb71ce` |
| `extension-privacy-copy.json` | `generate-mobile-privacy-vectors.ts` | `62ef74e7e64fb79e01c00a5c95350d3ddd719340c5a50ee31e181ef7dff82838` |
| `extension-privacy-rules.json` | `generate-mobile-privacy-vectors.ts` | `771a2b7c98f8bc699b530305f6996aec2b150f96aec4bd4c5fe20358c45c20cd` |
| `extension-privacy-wire.json` | `generate-mobile-privacy-vectors.ts` | `6274911bb8d13c31815cb1f32cc4d158be634d47180aad9d77cbb01cac717171` |
| `extension-transactions-argent-refusals.json` | `generate-mobile-transaction-vectors.ts` | `f2eb7492ca5425e618b9954b22fc666bf7a2a4645438ba8837a1c3f6b78d2997` |
| `extension-transactions-argent-upgraded.json` | `generate-mobile-transaction-vectors.ts` | `9d14e829c7c2dfccb940e150984036db18a528ddd65f134763371049981b8156` |
| `extension-transactions-argent.json` | `generate-mobile-transaction-vectors.ts` | `854103a684d841a7ed2949ac238283d17a934cd7cba9a9d29b522868959edb5f` |
| `extension-transactions-braavos-refusals.json` | `generate-mobile-transaction-vectors.ts` | `5c15ab334651870307d8259a6c75865f8a5e555fa53b2bf50d468879eeefa102` |
| `extension-transactions-braavos.json` | `generate-mobile-transaction-vectors.ts` | `fe2a1bdaf2420700b70396b7214fcb706f2f6aab88fa46ee6fa17f1ed3fc9f4f` |
| `extension-transactions-copy.json` | `generate-mobile-transaction-vectors.ts` | `0b3904f1f16c3d180b1478d8e9a60a80854d81e2a3c17f00331226dff6c5cdbc` |
| `extension-transactions-observation.json` | `generate-mobile-transaction-vectors.ts` | `bc02c237f67d1ac646a86df25305f4be0181a8f3e44d19cf5c748bc898c01541` |
| `extension-transactions-presentation.json` | `generate-mobile-transaction-vectors.ts` | `b374f88f25e31cb4e4d2360e5639c6b6e5fc7516c2592563004765087d6f968a` |
| `extension-transactions-rules.json` | `generate-mobile-transaction-vectors.ts` | `65673321490171d26684a2e133c23e821d8ac185227f220e21ca383e719af825` |
| `extension-transactions-status.json` | `generate-mobile-transaction-vectors.ts` | `8e36ccc8e9a9752b5d2d704a43fbcb4325cf5a1b6d1bbfc01042a6f6277db671` |
| `extension-transactions-verification.json` | `generate-mobile-transaction-vectors.ts` | `c7186e7c4d1a1787aac77d58d519381520ac41ca74c5d13d207adcca8c16ba8c` |
| `extension-transactions-wire.json` | `generate-mobile-transaction-vectors.ts` | `a4e8085ad6b5e14f05c66f291b8198a167cae729e639a812c7af908d412d8e3d` |
| `extension-activity-detail.jsonl` | `mobile-parity-activity/` | `f4a53faad7bb3f3b24d1eee3fee6a956919bc5b24fdf86d2909b917ab48e7d48` |
| `extension-activity-feed.jsonl` | `mobile-parity-activity-feed/` | `109a0a25356ef2ada1a27eb5e09509b894cdb24763ce102d02f5db052d1fbbe9` |
| `extension-allowances.jsonl` | `mobile-parity-allowances/` | `11085ef20db40299f91af82512afb9db25e3d0b0150220e350352c436e673d3b` |
| `extension-swap.jsonl` | `mobile-parity-swap/` | `86408b02ba6f86fc6141ab06418b41da66ba8426860c20ec57c720203fc0c838` |
| `extension-staking.jsonl` | `mobile-parity-staking/` | `6e5b51fcf743981c7486d3aac6ea3adef03cfffb670b20c1f9801aa8f2e27c96` |
| `extension-multisig-transactions.json` | `generate-mobile-parity-vectors.ts` (`mobile-multisig-*.ts`) | `6af5ce7e64ed96da66c8cfc77b264e230395846ad9f3851b1f7fd7f9afe8c8a3` |
| `extension-multisig-transactions-refusals.json` | `generate-mobile-parity-vectors.ts` (`mobile-multisig-*.ts`) | `80b278d7f23092c70ae76306b813cb5fe13cea8e79b018d7223d9fa5a7185653` |
| `extension-multisig-transactions-signers.json` | `generate-mobile-parity-vectors.ts` (`mobile-multisig-*.ts`) | `78ad15af9b68692724cdb1e84118591870f545db892eeaf40ff1e92fe2d9a5b3` |
| `extension-multisig-import.jsonl` | `mobile-parity-multisig-import/` | `bbe09ac4d8b73d9a4af13681bb0eb92850773863897b825bb25acea330208ae5` |
| `extension-call-review.jsonl` | `mobile-parity-calls/` | `c9e106c0c6433eccaad6b5503b1b44d80e09cfd1bcac64904ac9c01224055ce6` |
| `vault-v13.json` | `generate-mobile-parity-vectors.ts` (`mobile-parity-vault.ts`) | `d4f0cf2a62d209cd05286a6d81b372bdaf1d356ba273b41e3e8ed2437912977f` |
| `vault-v13-imported.json` | `generate-mobile-parity-vectors.ts` (`mobile-parity-vault.ts`) | `099ced8a46789f9d3cf7d3c679d5c5287876ccab618b7e7674c9ef65c774da97` |
| `extension-settings-parity.jsonl` | `mobile-parity-settings.ts` | `dfa7f7248b284bac56f1cafa261b7d6941d777bcbe6ea171ae26484d23c395a3` |
| `extension-stark-name.jsonl` | `mobile-parity-stark-name.ts` | `6b12f77b5f9393c078019d671d13b4965cedcf24e0e0bc387ae29814b328ee79` |
| `vault-v13-full.json` | `generate-mobile-parity-vectors.ts` (`mobile-parity-vault-full.ts`) | `fc436cfa55d752a8782112a42b5d08632bcef69f736310823fd8e0f1d8434834` |
| `vault-v14.json` | `generate-mobile-parity-vectors.ts` (`mobile-parity-vault-full.ts`) | `97a437888e92569c45b810d757633506bb971fd9f9f91199ec8bfe05f4e01a5b` |
| `vault-v13-to-v14.json` | `generate-mobile-parity-vectors.ts` (`mobile-parity-vault-full.ts`) | `58e4671a1ebe15bef6159df851211390de48c11b42537c8d6fb2e70290067565` |
| `extension-gasless.jsonl` | `mobile-parity-gasless/` | `c232ecadeb65c8dc36e64d3b51a091eb63c8923698f8d686b269845e4d9e45c7` |
| `extension-dapp.jsonl` | `mobile-parity-dapp/` | `4fa29896a551736d6f8bcb9dda21819b5077df8e6ad1cf15d3bcf2e6d50d9ef5` |
| `extension-nfts.jsonl` | `mobile-parity-nfts/` | `64d78a451ba193fbabd603e149fa6a89ad0009bb29ea2ab5fb047287d67c3591` |
| `extension-ledger.jsonl` | `mobile-ledger-speculos.ts` (`bun run mobile:ledger`) | `944d70116b7606ee1b6a0087dd578132153a26c63b9550e56af267766f201b5d` |

Read today: `extension-parity.json` (`parity.rs`, `vault_parity.rs`),
`accounts-parity-families-sepolia.json` and `nostr-multisig-parity.json` (`parity.rs`;
`nostr-multisig-parity.json`'s `argent`, `classHash` and `joins` with `multisig.json` as Bramble's
pinned class and limits also in `multisig_parity.rs`), `extension-privacy-wire.json`'s viewing keys (`parity.rs`), both
`accounts-parity-families-*.json` and `accounts.json` (`accounts_parity.rs`), both
`accounts-parity-scan*.json` and `accounts.json` (`accounts_scan_parity.rs`; `accounts.json` and
the scan's shapes also feed `accounts_scan_mechanism.rs`), and every
`extension-transactions-*.json` (`transactions_*_parity.rs`; `-observation` in
`transactions_rules_parity.rs`; `-status`'s `notices` sentences are Bramble's copy, which mobile
asserts), and `portfolio-parity.jsonl` (all but `status.empty`, Bramble's
empty-state copy, which mobile asserts, and the top-level `activity.onChain`, which no test has
read) with `networks.json` and
`token-registry.json` as Bramble's catalogs (`portfolio_parity.rs`, `token_parity.rs`,
`history_parity.rs`, `row_parity.rs`; the catalogs also feed `portfolio_state.rs` and
`transactions_rules_parity.rs`'s explorer links), and `extension-privacy-wire.json` and
`extension-privacy-rules.json` with `privacy.json` as Bramble's privacy routing and `networks.json`
(`privacy_parity.rs`, `privacy_gates_parity.rs`, `privacy_plan.rs`), and all three
`nostr-coordination-parity-*.json` with `multisig.json`'s coordination, relay and Nostr values and
`nostr-multisig-parity.json`'s `relayUrls` (`nostr_coordination_parity.rs`), and `vault-v13.json`
(`vault_v13.rs`), and `extension-activity-detail.jsonl` (`activity_parity.rs`; its `receiptFee`
block's `throughProvider` records the extension's starknet.js path, which no test reads), and
`vault-v13-imported.json` (`vault_v13_imported.rs`), `extension-settings-parity.jsonl`
(`settings_parity.rs`), `extension-stark-name.jsonl` (`stark_name_parity.rs`),
`extension-activity-feed.jsonl` with `networks.json` for the explorer links
(`activity_feed_parity.rs`), `extension-call-review.jsonl` (`calls_review_parity.rs`: the call
summaries, the review flags, a multicall on the wire and the batches refused up front),
`extension-allowances.jsonl` (`allowances_parity.rs`; its `log` and `recording` blocks
`allowances_log_parity.rs`, its `scan` and `tracked` transcripts `allowances_scan_parity.rs`),
`extension-swap.jsonl` (`swap_parity.rs`; an accepted build's `executed` reply is the extension's
submit, which no test reads), `extension-staking.jsonl` (`staking_parity.rs`, and its `action`
block `staking_action_parity.rs`), `vault-v13-full.json`, `vault-v14.json` and
`vault-v13-to-v14.json` (`vault_migration.rs`: each payload against its sealed copy, the first two
written back unchanged, and unlocking the migration's input writing back its output), and `extension-ledger.jsonl`
(`ledger_parity.rs`: the readers, `DeviceSigning`, `KeyEnumeration`, `Plan::with_device` and,
under `mobile`, `LedgerKeyReader` with `ledger_account_candidates`), and
`extension-gasless.jsonl` (every line: `gasless_parity.rs` its `hash`, `review` and `offers`
blocks, `gasless_reads_parity.rs` the rest; the lines the core departs from, its
`knownExtensionBug` lines among them, are asserted as departures), and `extension-dapp.jsonl`
(every line: `dapp_request_parity.rs` its `request` block, `dapp_connect_parity.rs` `connect`,
`grants` and `queue`, `dapp_sites_parity.rs` `site`, `feedParse` and `feed`,
`dapp_typed_data_parity.rs` `typedData` and `outside`, `dapp_sign_parity.rs` `sign`; each case the core departs from is listed
there with its reason and asserted as a departure), and `extension-nfts.jsonl`'s `watched` block,
`addNft` and `watchAsset` (`nfts_watched_parity.rs`: the page's reader, then the store, with the
recorded node replies as the ownership verdict; the
`gallery candidates`' `indexed` and `merged` and its other blocks wait for the rest of P17).
`extension-privacy-copy.json` is the source of
Bramble's generated Swift and Kotlin privacy copy; no Rust function returns those sentences, so
no test here reads it. `extension-multisig-transactions.json` is read by
`multisig_deploy_parity.rs` (`deploy`), `multisig_proposal_parity.rs` (`proposal`),
`multisig_signers_parity.rs` (the proposal's summary) and `multisig_intent_review.rs` (the
proposal's summary as the intent card's call line, and the first signer change of `-signers.json`). `multisig_signers_parity.rs` also reads
all of `-signers.json` and the `calls` part of `-refusals.json`; `multisig_deploy_parity.rs` its
`deploySubmit` part and `multisig_refusals_parity.rs` every other part
(`multisig_import_mechanism.rs` builds on its `readyIntent` and `account`).
`extension-multisig-import.jsonl` is read by `multisig_import_parity.rs` (every line; its
`divergence` lines and the wallet-level cases mobile does not share are asserted as departures). See
[P1](../../../../docs/PLAN.md#mobile-parity). `extension-nfts.jsonl` waits for P17's port; no test reads it yet. The rest sit
here unused until their Phase 2 step moves the matching mobile parity test in (see `docs/PLAN.md`,
Phase 1).

Not here, because they are not extension output: `vault-v3-container.json` (an earlier mobile
core), `ui-coordination.json` (iOS UI suite) and `rust-gift-wrap.json` (Rust
writes it, the extension opens it). They move with their modules as ordinary regression fixtures.

`tests/conformance/mobile-*.test.ts` in Bramble fails when a committed fixture drifts from the
extension, so a behaviour change there forces a re-generate; re-copy, update the hashes and commit
in this table, and the core then fails here until it matches.
