# wallet-core plan

One Rust core for Bramble, consumed by the browser extension as a wasm32 module and by the iOS and
Android apps through UniFFI. It replaces two implementations of the same wallet logic: the
extension's TypeScript (`packages/*` in `starknet-innovation/mc-wallet`) and the mobile Rust crate
(`mobile/core`, `bramble-core`, on `feature/mobile-wallet`).

Decisions, 2026-11-09:

- Mobile migrates first, because it is still being built: every module it moves here stops being
  written twice. The extension follows once the core covers what it needs (revised 2026-11-09;
  the first draft had the extension first).
- The extension is the oracle. Its real code paths generate golden fixtures, and every module
  ported here lands with them; nothing is ported on hand-written expectations (see Golden tests).
- Fresh crate here; modules are copied from `mobile/core` as each phase ports them. No history move.
- UniFFI for every binding, including JavaScript through `uniffi-bindgen-react-native`'s `wasm2`
  flavour. That tool is pre-production; the Rust surface does not depend on it, so the JS layer can
  move to wasm-bindgen directly (or whatever UniFFI's own JS backend becomes) without touching Rust.
- Krusty KMS is compiled in (`krusty-kms =2.0.0`). The extension drops
  `@starknetfoundation/krusty-kms-wasm`; one Krusty, one pin, checked by `core_info()`.
- The extension's vault schema (v12) is the target. Mobile's own vault payload migrates into it.
- The extension consumes a published npm package at an exact pin with a digest check, the way it
  pins Krusty today. Extension PRs go to `main`; mobile adopts the crate on `feature/mobile-wallet`.
- Host I/O is one key/value storage trait plus a clock, both host-provided (files on mobile,
  `chrome.storage` in the extension). Sans-IO RPC stays as the mobile core has it.
- The core becomes the source of truth for the network, account-class, token-registry and privacy
  catalogs. Design tokens stay in the extension's `ui-kit`.
- Everything is in scope long term (Ledger, dapp provider and permissions, STRK20 submission, swap,
  gasless, NFTs, allowances) but not in v1.

## Layout

```
Cargo.toml                 workspace; release profile (lto, panic=abort, strip)
rust-toolchain.toml        pinned channel, wasm32-unknown-unknown target
.cargo/config.toml         getrandom backend cfg for wasm32
deny.toml                  licences, advisories, sources
crates/core/               the crate: wallet-core (lib name wallet_core)
  src/lib.rs               scaffolding, CoreError, core_info()
  src/felt.rs              the extension's felt grammar
  src/kms.rs               the KmsPort surface
  src/nostr.rs             coordination identity, NIP-59 wrap/open
  tests/parity.rs          extension fixtures and known-answer vectors
  tests/fixtures/          copied from Bramble; README names the commit
js/                        npm package: ubrn config, entrypoint, smoke test over the built wasm
.github/workflows/         ci.yml (rust + wasm jobs), release.yml (tag → npm, with provenance)
```

Features: `raw-keys` (default) exposes the functions that take or return a raw Stark private key,
which the extension's v12 vault needs until Phase 3.2; mobile builds `--no-default-features`.
`bindgen` adds the Swift/Kotlin generator binary.

## Phases

0. **Scaffold** (done): the layout above, KMS and Nostr ported, native and wasm32 builds, parity
   tests in Rust and through the built wasm under Node, CI and release workflows.
1. **Golden suite**: copy every fixture the extension generates for mobile here, under the
   manifest described in Golden tests, before any further port. Bramble writes them into
   `mobile/core/tests/fixtures/` with `bun run mobile:parity`, which also runs the transaction and
   privacy generators:
   - KMS, accounts and a sealed envelope: `extension-parity.json`, `accounts-parity-*.json`.
   - Nostr multisig: `nostr-multisig-parity.json`.
   - Portfolio, tokens, history rows: `portfolio-parity.jsonl`.
   - Privacy: `extension-privacy-*.json`.
   - Transactions: `extension-transactions-*.json`.

   The same directory also holds fixtures that are *not* extension output, and they are not
   golden: `vault-v1.json`, `vault-v2-*.json`, `vault-v3-container.json` and
   `migration-v1-to-v5.golden.json` were written by earlier mobile cores and pin mobile's own
   migration chain; `ui-coordination.json` drives the iOS UI suite; `rust-gift-wrap.json` runs the
   other way (Rust writes it, the extension opens it). Those move with their modules as ordinary
   regression fixtures. Fixtures for modules not yet ported sit here unused until their phase.
   Where the extension has behaviour mobile ports but no generator covers it, the generator is
   added in Bramble first.
2. **Mobile adoption** (`feature/mobile-wallet`), one module group at a time. Each step moves the
   module from `mobile/core` into this crate together with its Rust parity test
   (`tests/*_parity.rs` in `mobile/core`), reading the golden copies from Phase 1, tags a release, and has `mobile/core` take the
   new tag as a git dependency and delete its copy. From then on, mobile work on that module is a
   PR here.
   1. **KMS, Nostr, felt.** Split `kms`, `nostr` and `felt` into a typed Rust API (`Felt`,
      `Zeroizing`) with the existing `#[uniffi::export]` String functions as thin wrappers, so
      mobile calls Rust, not the FFI shape, and the extension surface is unchanged.
      `mobile/core/src/kms_accounts.rs` calls `kms::private_key` and stays in mobile until 2.2,
      so key derivation must be `pub` to Rust but never `#[uniffi::export]`ed (a crate-private
      function is invisible across crates); `raw-keys` gates only the FFI exports. Mobile takes
      the crate with `default-features = false`, bumps Krusty 0.11 → 2.0.0 and UniFFI 0.29.5 →
      0.31.2 across the whole crate (`send/sign.rs`, `argent_multisig.rs`, `account_catalog.rs`
      included) and regenerates Swift and Kotlin; one Krusty and one UniFFI in the binary. UniFFI
      library mode will also emit wallet-core's exports into the Swift and Kotlin output; mobile
      either uses them or excludes the crate in `uniffi.toml`.
      Bramble's `tests/conformance/mobile-generated.test.ts` requires mobile's Krusty pin to equal
      the extension's npm pin (0.11.0), so it fails the moment mobile moves to 2.0.0 while the
      extension stays on 0.11 until Phase 3.1. Change it to check mobile's pin against
      wallet-core's `core_info()` instead; the golden vectors, still generated by the extension on
      0.11, are what prove 2.0.0 derives and signs the same.
   2. **Vault and accounts**: `vault.rs`, `container.rs` (envelope v3, byte-compatible),
      `account_*`, `kms_accounts.rs`, `wallet_accounts.rs`; the extension's v12 schema becomes the
      core's model and the mobile vault migrates into it. `HostStorage` and `HostClock` arrive as
      `#[uniffi::export(with_foreign)]` traits; `files.rs` becomes mobile's `HostStorage`.
   3. **Sans-IO transactions**: `rpc.rs`, `send/*`, transaction hashing, Argent and Braavos paths.
   4. **Portfolio, history, privacy reads**: `portfolio/*`, `history/*`, `activity/*`, `privacy/*`
      and the `js/*` JavaScript-semantics shims; the iOS `include_str!` in
      `history/appearance.rs` moves into `generated/`.
   5. **Catalogs**: `generated/*.json` are owned here; Bramble's `scripts/mobile-assets.ts`
      renderers invert so TypeScript reads them from the npm package.

   What stays in `mobile/core` is platform glue with no extension counterpart (biometric slots,
   file storage, app UI state), unless the extension gains it.
3. **Extension adoption** (Bramble PRs to `main`), against a core that by now covers vault,
   accounts and transactions:
   1. **KMS swap**: pin the npm package; a thin `packages/platform/kms-wallet-core/` adapter maps
      the generated API onto `KmsPort` (snake to camel case is already done by the bindings;
      `CoreError` becomes `WalletError`); delete `packages/platform/kms-krusty` and the Krusty npm
      dependency; `scripts/check-wallet-core.ts` replaces `check-krusty-adapter.ts` (package
      fields, sha256 of `wallet_core.wasm` and `wallet_core_bg.js`, the same known-answer
      vectors); `uniffiInitAsync` runs in `apps/extension/src/background/index.ts` before
      `createWalletVaultService`; `build.ts` copies the wasm and glue; `PINNED_RUNTIME_FILES` and
      `EXPECTED_DEPENDENCIES` in `scripts/config-standards-policy.ts`, `ALLOWED_EDGES` in
      `check-workspace-architecture.ts`, and `packages/testing/testkit/src/fake-kms.ts` follow.
      `getOzAccountClassHash` in `vault/src/accounts/account-contract.ts` calls
      `ozAccountClassHash(network)` and keeps its devnet constant. `wrapNostrApplicationData`
      gains a `createdAt` argument supplied from `Date.now()`. Three behaviours the adapter must
      keep in mind:
      - `validateMnemonic` returns `false` for any text; the old adapter threw on blank or
        oversized text. Keep that `isMnemonicText` throw in the TypeScript adapter.
      - `ozAccountClassHash` resolves Krusty's manifest at the pinned `OZ_ACCOUNT_VERSION`
        (3.0.0), as `account-contract.ts` does, never `latest`, so a Krusty bump cannot move
        addresses. OpenZeppelin addresses still use salt `0x0`, which the adapter passes
        explicitly.
      - `deriveStrk20ViewingKey` refuses non-canonical scopes (`0x0123`), as Krusty 0.11 does
        today.
   2. **Vault**: `encrypted-store.ts` becomes a shim over the core, with a write-through mirror
      over `chrome.storage`. Keys stop leaving Rust and `raw-keys` leaves the extension build.
      Migrations from schemas before v12 stay in TypeScript.
   3. **Transactions, then reads**: `@bramble/transactions` drives the `TransferPlan` step loop;
      portfolio, history and privacy follow.

   The extension's TypeScript is the oracle until it is replaced, so each swap is proved by the
   golden suite passing through the extension's own conformance tests before the TypeScript goes.
4. **Later**: Ledger, dapp provider and permissions, STRK20 submission, swap, gasless, NFTs,
   allowances.

## Golden tests

The extension's TypeScript is the reference implementation. Its generators in Bramble
(`scripts/generate-mobile-parity-vectors.ts`, which also imports
`generate-mobile-transaction-vectors.ts` and `generate-mobile-privacy-vectors.ts`, all behind
`bun run mobile:parity`) run the real code paths (Krusty, `encryptJson`, account
discovery, transfer planning, portfolio rendering) and write JSON fixtures. Re-running them leaves
no diff; seals with fresh randomness are kept while the extension still opens them.

- **Here**: the fixtures live in `crates/core/tests/fixtures/`, with `README.md` as the manifest:
  per file, the generator, the Bramble commit and the sha256. A test checks the hashes, so a
  hand-edited fixture fails. Bramble stays the source: syncing is re-running the generator there
  and copying the files, never editing them here.
- **Three harnesses, one set of vectors**: the Rust tests (`tests/*.rs`), the built wasm under Node
  (`js/test/smoke.test.mjs`, so the shipped artifact is what is tested), and mobile, whose existing
  parity tests move here with their modules.
- **Drift**: `bun run check` in Bramble (the `tests/conformance/mobile-*.test.ts` suites) fails
  when a committed fixture no longer matches what the extension produces, so an extension change that alters behaviour forces a re-generate,
  and the re-copied fixture then fails here until the core matches. After Phase 3 those same
  conformance tests run the extension through the core, proving each swap changed nothing.
- **Gaps**: mobile-only behaviour (biometric, file storage) has ordinary Rust tests; it has no
  oracle. Extension behaviour without a generator gets one in Bramble before its module is ported.
- `core_info_matches_the_lockfile` keeps the reported Krusty version honest.

## Build notes

- wasm32 needs `wasm-bindgen-cli` at the exact version in `Cargo.lock` (getrandom's wasm backend
  reaches wasm-bindgen); `ubrn` runs it and writes `wallet_core_bg.js`, which ships beside the wasm.
- Generated TypeScript uses extensionless relative imports; `js/scripts/node-esm-imports.mjs` adds
  `.js` so the package resolves under Node ESM as well as bundlers.
- Under a CSP without `unsafe-eval` (the extension's) the wasm2 player falls back to an interpreted
  dispatcher. `'wasm-unsafe-eval'` is all the manifest needs, and it already has it.
- `strip = "symbols"` drops the `target_features` section; wasm-bindgen warns but the module is
  correct. Switch to `strip = "debuginfo"` plus `wasm-opt` if the warning ever matters.

## Open questions

- npm scope: `@starknet-innovation` (matches the GitHub org) or `@starknetfoundation` (matches Krusty).
- Bramble PR #803 (Krusty 0.11 → 0.14) was closed without comment; this repo pins 2.0.0, one day old
  at the time of writing, so review its diff (Bitcoin and QSB modules, zeroizing HMAC) before relying on it. Blocks
  Phase 2.1, since mobile moves from 0.11 to it, and that step also breaks Bramble's
  Krusty pin-equality test (see 2.1).
- UniFFI 0.29.5 → 0.31.2: what changes in the generated Swift and Kotlin (Phase 2.1).
- `HostStorage` is synchronous and `chrome.storage` is not; the extension side needs a write-through
  mirror (the pattern `encrypted-store.ts` already uses).
- Whether the devnet OZ class hash moves into the core or stays a TypeScript constant.
- Who owns the mobile vault → v12 migration.
- Which modules count as mobile-only glue and stay in `mobile/core` (Phase 2).
