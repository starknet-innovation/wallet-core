# wallet-core plan

One Rust core for Bramble, consumed by the browser extension as a wasm32 module and by the iOS and
Android apps through UniFFI. It replaces two implementations of the same wallet logic: the
extension's TypeScript (`packages/*` in `starknet-innovation/mc-wallet`) and the mobile Rust crate
(`mobile/core`, `bramble-core`, on `feature/mobile-wallet`).

Decisions, 2026-11-09:

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
which the extension's v12 vault needs until Phase 2; mobile builds `--no-default-features`.
`bindgen` adds the Swift/Kotlin generator binary.

## Phases

0. **Scaffold** (done): the layout above, KMS and Nostr ported, native and wasm32 builds, parity
   tests in Rust and through the built wasm under Node, CI and release workflows.
1. **Extension KMS swap** (Bramble PR to `main`): pin the npm package; a thin
   `packages/platform/kms-wallet-core/` adapter maps the generated API onto `KmsPort` (snake to
   camel case is already done by the bindings; `CoreError` becomes `WalletError`); delete
   `packages/platform/kms-krusty` and the Krusty npm dependency; `scripts/check-wallet-core.ts`
   replaces `check-krusty-adapter.ts` (package fields, sha256 of `wallet_core.wasm` and
   `wallet_core_bg.js`, the same known-answer vectors); `uniffiInitAsync` runs in
   `apps/extension/src/background/index.ts` before `createWalletVaultService`; `build.ts` copies the
   wasm and glue; `PINNED_RUNTIME_FILES` and `EXPECTED_DEPENDENCIES` in
   `scripts/config-standards-policy.ts`, `ALLOWED_EDGES` in `check-workspace-architecture.ts`, and
   `packages/testing/testkit/src/fake-kms.ts` follow. `getOzAccountClassHash` in
   `vault/src/accounts/account-contract.ts` calls `ozAccountClassHash(network)` and keeps its devnet
   constant. `wrapNostrApplicationData` gains a `createdAt` argument supplied from `Date.now()`.
   Three behaviours the adapter must keep in mind:
   - `validateMnemonic` returns `false` for any text; the old adapter threw on blank or oversized
     text. Keep that `isMnemonicText` throw in the TypeScript adapter.
   - `ozAccountClassHash` resolves Krusty's manifest at the pinned `OZ_ACCOUNT_VERSION` (3.0.0),
     as `account-contract.ts` does, never `latest`, so a Krusty bump cannot move addresses.
     OpenZeppelin addresses still use salt `0x0`, which the adapter passes explicitly.
   - `deriveStrk20ViewingKey` refuses non-canonical scopes (`0x0123`), as Krusty 0.11 does today.
2. **Vault and accounts**: port `vault.rs` and `container.rs` (envelope v3, byte-compatible), adopt
   the extension's v12 schema as the core's model, add `HostStorage` and `HostClock` as
   `#[uniffi::export(with_foreign)]` traits (they work under wasm2), make `encrypted-store.ts` a shim
   over the core. Keys stop leaving Rust and `raw-keys` leaves the extension build. Migrations from
   schemas before v12 stay in TypeScript until Rust owns the schema.
3. **Sans-IO transactions**: `rpc.rs`, `send/*`, transaction hashing; `@bramble/transactions`
   drives the `TransferPlan` step loop.
4. **Portfolio, history, privacy reads**: `portfolio/*`, `history/*`, `privacy/*` and the `js/*`
   JavaScript-semantics shims; the iOS `include_str!` in `history/appearance.rs` moves into
   `generated/`.
5. **Catalogs**: `generated/*.json` are owned here; Bramble's `scripts/mobile-assets.ts` renderers
   invert so TypeScript reads them from the npm package.
6. **Mobile adoption** (`feature/mobile-wallet`): `mobile/core` becomes a git dependency on a tag,
   Swift and Kotlin regenerate on UniFFI 0.31, `files.rs` becomes a `HostStorage` implementation,
   the mobile vault migrates to v12, builds use `--no-default-features`.
7. **Later**: Ledger, dapp provider and permissions, STRK20 submission, swap, gasless, NFTs,
   allowances.

## Parity

Fixtures come from the extension's real code path (`bun run mobile:parity` in Bramble) and are
copied here; `crates/core/tests/fixtures/README.md` records the Bramble commit. Bramble's
`tests/conformance/mobile-*.test.ts` fails when the fixtures drift from the extension, so after
Phase 1 the same test proves the swap changed nothing. `tests/parity.rs` holds the Rust side and
`js/test/smoke.test.mjs` replays the same vectors through the built wasm, so the shipped artifact is
what is tested. `core_info_matches_the_lockfile` keeps the reported Krusty version honest.

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
  at the time of writing, so review its diff (Bitcoin and QSB modules, zeroizing HMAC) before relying on it.
- UniFFI 0.29.5 → 0.31.2: what changes in the generated Swift and Kotlin (Phase 6).
- `HostStorage` is synchronous and `chrome.storage` is not; the extension side needs a write-through
  mirror (the pattern `encrypted-store.ts` already uses).
- Whether the devnet OZ class hash moves into the core or stays a TypeScript constant.
- Who owns the mobile vault → v12 migration.
