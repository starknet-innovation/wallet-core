# wallet-core plan

One Rust core for Bramble, consumed by the browser extension as a wasm32 module and by the iOS and
Android apps through UniFFI. It replaces two implementations of the same wallet logic: the
extension's TypeScript (`packages/*` in `starknet-innovation/mc-wallet`) and the mobile Rust crate
(`mobile/core`, `bramble-core`, on `feature/mobile-wallet`). The wallet logic itself is kept
generic, so another Starknet wallet can build on it without inheriting Bramble's choices.

Decisions, 2026-11-09:

- Fresh crates here; modules are copied from `mobile/core` as each phase ports them. No history move.
- Two crates, one repo: `wallet-core` (generic) and `bramble-core` (Bramble). See
  [Generic and Bramble](#generic-and-bramble).
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

## Generic and Bramble

`wallet-core` holds mechanism; `bramble-core` holds Bramble's choices and the FFI. The rule: a
product decision is an **argument** in `wallet-core` and a **constant** in `bramble-core`. No
traits or config objects for wallets that do not exist yet.

| `wallet-core` (plain Rust, no FFI) | `bramble-core` (UniFFI, ships to npm, Swift, Kotlin) |
| --- | --- |
| felt grammar; mnemonic generation and validation | `OZ_ACCOUNT_VERSION = "3.0.0"`; `oz_account_class_hash(network)` passes it in |
| Stark and Ready (Argent legacy) derivation, signing, STRK20 viewing key | which of those cross the FFI (`raw-keys`) |
| OZ address derivation with a **required** salt | `KmsPort`'s optional salt (omitted = public key, Krusty's default); OZ accounts pass `0x0` from `account-contract.ts` |
| OZ class hash at a caller-given manifest version | `CoreError`, `core_info()`, remote exports of `wallet-core`'s records |
| Nostr key derivation, NIP-59 wrap/open, event grammar and resource bounds | parity fixtures and tests against the extension |
| Phase 2+: vault container (envelope v3), storage and clock traits, sans-IO RPC, transaction hashing, transfer planning, portfolio/history/privacy logic | Phase 2+: v12 vault schema and migrations, UniFFI foreign traits adapting to `wallet-core`'s, catalogs, iOS/extension-only assets |

The resource bounds (mnemonic, passphrase, Nostr event and data sizes) are Bramble's numbers, kept in
`wallet-core` because they protect against oversized input rather than express product taste.

`bramble-core` keeps the crate, lib and UniFFI module names mobile already uses (`bramble_core`,
`BrambleCore`, `org.bramble.core`), so Phase 6 swaps the dependency without touching Swift or Kotlin
imports. A second wallet would add its own `<wallet>-core` crate the same way, or use `wallet-core`
from Rust directly.

## Layout

```
Cargo.toml                 workspace; release profile (lto, panic=abort, strip)
rust-toolchain.toml        pinned channel, wasm32-unknown-unknown target
.cargo/config.toml         getrandom backend cfg for wasm32
deny.toml                  licences, advisories, sources
crates/wallet-core/        generic: lib wallet_core, plain Rust
  src/lib.rs               Error, KRUSTY_VERSION, re-exports
  src/felt.rs              strict lowercase-hex felt grammar
  src/kms.rs               key custody over Krusty
  src/nostr.rs             coordination identity, NIP-59 wrap/open
crates/bramble-core/       Bramble: lib bramble_core, the cdylib/staticlib/wasm32 module
  src/lib.rs               UniFFI scaffolding, CoreError, core_info(), Bramble constants, exports
  src/bin/uniffi-bindgen.rs  Swift/Kotlin generator (`bindgen` feature)
  uniffi.toml              Swift/Kotlin module names, as mobile's
  tests/parity.rs          extension fixtures and known-answer vectors
  tests/fixtures/          copied from Bramble; README names the commit
js/                        npm package @starknet-innovation/bramble-core: ubrn config,
                           entrypoint, smoke test over the built wasm
.github/workflows/         ci.yml (rust + wasm jobs), release.yml (tag → npm, with provenance)
```

Features (on `bramble-core`): `raw-keys` (default) exports the functions that take or return a raw
Stark private key, which the extension's v12 vault needs until Phase 2; mobile builds
`--no-default-features`. `bindgen` adds the Swift/Kotlin generator binary. `wallet-core` has no
features.

## Phases

Each phase names the crate a module lands in; the table above decides anything not named.

0. **Scaffold** (done): the layout above, KMS and Nostr ported, native and wasm32 builds, parity
   tests in Rust and through the built wasm under Node, CI and release workflows. Split into
   `wallet-core` and `bramble-core` the same day; the exported API did not change.
1. **Extension KMS swap** (Bramble PR to `main`): pin the npm package; a thin
   `packages/platform/kms-bramble-core/` adapter maps the generated API onto `KmsPort` (snake to
   camel case is already done by the bindings; `CoreError` becomes `WalletError`); delete
   `packages/platform/kms-krusty` and the Krusty npm dependency; `scripts/check-bramble-core.ts`
   replaces `check-krusty-adapter.ts` (package fields, sha256 of `bramble_core.wasm` and
   `bramble_core_bg.js`, the same known-answer vectors); `uniffiInitAsync` runs in
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
     OpenZeppelin addresses still use salt `0x0`, which `account-contract.ts` passes explicitly; the
     adapter forwards an omitted salt as omitted (`bramble-core` then uses the public key, as
     Krusty does today).
   - `deriveStrk20ViewingKey` refuses non-canonical scopes (`0x0123`), as Krusty 0.11 does today.
2. **Vault and accounts**: port `container.rs` (envelope v3, byte-compatible) and the vault
   mechanics into `wallet-core`, with plain Rust `Storage` and `Clock` traits. `bramble-core` adopts
   the extension's v12 schema as its model and adds `HostStorage` and `HostClock` as
   `#[uniffi::export(with_foreign)]` traits (they work under wasm2) that adapt to `wallet-core`'s.
   `encrypted-store.ts` becomes a shim over the core. Keys stop leaving Rust and `raw-keys` leaves
   the extension build. Migrations from schemas before v12 stay in TypeScript until Rust owns the
   schema.
3. **Sans-IO transactions**: `rpc.rs`, `send/*` and transaction hashing into `wallet-core`;
   Bramble's defaults (fee margins, which flows it offers) into `bramble-core`.
   `@bramble/transactions` drives the `TransferPlan` step loop.
4. **Portfolio, history, privacy reads**: `portfolio/*`, `history/*`, `privacy/*` into
   `wallet-core`, with the `js/*` JavaScript-semantics shims beside whichever code calls them; the
   iOS `include_str!` in `history/appearance.rs` moves into `bramble-core`'s `generated/`.
5. **Catalogs**: `generated/*.json` are owned by `bramble-core` (Bramble curates them); `wallet-core`
   takes catalogs as arguments. Bramble's `scripts/mobile-assets.ts` renderers invert so TypeScript
   reads them from the npm package.
6. **Mobile adoption** (`feature/mobile-wallet`): `mobile/core` is replaced by a git dependency on
   this repo's `bramble-core` at a tag (same crate, lib and module names), Swift and Kotlin
   regenerate on UniFFI 0.31, `files.rs` becomes a `HostStorage` implementation, the mobile vault
   migrates to v12, builds use `--no-default-features`.
7. **Later**: Ledger, dapp provider and permissions, STRK20 submission, swap, gasless, NFTs,
   allowances.

## Parity

Fixtures come from the extension's real code path (`bun run mobile:parity` in Bramble) and are
copied here; `crates/bramble-core/tests/fixtures/README.md` records the Bramble commit. Bramble's
`tests/conformance/mobile-*.test.ts` fails when the fixtures drift from the extension, so after
Phase 1 the same test proves the swap changed nothing. `crates/bramble-core/tests/parity.rs` holds
the Rust side and `js/test/smoke.test.mjs` replays the same vectors through the built wasm, so the
shipped artifact is what is tested. `krusty_version_matches_the_lockfile` (in `wallet-core`) keeps
the Krusty version `core_info()` reports honest. `wallet-core`'s own unit tests cover its grammar
and bounds without Bramble's fixtures.

## Build notes

- wasm32 needs `wasm-bindgen-cli` at the exact version in `Cargo.lock` (getrandom's wasm backend
  reaches wasm-bindgen); `ubrn` runs it and writes `bramble_core_bg.js`, which ships beside the wasm.
- `wallet-core`'s records cross the FFI as `#[uniffi::remote(Record)]` / `remote(Enum)` declarations
  in `bramble-core`; their fields are repeated there and must match (the compiler checks). Under
  wasm2 they generate the same TypeScript as local records.
- Generated TypeScript uses extensionless relative imports; `js/scripts/node-esm-imports.mjs` adds
  `.js` so the package resolves under Node ESM as well as bundlers.
- Under a CSP without `unsafe-eval` (the extension's) the wasm2 player falls back to an interpreted
  dispatcher. `'wasm-unsafe-eval'` is all the manifest needs, and it already has it.
- `strip = "symbols"` drops the `target_features` section; wasm-bindgen warns but the module is
  correct. Switch to `strip = "debuginfo"` plus `wasm-opt` if the warning ever matters.

## Open questions

- npm scope: `@starknet-innovation` (matches the GitHub org) or `@starknetfoundation` (matches Krusty).
  Settle before the first release tag; the extension's pin fixes the name.
- Bramble PR #803 (Krusty 0.11 → 0.14) was closed without comment; this repo pins 2.0.0, one day old
  at the time of writing, so review its diff (Bitcoin and QSB modules, zeroizing HMAC) before relying
  on it. This blocks Phase 1: the extension moves from Krusty 0.11 to 2.0.
- UniFFI 0.29.5 → 0.31.2: what changes in the generated Swift and Kotlin (Phase 6).
- `HostStorage` is synchronous and `chrome.storage` is not; the extension side needs a write-through
  mirror (the pattern `encrypted-store.ts` already uses).
- Whether the devnet OZ class hash moves into the core or stays a TypeScript constant.
- Who owns the mobile vault → v12 migration.
- When a second wallet wants `wallet-core`: publish it to crates.io, and whether it ships its own
  UniFFI feature or each wallet keeps exporting from its own crate (today's answer).
