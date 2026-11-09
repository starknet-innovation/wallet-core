# wallet-core plan

One Rust core for Bramble, consumed by the browser extension as a wasm32 module and by the iOS and
Android apps through UniFFI. It replaces two implementations of the same wallet logic: the
extension's TypeScript (`packages/*` in `starknet-innovation/mc-wallet`) and the mobile Rust crate
(`mobile/core`, `bramble-core`, on `feature/mobile-wallet`). The wallet logic itself is kept
generic, so another Starknet wallet can build on it without inheriting Bramble's choices.

Decisions, 2026-11-09:

- Mobile migrates first, because it is still being built: every module it moves here stops being
  written twice. The extension follows once the core covers what it needs (revised 2026-11-09;
  the first draft had the extension first).
- The extension is the oracle. Its real code paths generate golden fixtures, and every module
  ported here lands with them; nothing is ported on hand-written expectations (see Golden tests).
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
| Later phases: vault container (envelope v3), storage and clock traits, sans-IO RPC, transaction hashing, transfer planning, portfolio/history/privacy logic | Later phases: v12 vault schema and migrations, UniFFI foreign traits adapting to `wallet-core`'s, catalogs, iOS/extension-only assets |

The resource bounds (mnemonic, passphrase, Nostr event and data sizes) are Bramble's numbers, kept in
`wallet-core` because they protect against oversized input rather than express product taste.

`bramble-core` keeps the crate, lib and UniFFI module names mobile already uses (`bramble_core`,
`BrambleCore`, `org.bramble.core`), so the last mobile step (2.6) swaps the dependency without
touching Swift or Kotlin imports. A second wallet would add its own `<wallet>-core` crate the same
way, or use `wallet-core` from Rust directly.

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
Stark private key, which the extension's v12 vault needs until Phase 3.2; mobile builds
`--no-default-features`. `bindgen` adds the Swift/Kotlin generator binary. `wallet-core` has no
features.

## Phases

Each phase names the crate a module lands in; the table above decides anything not named.

0. **Scaffold** (done): the layout above, KMS and Nostr ported, native and wasm32 builds, parity
   tests in Rust and through the built wasm under Node, CI and release workflows. Split into
   `wallet-core` and `bramble-core` the same day; the exported API did not change.
1. **Golden suite** (done: 22 files from Bramble `0a4bd84c`, hash-checked by
   `crates/bramble-core/tests/fixtures.rs`; the hand-cut `nostr-parity.json` subset gave way to the
   whole `nostr-multisig-parity.json`): copy every fixture the extension generates for mobile into
   `crates/bramble-core/tests/fixtures/`, under the manifest described in Golden tests, before any
   further port. Bramble writes them into `mobile/core/tests/fixtures/` with `bun run mobile:parity`,
   which also runs the transaction and privacy generators:
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
2. **Mobile adoption** (`feature/mobile-wallet`), one module group at a time. Mobile's crate is
   also named `bramble-core`, so until the last step it cannot depend on this repo's
   `bramble-core`; it depends on `wallet-core` (plain Rust, no UniFFI, so no second scaffolding
   in the binary) at a tag. Each step moves the module's mechanism from `mobile/core` into
   `wallet-core` per the Generic and Bramble table, and its Rust parity test
   (`tests/*_parity.rs` in `mobile/core`) into `crates/bramble-core/tests/`, reading the golden
   copies from Phase 1; `mobile/core`
   takes the new tag, deletes its copy and keeps Bramble's constants and exports. From then on,
   mobile work on that mechanism is a PR here.
   1. **KMS, Nostr, felt** (already in `wallet-core`). Mobile bumps Krusty 0.11 → 2.0.0 across
      the whole crate (`send/sign.rs`, `argent_multisig.rs`, `account_catalog.rs` included) and
      deletes `kms.rs`, `nostr.rs` and `felt.rs`. Two gaps first:
      - (done) `wallet-core`'s API was String in, String out, and `StarkKeypair.private_key` a
        plain `String`. Mobile works in `Felt` and holds keys in `Zeroizing`
        (`kms_accounts.rs` calls `kms::private_key`). `wallet-core` now borrows secrets (`&str`)
        and returns them zeroizing: phrases as `Zeroizing<String>`, private and viewing keys as
        Krusty's `SecretFelt` (re-exported, redacted `Debug`). `bramble-core` converts to `String`
        only at the FFI, behind `raw-keys`. The Nostr event grammar mobile's relay code uses
        (`NostrEvent`, `parse_event_json`, `event_from_value`, `has_recipient_tag`,
        `is_public_key`, `safe_non_negative_integer`, `kms::utf16_length`) is public.
        Mobile's implementation is still in flux, so where it differs from the extension it takes
        the core's behaviour rather than the core bending to it: its 256-byte passphrase bound
        becomes the extension's 1,024 UTF-16 units (`wallet-model/preferences/passphrase.ts`), and
        its `oz_class_hash` stops reading Krusty's `latest` and uses `OZ_ACCOUNT_VERSION` (3.0.0,
        `account-contract.ts`), so a Krusty bump cannot move its addresses.
      - Bramble's `tests/conformance/mobile-generated.test.ts` requires mobile's Krusty pin to
        equal the extension's npm pin (0.11.0), so it fails the moment mobile moves to 2.0.0
        while the extension stays on 0.11 until Phase 3.1. Point it at
        `wallet_core::KRUSTY_VERSION` instead; the golden vectors, still generated by the
        extension on 0.11, are what prove 2.0.0 derives and signs the same.
   2. **Vault and accounts**: `container.rs` (envelope v3, byte-compatible) and the vault
      mechanics into `wallet-core`, with plain Rust `Storage` and `Clock` traits; mobile's
      `files.rs` implements `Storage`. The v12 schema and migrations are Bramble's and stay in
      `mobile/core` until 2.6; the mobile vault migrates into v12 here.
   3. **Sans-IO transactions**: `rpc.rs`, `send/*` and transaction hashing, Argent and Braavos
      paths, into `wallet-core`; Bramble's defaults (fee margins, which flows it offers) stay
      with the Bramble crate.
   4. **Portfolio, history, privacy reads**: `portfolio/*`, `history/*`, `activity/*`,
      `privacy/*` into `wallet-core`, with the `js/*` JavaScript-semantics shims beside whichever
      code calls them; the iOS `include_str!` in `history/appearance.rs` moves to the Bramble
      crate's `generated/`.
   5. **Catalogs**: `generated/*.json` are Bramble's; `wallet-core` takes catalogs as arguments.
   6. **Swap**: what remains in `mobile/core` that is Bramble's but not mobile-only (constants,
      v12 schema and migrations, catalogs, the UniFFI surface, `HostStorage` and `HostClock` as
      `#[uniffi::export(with_foreign)]` traits adapting to `wallet-core`'s) moves into this
      repo's `bramble-core`. `mobile/core` is then replaced by a git dependency on it at a tag
      (same crate, lib and module names), Swift and Kotlin regenerate on UniFFI 0.29.5 → 0.31.2,
      and builds use `--no-default-features`.

   Mobile-only platform glue with no extension counterpart (biometric slots, file storage, app
   UI state) stays in the app, or in `bramble-core` behind mobile's build, unless the extension
   gains it.
3. **Extension adoption** (Bramble PRs to `main`), against a `bramble-core` that by now covers
   vault, accounts and transactions:
   1. **KMS swap**: pin the npm package; a thin `packages/platform/kms-bramble-core/` adapter maps
      the generated API onto `KmsPort` (snake to camel case is already done by the bindings;
      `CoreError` becomes `WalletError`); delete `packages/platform/kms-krusty` and the Krusty npm
      dependency; `scripts/check-bramble-core.ts` replaces `check-krusty-adapter.ts` (package
      fields, sha256 of `bramble_core.wasm` and `bramble_core_bg.js`, the same known-answer
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
        addresses. OpenZeppelin addresses still use salt `0x0`, which `account-contract.ts`
        passes explicitly; the adapter forwards an omitted salt as omitted (`bramble-core` then
        uses the public key, as Krusty does today).
      - `deriveStrk20ViewingKey` refuses non-canonical scopes (`0x0123`), as Krusty 0.11 does
        today.
   2. **Vault**: `encrypted-store.ts` becomes a shim over the core, with a write-through mirror
      over `chrome.storage`. Keys stop leaving Rust and `raw-keys` leaves the extension build.
      Migrations from schemas before v12 stay in TypeScript.
   3. **Transactions, then reads**: `@bramble/transactions` drives the `TransferPlan` step loop;
      portfolio, history and privacy follow. Bramble's `scripts/mobile-assets.ts` renderers
      invert so TypeScript reads catalogs from the npm package.

   The extension's TypeScript is the oracle until it is replaced, so each swap is proved by the
   golden suite passing through the extension's own conformance tests before the TypeScript goes.
4. **Later**: Ledger, dapp provider and permissions, STRK20 submission, swap, gasless, NFTs,
   allowances.

## Golden tests

The extension's TypeScript is the reference implementation. Its generators in Bramble
(`scripts/generate-mobile-parity-vectors.ts`, which also imports
`generate-mobile-transaction-vectors.ts` and `generate-mobile-privacy-vectors.ts`, all behind
`bun run mobile:parity`) run the real code paths (Krusty, `encryptJson`, account discovery,
transfer planning, portfolio rendering) and write JSON fixtures. Re-running them leaves no diff;
seals with fresh randomness are kept while the extension still opens them.

- **Here**: the fixtures live in `crates/bramble-core/tests/fixtures/`, with `README.md` as the
  manifest: per file, the generator, the Bramble commit and the sha256. A test checks the hashes,
  so a hand-edited fixture fails. Bramble stays the source: syncing is re-running the generator
  there and copying the files, never editing them here.
- **Three harnesses, one set of vectors**: the Rust tests (`crates/bramble-core/tests/*.rs`,
  which exercise `wallet-core` through Bramble's constants), the built wasm under Node
  (`js/test/smoke.test.mjs`, so the shipped artifact is what is tested), and mobile, whose
  existing parity tests move here with their modules. `wallet-core`'s own unit tests cover its
  grammar and bounds without Bramble's fixtures.
- **Drift**: `bun run check` in Bramble (the `tests/conformance/mobile-*.test.ts` suites) fails
  when a committed fixture no longer matches what the extension produces, so an extension change
  that alters behaviour forces a re-generate, and the re-copied fixture then fails here until the
  core matches. After Phase 3 those same conformance tests run the extension through the core,
  proving each swap changed nothing.
- **Gaps**: mobile-only behaviour (biometric, file storage) has ordinary Rust tests; it has no
  oracle. Extension behaviour without a generator gets one in Bramble before its module is ported.
- `krusty_version_matches_the_lockfile` (in `wallet-core`) keeps the Krusty version `core_info()`
  reports honest.

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
  on it. This blocks Phase 2.1: mobile moves from Krusty 0.11 to 2.0, and that step also breaks
  Bramble's Krusty pin-equality test (see 2.1).
- UniFFI 0.29.5 → 0.31.2: what changes in the generated Swift and Kotlin (Phase 2.6).
- `HostStorage` is synchronous and `chrome.storage` is not; the extension side needs a write-through
  mirror (the pattern `encrypted-store.ts` already uses).
- Whether the devnet OZ class hash moves into the core or stays a TypeScript constant.
- Who owns the mobile vault → v12 migration.
- When a second wallet wants `wallet-core`: publish it to crates.io, and whether it ships its own
  UniFFI feature or each wallet keeps exporting from its own crate (today's answer).
- Which modules count as mobile-only glue, and whether they live in the app or in `bramble-core`
  behind mobile's build (Phase 2).
