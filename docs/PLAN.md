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
- `starknet-types-core` stays at `=0.2.4`, the version Krusty 2.0.0 builds on. `Felt` crosses the
  Krusty boundary, so a second copy in the lockfile would be a second, incompatible `Felt`. It moves
  to 1.x only in the same change as a deliberate Krusty bump, never on its own.
- The extension's vault schema (v12) is the target. Mobile's own vault payload migrates into it.
- The extension consumes a published npm package at an exact pin with a digest check, the way it
  pins Krusty today. Extension PRs go to `main`; mobile adopts the crate on `feature/mobile-wallet`.
- Host I/O is one key/value storage trait plus a clock, both host-provided (files on mobile,
  `chrome.storage` in the extension). Sans-IO RPC stays as the mobile core has it.
- The core becomes the source of truth for the network, account-class, token-registry and privacy
  catalogs. Design tokens stay in the extension's `ui-kit`.
- Everything is in scope long term (Ledger, dapp provider and permissions, STRK20 submission, swap,
  gasless, NFTs, allowances) but not in v1.

## Status

As of 2026-11-11. Every Phase 2 step has two halves: the mechanism lands here, then mobile deletes
its copy and depends on this one. Both halves are done for 2.1, the vault and transactions, so
mobile holds no copy of what this repo has ported. A step is done when both halves are.

| Step | Here | Mobile (`mc-wallet`, `feature/mobile-wallet`, at `01bd6517`) |
| --- | --- | --- |
| 0 Scaffold, 1 Golden suite | done | n/a |
| 2.1 KMS, Nostr, felt | done (#13, #20) | done (Bramble #907, #909), at `v0.1.0` |
| 2.2 Vault | done (#19) | done (Bramble #911), at `v0.1.0`; the vault → v12 migration is placed in 2.7 (see 2.7), owner not named |
| 2.2 Accounts | A1 (#39), F3 (#40) and A2 (#41) open as a stack | G3 open (Bramble #918); M5 open (Bramble #921), pinning A2's branch tip by `rev` until a tag follows |
| 2.3 Transactions | done (#22, #25, #31) | done (Bramble #916), at `v0.2.1` |
| 2.4a Shims | open (#43) | M6 open (Bramble #922) |
| 2.4b Portfolio | open (#44) | M7 open (Bramble #923) |
| 2.4c History and activity | open (#45) | M8 open (Bramble #924) |
| 2.4d Privacy | open (#46) | M9 open (Bramble #925) |
| 2.5a Multisig account | open (#47) | M10 open (Bramble #926) |
| 2.5b Multisig coordination, 2.6 Catalogs, 2.7 Swap | not started; F1 and F2 open (#42) for 2.4d and 2.5 | not started; G1 and G2 open (Bramble #919, #920) |
| 3 Extension | not started; `js/package.json` is private | n/a |

`v0.1.0` is tagged at `35abc8b` (GitHub release only) and was what M2 and M3 pinned. `v0.2.0`
renamed the package to `starknet-wallet-core` and carries #25, which removed
`PlanHost::discard`. `v0.2.1` is what mobile pins now, for everything: it makes `send::plan_probe`'s selectors,
`selector` and `braavos_signers` public, which mobile's `recovery_advisory.rs` uses. `v0.2.2`
is the first crates.io publish (#38, #27) and adds, since `v0.2.1`, the vault's refusal of a DEK
the file's slots do not wrap (#36) and vault transition tests from the Quint specs (#34, #35);
mobile has not taken it yet (M5 takes whatever tag follows A2).

### Next PRs

Adoption work (M*) and generators (G*) are PRs in Bramble (`starknet-innovation/mc-wallet`)
against `feature/mobile-wallet`, where `mobile/core` and the generators live (`main` has neither).
Ports (A*) and fixture copies (F*) are PRs here.

| PR | Repo, base | Needs | What it is | Described in |
| --- | --- | --- | --- | --- |
| M1 Krusty 0.11 → 2.0.0 (merged, Bramble #907) | Bramble, `feature/mobile-wallet` | nothing | pins, version string, one conformance test, `rust-gift-wrap.json` regenerated | 2.1 step 1 |
| M2 Take `wallet-core`: felt, Nostr, KMS (merged, Bramble #909) | same | M1, `v0.1.0` | adds the dependency; three modules become re-exports or glue; duplicate parity tests deleted | 2.1 step 3 |
| M3 Vault (merged, Bramble #911) | same | M2 | `vault.rs`, `container.rs` and `ClockReading` from the core; `HostClock` and `files.rs` adapt | 2.2 |
| M4 Send (merged, Bramble #916) | same | M2, `v0.2.1` | `rpc.rs` and the `send/` mechanism from the core; `TransferPlan` wraps `Plan` | 2.3 |
| G1 Viewing public key vector (open, Bramble #919) | same, stacked on G3 | nothing | one field in the privacy generator | Golden tests |
| G2 Multisig coordination generator (open, Bramble #920) | same, stacked on G1 | `main` merged in | three new fixtures, `nostr-coordination-parity-{rules,relays,paging}.json`, for the 2.5 rules | Golden tests |
| F1, F2 Fixture copies (open, #42) | here, stacked on A2 | G1, G2 | files copied unchanged, sha256 rows and the Bramble commit line updated, the test that reads them | Golden tests |
| G3 Recovery scan generator (open, Bramble #918) | Bramble, `feature/mobile-wallet` | nothing | `accounts-parity-scan.json` and `accounts-parity-scan-signing.json`: shapes, creatable and keep rules, scan target, signing verdicts, registered elsewhere | Golden tests |
| F3 Catalog and scan copies (open, #40) | here | G3 | both `accounts-parity-scan*.json` and `generated/accounts.json` copied as fixtures; every other fixture re-copied at the same Bramble commit | 2.2, Accounts |
| A1 Account addresses (done, #39) | here | nothing | `Ladder`, `AccountFamily`, `family_address` in `wallet-core`; tested over both families fixtures | 2.2, Accounts |
| A2 Recovery scan (open, #41) | here | A1, F3 | `accounts::{scan, advisory}`, `rpc::Request`; tested over the scan fixture and mobile's canned-node cases | 2.2, Accounts |
| M5 Accounts (open, Bramble #921) | Bramble, `feature/mobile-wallet`, stacked on G2 | A2 and its tag (a `rev` until then) | `kms_accounts.rs` and `recovery_advisory.rs` go; `recovery_scan.rs` becomes a wrapper | 2.2, Accounts |
| 2.4a Shims (open, #43) | here, stacked on F1/F2 | nothing | `js/*` and `history::time` copied; `js_trim` and `short_label` move to `js::num` | 2.4 |
| M6 Shims (open, Bramble #922) | Bramble, stacked on M5 | 2.4a | `js/` and `history/time.rs` deleted, re-exported from the core | 2.4 |
| 2.4b Portfolio (open, #44) | here, stacked on 2.4a | 2.4a | `portfolio/*`, `catalog.rs`, account ids; three parity suites over `portfolio-parity.jsonl` | 2.4 |
| M7 Portfolio (open, Bramble #923) | Bramble, stacked on M6 | 2.4b | `portfolio/` becomes FFI wrappers and copy over the core; `networks.rs` builds the core's catalog records | 2.4 |
| 2.4c History and activity (open, #45) | here, stacked on 2.4b | 2.4b | `history/*`, `activity/*`, `Storage::rename`; `history_parity.rs`, `row_parity.rs` and the cases 2.3 and 2.4b left behind | 2.4 |
| M8 History and activity (open, Bramble #924) | Bramble, stacked on M7 | 2.4c | `history/` and `activity/` become FFI wrappers, copy and the file lock over the core | 2.4 |
| 2.4d Privacy (open, #46) | here, stacked on 2.4c | 2.4c, F1 | `privacy/*` with `PrivacyHost`; the privacy parity and plan suites | 2.4 |
| M9 Privacy (open, Bramble #925) | Bramble, stacked on M8 | 2.4d | `privacy/` becomes the FFI object, the routing loader and the `PrivacyHost` over the session | 2.4 |
| 2.5a Multisig account (open, #47) | here, stacked on 2.4d | 2.4d | `accounts::{multisig, multisig_records, multisig_read, multisig_declared}`, `js::felt::lenient_hex_felt`; `multisig_parity.rs` over `multisig.json` copied as a fixture | 2.5 |
| M10 Multisig account (open, Bramble #926) | Bramble, stacked on M9 | 2.5a | `argent_multisig.rs`, `multisig_rpc.rs` and `class_declared.rs` become wrappers passing Bramble's class; `multisig_store.rs` keeps the file; mobile stops naming Krusty outside `kms.rs` | 2.5 |

Every adoption PR for what is ported has merged, so by the Process rule the next port may start.
2.2 Accounts goes first: it is the unfinished half of a step, it is smaller than any part of 2.4,
and it has to land before 2.7 (see the Krusty constraint under Unassigned modules). A1 and G3 can
start now and in parallel; A2 waits for F3. G1 and G2 can also start now (`main` was merged into
the branch by Bramble #908); G2 is the long pole for 2.5. #908 also regenerated four fixtures
(`extension-transactions-copy.json`, `extension-privacy-copy.json`, `extension-privacy-rules.json`,
`portfolio-parity.jsonl`), so the copies here were behind Bramble; F3 re-copied every file at one
Bramble commit (G3's, `860a90ec`), which brought those four up to date. Every test here passes
over the re-copied files with no change to the core.

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
| Later phases: vault container (envelope v3), storage and clock traits, sans-IO RPC, transaction hashing, transfer planning, portfolio/history/privacy logic, Nostr relay paging and gift-wrap batch checks | Later phases: v12 vault schema and migrations, UniFFI foreign traits adapting to `wallet-core`'s, catalogs, multisig coordination policy (identifier, relays, sync window, who counts as a co-signer), iOS/extension-only assets |

The resource bounds (mnemonic, passphrase, Nostr event and data sizes) are Bramble's numbers, kept in
`wallet-core` because they protect against oversized input rather than express product taste.

`bramble-core` keeps the crate, lib and UniFFI module names mobile already uses (`bramble_core`,
`BrambleCore`, `org.bramble.core`), so the last mobile step (2.7) swaps the dependency without
touching Swift or Kotlin imports. A second wallet would add its own `<wallet>-core` crate the same
way, or use `wallet-core` from Rust directly.

## Layout

```
Cargo.toml                 workspace; release profile (lto, panic=abort, strip)
rust-toolchain.toml        pinned channel, wasm32-unknown-unknown target
.cargo/config.toml         getrandom backend cfg for wasm32
deny.toml                  licences, advisories, sources
.gitattributes             golden fixtures checked out byte-for-byte
AGENTS.md                  contributor and agent rules (CLAUDE.md points to it)
tools/check.sh             the one check entrypoint: CI, hooks and agents run its modes
.githooks/                 pre-commit (quick), pre-push (lint + test)
crates/wallet-core/        generic: package starknet-wallet-core, lib starknet_wallet_core
  src/lib.rs               Error, KRUSTY_VERSION, re-exports
  src/felt.rs              strict lowercase-hex felt grammar
  src/kms.rs               key custody over Krusty, account keys per ladder
  src/accounts/            ladders, account families, family addresses and constructor calldata
  src/nostr.rs             coordination identity, NIP-59 wrap/open
  src/envelope.rs          the extension's EncryptedEnvelope v3 (PBKDF2-SHA-256, AES-256-GCM)
  src/container.rs         vault container: DEK, passphrase and biometric slots, sealed payload
  src/host.rs              host-provided Storage and Clock traits, ClockReading
  src/rpc.rs               sans-IO JSON-RPC: envelope, chain id, balanceOf, amounts, u256 text
  src/send/                transfers: calls, fees, judge, rpc_bodies, sign (v3 hashes), u256, and
                           the sans-IO Plan with its PlanHost
  tests/vault_*transitions.rs  vault transitions from the Quint vault spec
crates/bramble-core/       Bramble: lib bramble_core, the cdylib/staticlib/wasm32 module
  src/lib.rs               UniFFI scaffolding, CoreError, core_info(), Bramble constants, exports
  src/bin/uniffi-bindgen.rs  Swift/Kotlin generator (`bindgen` feature)
  uniffi.toml              Swift/Kotlin module names, as mobile's
  tests/parity.rs          extension fixtures and known-answer vectors
  tests/vault_parity.rs    envelope and DEK slot the extension sealed
  tests/accounts_parity.rs  every family, ladder and class the extension derives
  tests/transactions_*_parity.rs  send rules; every body, hash and signing; the plan over every capture
  tests/fixtures.rs        holds tests/fixtures/ to its README manifest (sha256 per file)
  tests/fixtures/          copied from Bramble; README is the manifest
js/                        npm package @starknet-innovation/bramble-core: ubrn config,
                           entrypoint, smoke test over the built wasm
.github/workflows/         ci.yml (rust + wasm jobs), release.yml (tag → npm, with provenance),
                           publish-crates.yml (tag → crates.io, starknet-wallet-core then
                           bramble-core),
                           advisories.yml (weekly `cargo deny check advisories` on main)
quint-specs/               Quint models: vault, send, the bramble-core FFI
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
1. **Golden suite** (done): every fixture the extension generates for mobile, 22 files from
   Bramble `0a4bd84c`, copied into `crates/bramble-core/tests/fixtures/` under the manifest
   described in Golden tests and hash-checked by `crates/bramble-core/tests/fixtures.rs`. The
   hand-cut `nostr-parity.json` subset gave way to the whole `nostr-multisig-parity.json`.
   Bramble writes them into `mobile/core/tests/fixtures/` with `bun run mobile:parity`, which also
   runs the transaction and privacy generators:
   - KMS, accounts and a sealed envelope: `extension-parity.json`, `accounts-parity-*.json`.
   - Nostr multisig: `nostr-multisig-parity.json`.
   - Portfolio, tokens, history rows: `portfolio-parity.jsonl`.
   - Privacy: `extension-privacy-*.json`.
   - Transactions: `extension-transactions-*.json`.

   Bramble's `mobile/core/tests/fixtures/` also holds fixtures that are *not* extension output;
   they are not golden and were not copied: `vault-v1.json`, `vault-v2-*.json`,
   `vault-v3-container.json` and `migration-v1-to-v5.golden.json` were written by earlier mobile
   cores and pin mobile's own migration chain; `ui-coordination.json` drives the iOS UI suite;
   `rust-gift-wrap.json` runs the other way (Rust writes it, the extension opens it). Those move
   with their modules as ordinary regression fixtures. The golden fixtures for modules not yet
   ported sit in this repo unused until their phase: `portfolio-parity.jsonl`,
   `extension-privacy-*.json`, `extension-transactions-observation.json` (2.4), and
   `accounts-parity-recovery.json` and `accounts-parity-rules.json` (Bramble's wording and rules,
   2.7). Where the
   extension has behaviour mobile ports but no generator covers it, the generator is added in
   Bramble first.
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
      the whole crate and then takes `kms.rs`, `nostr.rs` and `felt.rs` from `wallet-core`.
      Mobile has no users yet, so Krusty 2.0.0 is taken without a separate diff review: the
      golden fixtures are the check that mobile derives and signs as the extension does.
      Steps, in order:
      1. **Krusty bump in mobile** (one PR on `feature/mobile-wallet`; needs nothing from this
         repo). Tried on a copy of `0a4bd84c`: with `krusty-kms` and `krusty-kms-common` at
         `=2.0.0` and `bip39` at `=3.0.0` the crate builds with no source change, clippy is
         clean, and 369 tests pass (3 network tests ignored) with every fixture byte-identical.
         The PR:
         - `Cargo.toml`: `krusty-kms` and `krusty-kms-common` `=0.11.0` → `=2.0.0`; `bip39`
           `=2.2.2` → `=3.0.0`, the version Krusty 2.0.0 uses, so the lockfile holds one copy
           (`ui.rs`'s word assist needs no change). `starknet-types-core` stays `=0.2.4`.
           `cargo tree -d` lists no second `krusty-kms`, `bip39` or `starknet-types-core`. The
           comment above the pins names `mobile-core-pins.test.ts`, which does not exist; it
           should name `mobile-generated.test.ts`.
         - `src/lib.rs`: `krusty_version: "2.0.0"`.
         - Bramble's `tests/conformance/mobile-generated.test.ts` ("the Rust core pins the same
           Krusty release as the extension"): drop the npm-pin equality, which the extension
           keeps at 0.11 until 3.1, and check instead that `Cargo.toml`'s pin equals
           `lib.rs`'s `krusty_version`. Step 3 points it at `wallet_core::KRUSTY_VERSION`.
         - Comments and docs naming 0.11.0 as mobile's Krusty: `send/mod.rs`, `send/sign.rs`,
           `privacy/mod.rs`, `argent_multisig.rs`, `kms.rs`, `mobile/SCOPE.md`; and
           `scripts/generate-mobile-parity-vectors.ts`, whose header says to bump Krusty in
           `package.json` and `Cargo.toml` together (now: re-run after bumping either).
         - `rust-gift-wrap.json` regenerated (`BRAMBLE_WRITE_FIXTURES=1 cargo test --test
           nostr_multisig_parity`), so `mobile-nostr-parity.test.ts` opens a wrap Krusty 2.0
           made with the extension's 0.11. Without it the test opens the old 0.11 wrap and
           proves nothing about 2.0. It is a regression fixture, so regenerating it is allowed.
           Tried: the regenerated wrap opens in the extension's test, and a tampered one fails.
         - Checks: `cargo test`, clippy with `-D warnings`, `bun run check` in Bramble, and an
           iOS and Android build. Swift and Kotlin bindings regenerate with no diff.
         - If a golden fixture ever fails here, Krusty changed behaviour: stop and report it
           upstream; never edit the fixture.
      2. **Nostr parity gaps in `wallet-core`** (done), found by comparing mobile's `nostr.rs`
         and `kms.rs` with the core:
         - `open_nostr_event` opens a gift wrap from a parsed `NostrEvent`, re-serialised as the
           extension's `JSON.stringify(event)` and re-checked against the event grammar.
           `open_nostr_application_data` (event JSON) is now a wrapper over it.
         - `stark_public_key` takes a `SecretFelt`, for mobile's `scoped_viewing_public_key`
           (recovery compares it with the key a privacy pool holds).
         - Parity tests: the parsed open against `nostr-multisig-parity.json`'s gift wrap and
           its refused variants; the public key against `extension-parity.json`'s `publicKeyX`.
           Still open, in Bramble: the privacy fixtures hold no viewing *public* keys, so the
           composed `previewStrk20ViewingPublicKey` has no golden vector. Add one to the privacy
           generator, and test against it once it is copied here.
      3. **Mobile takes the core** (M2) at `v0.1.0`, tagged at `35abc8b`. That tag makes a
         GitHub release only: `release.yml` skips npm while `js/package.json` is private, which
         it stays until Phase 3.1. Mobile pins `wallet-core` by `git` and `tag`
         (`wallet-core = { git = "https://github.com/starknet-innovation/wallet-core", tag =
         "v0.1.0" }`); `Cargo.lock` records the commit. In `Cargo.toml`, `sha2` moves `=0.11.0`
         → `=0.11.1`: the core pins `=0.11.1`, and two exact pins in one semver range do not
         resolve. Every other shared pin already agrees. `krusty-kms` and `krusty-kms-common`
         stay direct dependencies at the core's pin, because `kms_accounts.rs`,
         `argent_multisig.rs`, `account_catalog.rs`, `networks.rs` and `send/` still name
         Krusty; `cargo tree -d` must list neither twice. Mobile's `CoreError` gains
         `From<wallet_core::Error>`, variant for variant as `bramble-core`'s here. From the tag
         after `v0.1.0` the package is `starknet-wallet-core`; mobile keeps every `wallet_core::`
         path with `wallet-core = { package = "starknet-wallet-core", git = …, tag = … }`.
         `felt.rs` becomes `pub(crate) use wallet_core::felt;`, `nostr.rs` maps onto the core
         (`ApplicationData` → `NostrApplicationData`), and `kms.rs` keeps only mobile's glue
         (`Ladder` and its serde names, `normalise_phrase`, error mapping, `OZ_ACCOUNT_VERSION`)
         until 2.7. `krusty_version` reads `wallet_core::KRUSTY_VERSION`. The KMS half of
         mobile's `parity.rs` and the key and gift-wrap tests in `nostr_multisig_parity.rs`
         are deleted, since `crates/bramble-core/tests/parity.rs` covers them;
         `privacy_key_boundary.rs` is pointed at where derivation now lives. By name: from
         `parity.rs`, `accounts_and_signatures_match_the_extension`; from
         `nostr_multisig_parity.rs`, `coordination_keys_match_the_extension` and
         `opens_the_extension_gift_wrap_and_refuses_its_variants`. Its
         `rust_gift_wraps_round_trip_and_the_committed_one_still_opens` stays: it writes
         `rust-gift-wrap.json`, which the extension's `mobile-nostr-parity.test.ts` opens.
         `felt.rs` here and in mobile export the same functions, so its 24 importers do not
         change. The conformance test from step 1 now checks that `Cargo.lock` resolves one
         `krusty-kms` equal to the `Cargo.toml` pin and that `lib.rs` reads
         `wallet_core::KRUSTY_VERSION`. Checks as step 1; the Swift and Kotlin bindings
         regenerate with no diff, because nothing exported changes.

      What step 3 needs from this repo:
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
      - (done) The Nostr parity gaps in step 2.
   2. **Vault and accounts**: `container.rs` (envelope v3, byte-compatible) and the vault
      mechanics into `wallet-core`, with plain Rust `Storage` and `Clock` traits; mobile's
      `files.rs` implements `Storage`. The v12 schema and migrations are Bramble's and stay in
      `mobile/core` until 2.7; the mobile vault migrates into v12 here.
      In `wallet-core`: `envelope.rs` (mobile's `vault.rs`), `container.rs`, and `host.rs`
      (`Storage`, `Clock`, mobile's `ClockReading`). The vault version is an argument (mobile's
      `VAULT_VERSION` stays Bramble's), and so is the passphrase rule: `SealingKey::create` no
      longer calls `check_password`, so mobile checks first. Mobile's `HostClock` adapts to
      `Clock`, and `migrations_tests.rs` uses `SealingKey::from_parts` where it used `fixed`.

      Mobile's side (M3):
      - `vault.rs` keeps only Bramble's passphrase rule (`check_password`,
        `MIN_PASSWORD_LENGTH`) and re-exports `wallet_core::envelope`. `container.rs` becomes a
        re-export of `wallet_core::container`, with `parse(text)` → `parse(text, VAULT_VERSION)`.
      - `clock.rs` keeps the UniFFI `HostClock` foreign trait and implements
        `wallet_core::host::Clock` for it; `ClockReading` comes from the core. `files.rs` gains
        a `Storage` over `write_atomic` for the container's `read` and `write`.
      - `SealingKey::create` no longer checks the passphrase. Its one production caller,
        `wallet.rs`'s `create_vault`, is reached only from `create_wallet_on_network` and
        `import_wallet`, which both call `check_password` first. The check moves into
        `create_vault`, so a third caller cannot skip it; `testing.rs`'s two hooks call it
        too. A test proves a passphrase under the minimum is still refused on create and on
        import.
      - `SealingKey::fixed` → `from_parts` in `container.rs`'s tests and `migrations_tests.rs`.
      - `parity.rs` loses `opens_an_envelope_the_extension_sealed` and
        `opens_a_dek_slot_the_extension_sealed` (`vault_parity.rs` here covers them) and, after
        M2, is empty and deleted. `vault-v1.json`, `vault-v2-*.json`, `vault-v3-container.json`
        and `migration-v1-to-v5.golden.json` stay and must pass untouched: they are the proof
        the swap changed no byte on disk.
      - Not in M3: the mobile vault → v12 migration.

      **Accounts** (A1, A2 here; M5 in mobile). Mobile's account mechanism is `kms_accounts.rs`
      (198 lines: family addresses through Krusty), `recovery_scan.rs` (499: the sans-IO
      discovery scan), `recovery_advisory.rs` (402: the scan's signer and privacy-pool reads)
      and the rule half of `recovery.rs` (499, of which scan shapes and the keep rule are
      mechanism and the rest is row wording and restore). It must move before 2.7 because
      `kms_accounts.rs` names Krusty and `bramble-core` may not. The split, by the
      argument-or-constant rule:

      | Into `wallet-core` (`src/accounts/`) | Stays Bramble's (mobile until 2.7, then `bramble-core`) |
      | --- | --- |
      | `Ladder` (`Account`, `Address`, `ArgentX`) with `kind()`, `vendor(family)` and `fits(family)`; `kms::account_key(phrase, passphrase, ladder, index)` (mobile's `kms::private_key`) and its public key | `Ladder`'s serde names (the vault spells them) and UniFFI derive, through `#[uniffi::remote]` |
      | `AccountFamily` with the extension's wire spelling | `accountFamilyName` ("Bramble", "Ready", "Braavos") |
      | `family_address(family, class_hash, argent_shape, salt, public_key)` and `constructor_calldata(..)` (the same placement, for a deployment): class and salt **required**; Krusty builds the calldata and address; an Argent class Krusty lists must have the layout the caller names, and one it does not list is refused unless the caller passes `Unlisted(shape)` | `resolve_class` (OZ at `OZ_ACCOUNT_VERSION`, `argent_default`, Braavos has none), salt `0x0` for OZ and the public key otherwise; Xverse as the one `Unlisted` class |
      | `accounts::scan::Scan`, one per (fingerprint, network), `next(phrase, passphrase, &ScanConfig, manual)` / `submit(responses)`; judging reuses `send::judge::answer` rather than a third reply parser; `MAX_CANDIDATES` (an input bound, like the resource bounds) | `ScanConfig`'s contents: shapes in order, each with its `creatable` flag (`isCreatableCandidate`), `RECOVERY_SCAN_LIMITS`, the tokens whose balance counts as funded, the privacy pool per network |
      | `accounts::advisory`: which reads a live class needs, `Restriction` (Guardian, OwnerChanged, OwnerType, Release, StrongSigner) from the reads and the block time, `registered_elsewhere` (the own viewing key derived in the core); "signs for" reads the same `send::Authorization` the plan takes, so the class lists are not passed twice; the pool's `get_public_key` selector beside `plan_probe`'s | `RESTRICTION_COPY`, the privacy-pool note, scan status lines |
      | `rpc::Request { id, body, max_response_bytes, optional }` and `rpc::Response` (mobile's `rpc_request.rs` without UniFFI) | their UniFFI records, through `#[uniffi::remote]` |
      | | `RecoveryCandidate` and its wording (`candidateName`, `candidateTitle`, hints, selection keys), `derive_selections` (restore checks over vault records), `account_store.rs` |

      Each `wallet-core` file stays under 500 lines: `accounts/mod.rs` (ladders, families,
      addresses), `accounts/scan.rs`, `accounts/advisory.rs`, with tests beside them.

      - **A1, account addresses** (needs nothing). `Ladder`, `AccountFamily`, `account_key` and
        `family_address`. Test `accounts_parity.rs` here: every vector in both
        `accounts-parity-families-*.json` derives the extension's public key and address on its
        ladder, class and salt (mobile's `every_family_and_class_derives_the_extensions_address`
        and `split_family_fixtures_agree_on_the_phrase`), and `kms_accounts.rs`'s Xverse test
        (it deploys with Argent 0.4 calldata at its own address); its class-rules test stays in
        mobile with `resolve_class`. The test
        reads the class table from `generated/accounts.json` once F3 copies it; until then it
        holds the extension's layouts for the four Argent classes the families fixtures name.
        `parity.rs`'s `every_family_ladder_matches_the_extension` stays as it is: it checks the
        FFI's raw-key exports, which `accounts_parity.rs` does not reach. Done: `Ladder`,
        `AccountFamily`, `ArgentLayout` and `ArgentShape`, `family_address` and
        `constructor_calldata` in `accounts/mod.rs`; `kms::{account_key, account_public_key}`.
      - **A2, recovery scan** (needs A1, F3). `accounts::{scan, advisory}` and `rpc::Request`.
        Tests: `accounts_scan_parity.rs` over `accounts-parity-scan.json` (G3: shapes, creatable
        rule, scan target, keep rule, signing verdicts, registered elsewhere), and
        `accounts_scan_mechanism.rs`, mobile's `tests/recovery.rs` scan cases against a canned
        node (a quiet wallet, activity across families, a pause and retry, exact ids, a manual
        index, a network reset) plus `recovery_scan.rs`'s and `recovery_advisory.rs`'s unit tests.
        Tag after A2. Done, with these differences from the text above: `submit` takes the
        phrase, passphrase and config again (mobile read its own session; the scan stores no
        secret) and refuses another phrase or network; the config, candidates, keep rule and
        gap limit are in a third file, `accounts/scan_rules.rs`; the scan keeps mobile's
        class-hash judging as `scan::judge_class_hash`, built on `send::judge::answer`, because
        `send::judge::class_hash` reads any result as deployed and does not return the class;
        `ScanConfig` also carries the network, the reserved indexes and a `SaltRule` per shape;
        limits that would underflow or pass `MAX_CANDIDATES` in one batch are refused; a Ready
        shape under a BIP-39 passphrase fails the batch closed. The two Krusty-against-catalog
        tests are in `accounts_scan_parity.rs`. One fixture case is not reproduced exactly: a
        pool key spelled with more than 64 hex digits is read by the extension (`BigInt`) and
        counts as no answer here (the felt grammar); the verdicts in the fixture agree.
      - **M5, mobile takes it** (Bramble, needs A2's tag). `kms_accounts.rs` and
        `recovery_advisory.rs` are deleted; `send/wallet_ext.rs`'s `deployment` takes its Argent
        calldata from `constructor_calldata`; `recovery_scan.rs` keeps the UniFFI step record and
        status lines and builds `ScanConfig` from `account_catalog`, `networks` and the privacy
        route; `recovery.rs` keeps wording and `derive_selections`, whose `record_at` calls the
        core's `family_address`; `argent_preset` and `account_catalog`'s Krusty import go. The core
        places a Braavos class only if Krusty lists it as a deployment class
        (`BraavosAccount::try_with_class_hash`), and an Argent class only on a layout that agrees
        with Krusty's; `resolve_class` still decides which of those Bramble offers, a check
        mobile's `family_address` made itself. Its two
        Krusty tests (`krusty_constructors_agree_with_the_extension_table`,
        `open_zeppelin_class_is_krustys_on_every_network`) check Krusty against the catalog, so
        they move here into A2's tests, over F3's `accounts.json`.
        `accounts_parity.rs` loses the two family tests (here now) and keeps
        `discovery_rows_are_worded_like_the_extension`, `advisory_notes_are_worded_like_the_extension`
        and the names, labels and groups tests, which are Bramble's copy and rules and move in
        2.7. So `accounts-parity-recovery.json` and `accounts-parity-rules.json` stay unread
        here until 2.7. `tests/recovery.rs`'s restore cases stay: they exercise the vault write.
        Checks as M4; the Swift and Kotlin bindings regenerate with no diff.

      After M5, mobile names Krusty (`krusty_kms_common`) only for `ChainId` and `KmsError` in
      `kms.rs` and `networks.rs`, which 2.6 replaces with `wallet_core::NetworkId` and
      `wallet_core::Error`, and in `argent_multisig.rs` (2.5). Since M7 (2.4b) `networks.rs` uses the
      core's `NetworkId` and no longer names Krusty; `kms.rs` and `argent_multisig.rs` remain.
   3. **Sans-IO transactions**: `rpc.rs`, `send/*` and transaction hashing, Argent and Braavos
      paths, into `wallet-core`; Bramble's defaults (fee margins, which flows it offers) stay
      with the Bramble crate. Done:
      - `rpc.rs` whole, and from `send/` the mechanism modules `u256`, `fees`, `judge`,
        `rpc_bodies`, `sign` and `calls`, with the types they use folded into them. The chain id
        comes from `NetworkId::chain_id()` (Krusty) rather than mobile's network catalog;
        `js_trim` and `short_label` live in `send::calls` until 2.4 brings the `js/*` shims;
        `sign::Deployment::address()` is the counterfactual-address check mobile made with
        Krusty directly. `CoreError` gains `Rpc`.
      - The state machine (`plan*.rs`) as `send::plan::Plan`, step for step. What tied it to
        mobile's `WalletCore` comes in from outside instead: `PlanHost` (lent to each
        `advance`) owns the session check, signing (`sign(hash, renew)`), the activity log
        (`record_pending`, `observe`; a signed write's record is never dropped, whatever the
        node answers) and explorer URLs; Bramble's account catalog rides in `Authorization`
        (Argent classes and their shapes, Braavos signing classes and implementation), as the
        Phase 2.6 rule asks; and its numbers are a `Policy`.
        `bramble-core` holds them: `FEE_MARGIN_PERCENT` (50), `REVIEWED_FEE_HEADROOM` (3/2),
        `CONFIRMATION_DELAYS_MS`, together as `SEND_POLICY`, each checked against the rules
        fixture. `Plan` is `&mut self`; the FFI object's mutex and its poison-closes rule stay
        with the wrapper. What the plan signs it derives itself: `Transfer::new` validates the
        request and builds the calldata, summary and verifier flags (mobile's
        `prepare_transfer` mechanism), and `Plan::new` drops a deployment that does not
        reproduce the account's address. A zero `Headroom` denominator fails closed.
      - Tests, all over fixtures already here: `transactions_rules_parity.rs` (mobile's
        `send/fixture_tests.rs`, less explorer URLs and the activity store, which move with
        2.4); `transactions_wire_parity.rs` (every request body in the OpenZeppelin, Ready and
        Braavos captures rebuilt, every signed hash recomputed from its own fields, its bounds
        re-derived from a quoted estimate, every hash the extension signed accounted for);
        `transactions_plan_parity.rs` and `transactions_refusal_parity.rs` (mobile's three
        `transactions_*_parity.rs`: the plan drives all 20 captures and 18 refusals with a test
        `PlanHost` on the published phrase, in `tests/plan_support`, checking every hash is signed
        in the extension's order, plus checkpoints, a lock after review and a fee past the
        headroom); `transactions_plan_mechanism.rs` (mobile's `transactions_plan.rs` against its
        canned node: wrong chain, out-of-order replies, node errors, write transport failure,
        hash mismatch, poisoned recipient, the 200-poll deployment wait, unknown deployment
        state, a missing bulk-estimate entry).

      Mobile's side (M4): take `v0.2.1` (`wallet-core = { package = "starknet-wallet-core", git =
      …, tag = "v0.2.1" }`, with `zeroize` `=1.9.0` → `=1.9.1` and `serde_json` `=1.0.151` →
      `=1.0.154`, the core's pins), delete `rpc.rs` and those `send/` files and `use
      wallet_core::{rpc, send::*}`. Mobile's plan still drops the pending record of a write the
      node errored on (`store::discard` in `plan_deploy.rs` and `plan_submit.rs`); the core's
      does not (#25), so those calls go, the record stays until `observe` settles it, and the
      failure carries the hash. Mobile's tests that expect the record gone change with it.
      `recovery_advisory.rs` reads the same account state outside a plan; its imports from
      mobile's `send::plan_probe` become `wallet_core::send::plan_probe`, except
      `ARGENT_SIGNER_ARRAY`: it is a catalog string the core does not hold (`ArgentClass` carries
      `signer_array` as a flag), so it moves from mobile's `plan_probe.rs` into `account_catalog`.
      `TransferPlan` becomes a UniFFI object around `Mutex<Plan>` holding `Weak<WalletCore>` and
      the session generation; its `PlanHost` maps onto `plan_sign`,
      `session_generation`, `activity::store` (`PendingRecord` → `Entry`, direction "out",
      status "pending") and `history::explorer`; `sign` must refuse once the generation moved,
      as `plan_sign` does. `wallet_ext.rs` keeps preparing plans (vault records, contacts,
      catalogs), building `Authorization` from `account_catalog` and calling `Transfer::new`.
      Records keep their UniFFI
      derives through `#[uniffi::remote]` until 2.7. Mobile's wallet-level tests
      (`transactions_lifecycle.rs`, `transactions_block_pin.rs`, and the contact, tracked-token
      and locked-prepare cases of `transactions_plan.rs`) stay there: they exercise its locks,
      store and address book through that wrapper.
   4. **Portfolio, history, privacy reads**: `portfolio/*`, `history/*`, `activity/*`,
      `privacy/*` into `wallet-core`, with the `js/*` JavaScript-semantics shims beside whichever
      code calls them; the iOS `include_str!` in `history/appearance.rs` moves to the Bramble
      crate's `generated/`. At about 11,200 lines this is the largest step, larger than 2.1–2.3
      together, so it goes in four parts, each a port here and an M PR in mobile, in the order
      the imports allow:
      1. **2.4a Shims** (1,638, plus `history/time.rs`, 196): `js/*` (Big, Number, JSON, regex)
         into `wallet-core::js`, with `send::calls::{js_trim, short_label}` moving beside them,
         and `history/time.rs` (no crate imports), whose `format_saved_data_label`
         `portfolio/snapshots.rs` uses. Unit tests only; their golden coverage comes through the
         later parts. Done (#43): the eight `js` files and `time.rs` are byte copies of
         mobile's; `short_label` joined `js::num`, and `send::calls` re-exports both names. M6
         (Bramble #922) makes mobile's `js` and `history::time` re-exports and points its
         private `js_trim` and `short_label` copies at the core.
      2. **2.4b Portfolio** (2,958): `portfolio/*`, plus `history/explorer.rs`'s
         `is_account_id_for_network`, which `snapshots.rs` uses and which itself imports
         `portfolio::model`, so it cannot go earlier. Oracle `portfolio-parity.jsonl`; tests
         `portfolio_parity.rs`, `token_parity.rs`, `portfolio_state.rs`. It reads `networks` six
         times: this is where the 2.6 rule first applies, so define the network and token
         records in `wallet-core` here, once, as arguments, and every later part reuses them.
         Done (#44): `catalog.rs` holds `NetworkInfo`, `TokenInfo`, `TokenRegistry` and the
         borrowed `Catalogs`, plain records with no serde (Bramble parses its own JSON), and
         `NetworkId` gains the extension's spelling (`catalog_id`). `PriceBook`,
         `BalanceSnapshots` and `TokenPreferences` are plain values with `&mut self`; mobile
         keeps the FFI objects as wrappers around a lock. `strk_address` answers `None` where
         mobile panicked on a catalog without STRK. Two functions that only return a sentence
         (`token_network_error`, `balance_failure_copy`) stay Bramble's. `canonical_account_id`
         came with `is_account_id_for_network`. `networks.json` and `token-registry.json` are
         copied as fixtures for the tests. `portfolio_state.rs` moved its portfolio cases; its
         history and activity cases follow in 2.4c. Still in `wallet-core` and candidates for
         2.6: the `bramble.mobile.portfolio.v1` schema string, the curated USDC.e alias, the
         CoinGecko base URL and the rule that only mainnet has fiat prices.
      3. **2.4c History and activity** (3,565): `history/*` and `activity/*`, which import each
         other. Oracle `portfolio-parity.jsonl` rows and `extension-transactions-observation.json`;
         tests `history_parity.rs`, `row_parity.rs`, and the explorer-URL and activity-store
         cases 2.3 left out of `transactions_rules_parity.rs`. Done (#45): `history/*` and
         `activity/*` as plain Rust. The activity log is `ActivityLog` over `host::Storage`,
         which gains one method, `rename`, for setting an unreadable file aside (the vault never
         calls it; it is a breaking change for anyone implementing `Storage`). The process-wide
         file lock becomes `&mut self`: the wallet keeps one log per file and serialises calls.
         The 4 MiB bound is checked on the bytes read, since `Storage` reports no size, and a
         read that errors is set aside like a corrupt file. `ActivityBook` and `HistoryScan`
         are plain values. `ActivityKind` is defined in `history::activity` (it was only in
         mobile's send wrapper). Explorer links take the network catalog and answer `None` for
         a network it lacks. Staying Bramble's: the empty-state and unavailable notices, the
         failure-notice sentence (with its fixture loops, in mobile's tests), deleting the log
         with the wallet, and the iOS icon-name test. Still in `wallet-core` and candidates for
         2.6: the Starkscan API base URL, "Untracked token", and row hints that name Bramble.
      4. **2.4d Privacy** (3,007): `privacy/*`. `plan.rs` and `plan_steps.rs` hold `WalletCore`
         as `send/plan*.rs` did, so they take a host trait the way `Plan` takes `PlanHost`;
         `wallet_ext.rs` stays in mobile. Oracle `extension-privacy-*.json` and G1's vector;
         tests `privacy_parity.rs`, `privacy_plan.rs`. Done (#46): `privacy/*` as plain
         Rust. `PrivacyReadPlan` is a `&mut self` state machine lent a `PrivacyHost` on every
         `advance`, whose one method returns the scoped viewing key as a `SecretFelt` or `None`
         (locked, or the account gone); the wallet keeps the session, the generation check and
         the record lookup. The key is held in `SecretFelt`, written into one zeroizing body,
         and the body stays zeroizing until the host takes it (mobile copied it out at the
         step). Routing and limits are plain records (`privacy::config::{Route, Limits}`); the
         JSON loader and the test override stay Bramble's. Two rules are enforced in the core
         as well as in the wallet's loader, because the route is now an argument: an OHTTP
         route is not configured, and the discovery URL must be an `https` service URL
         (`isServiceUrl`). `DeploymentState` is defined in `send::types` (it was only in
         mobile's send wrapper). `privacy/json.rs` stays beside `js::json`: it refuses
         duplicate keys and non-safe integers where `js::json` is last-wins. The parity suite
         is three files (`privacy_parity.rs`, `privacy_gates_parity.rs`, `privacy_plan.rs`)
         over `privacy.json` copied as a fixture.
   5. **Multisig coordination**: what sits around the Nostr wrap/open the core already has,
      and is today written twice (the extension's `background/multisig/multisig-coordination-ops.ts`,
      `multisig-nostr-publish.ts`, `multisig-sync-window.ts` and the paging half of
      `nostr-relay.ts`; mobile's `coordination.rs`, `nostr_relay.rs`, `ffi_nostr_multisig.rs`).
      Sans-IO: the host keeps the WebSockets and the address book and passes in what it read.
      The extension's TypeScript is the oracle *and* the shape: functions keep its names and
      arguments, so its later swap (3.4) replaces bodies, not call sites, and mobile adapts to
      them. A Bramble generator for these rules comes first (see Golden tests).
      - `wallet-core`: relay-URL grammar (`isNostrRelayUrl`), gift-wrap batch validation
        (`parsedGiftWraps`: kind 1059, `p` tag, no duplicate ids, batch bound), sync paging
        (unseen-first bounding, the `until` cursor, interleaving candidates across relays) and
        acknowledgement counting.
      - `bramble-core`: the `bramble.multisig` identifier, default relays and "one custom relay
        replaces the defaults", `MAX_SYNC_PAGES`, the sync window (30-day first lookback, 3-day
        incremental, retry cut-off), recipient selection and the sender rule (a saved contact on
        the network whose Stark key's Argent signer GUID is in the signer set).

      It goes in two parts. **2.5a, the multisig account** (done, #47): mobile's
      `argent_multisig.rs`, `multisig_rpc.rs`, `class_declared.rs` and the validators of
      `multisig_store.rs`, into `wallet-core`'s `accounts::{multisig, multisig_read,
      multisig_declared, multisig_records}`, so mobile names Krusty only in `kms.rs`. How it differs
      from the table above: the pinned class and its signer limit travel together as an argument,
      `ArgentMultisigClass` (no `bramble-core` constant yet: Bramble's value is
      `generated/multisig.json`, which stays mobile's until 2.6, and the tests read a fixture copy
      of it); the name-length limit is an argument to `is_record`; the reads are plain `&mut self`
      values; replies go through `send::judge::answer` and `judge::chain`, while the block pin,
      the class check (a zero class is unsupported, not undeployed) and code-only not-found
      matching keep mobile's judging, since `send::judge`'s would change those verdicts; the
      records keep serde (they are the extension's stored JSON shape); the observed-authority
      fallback and the registry file stay in mobile; `lenient_hex_felt` moved from mobile's
      `contact_card.rs` into `js::felt`. **2.5b, the coordination** (the rest of this section)
      follows.
   6. **Catalogs**: `generated/*.json` are Bramble's; `wallet-core` takes catalogs as arguments.
   7. **Swap**: what remains in `mobile/core` that is Bramble's but not mobile-only (constants,
      v12 schema and migrations, catalogs, the UniFFI surface, `HostStorage` and `HostClock` as
      `#[uniffi::export(with_foreign)]` traits adapting to `wallet-core`'s) moves into this
      repo's `bramble-core`. `mobile/core` is then replaced by a git dependency on it at a tag
      (same crate, lib and module names), Swift and Kotlin regenerate on UniFFI 0.29.5 → 0.31.2,
      and builds use `--no-default-features`.

      **The mobile vault → v12 migration** (proposed placement; it needs a named owner). Mobile
      writes vault version 5 (`migrations.rs`: its own chain, v1 → v5, inside the DEK container);
      the extension writes schema v12 inside a bare v3 envelope. The migration is a payload step,
      v5 → v12, added to mobile's chain once both schemas live in `bramble-core`, so it lands
      with 2.7, as the step's first PR, before the dependency swap. It is Bramble's (a schema),
      not `wallet-core`'s. It has no oracle yet: a generator (G4) must first seal a v12 vault in
      the extension for a fixed phrase and account set, so the test can migrate mobile's
      `vault-v3-container.json` chain forward and compare payloads. Which container holds the
      v12 payload is the separate Open question on the extension's container; this step does not
      depend on its answer, because it changes the payload, not the container.

   Mobile-only platform glue with no extension counterpart (biometric slots, file storage, app
   UI state) stays in the app, or in `bramble-core` behind mobile's build, unless the extension
   gains it.

   <a id="unassigned-modules"></a>**Where every mobile file goes.** All of `mobile/core/src` at
   `01bd6517`, 23,114 lines including inline tests, so no file falls into 2.7's "what remains"
   by omission. Placements not yet ported are proposals, confirmed when their step starts.

   | Step | Mobile files | Lines | Lands in |
   | --- | --- | --- | --- |
   | 2.1–2.3, done | `kms.rs`, `nostr.rs`, `vault.rs`, `container.rs`, `clock.rs`, `files.rs` (glue left after M2–M4) | 448 | glue; 2.7 moves what is Bramble's |
   | 2.3, done | `send/{mod, plan, types, wallet_ext, fixture_tests}.rs` (`TransferPlan` and its tests) | 1,122 | `bramble-core` at 2.7 |
   | 2.2 Accounts | `kms_accounts.rs`, `recovery_scan.rs`, `recovery_advisory.rs`, `recovery.rs` | 1,598 | split as 2.2 says |
   | 2.4a–d | `js/*`, `portfolio/*`, `portfolio_testing.rs`, `history/*`, `activity/*`, `privacy/*` | 11,168 | `wallet-core`; copy and assets in `bramble-core` |
   | 2.5 | `coordination.rs`, `nostr_relay.rs`, `ffi_coordination.rs`, `ffi_nostr_multisig.rs` (each with `_tests`), `multisig_constants.rs`, `argent_multisig.rs`, `multisig_rpc.rs`, `multisig_store.rs`, `class_declared.rs` | 3,417 | constructor layout, authority read, relay paging in `wallet-core`; registry, pinned class, relays, sync window in `bramble-core` |
   | 2.6 | `account_catalog.rs`, `networks.rs` (and `generated/*.json`) | 638 | `bramble-core`; `wallet-core` takes them as arguments |
   | 2.7 | `wallet.rs`, `wallet_accounts.rs`, `records.rs`, `testing.rs`, `lib.rs`, `rpc_request.rs` (the UniFFI object and records); `account_store.rs`, `migrations.rs`, `migrations_tests.rs` (vault schema); `contacts.rs`, `contact_card.rs`, `groups.rs`, `account_names.rs`, `avatar.rs`, `ui.rs` (extension product rules) | 4,151 | `bramble-core` |
   | undecided | `security.rs` (wrong-passphrase backoff) | 269 | see Open questions |
   | stays | `biometric.rs`, `backup.rs`, `public_store.rs`, `bin/uniffi-bindgen.rs` | 303 | mobile-only glue |

   2.7 is still about 5,700 lines (its own files plus the send wrapper and the glue). Its
   account rules and wording have golden fixtures already (`accounts-parity-rules.json`,
   `accounts-parity-recovery.json`); its UniFFI object has none, and needs none, since mobile's
   own tests come with it.

   One constraint orders this. `bramble-core` never names `krusty_kms`, and after M4 mobile
   still does in `kms_accounts.rs`, `account_catalog.rs` and `send/wallet_ext.rs` (2.2 Accounts
   removes all three), `kms.rs` and `networks.rs` (`ChainId`, `KmsError`; 2.6) and
   `argent_multisig.rs` (2.5). Each needs a home in `wallet-core` before 2.7.
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
      Migrations from schemas before v12 stay in TypeScript. See [Key exposure](#key-exposure).
   3. **Transactions, then reads**: `@bramble/transactions` drives the `TransferPlan` step loop;
      portfolio, history and privacy follow. Bramble's `scripts/mobile-assets.ts` renderers
      invert so TypeScript reads catalogs from the npm package.
   4. **Multisig coordination**: `multisig-coordination-ops.ts`, `multisig-nostr-publish.ts` and
      `multisig-sync-window.ts` call the core's functions from 2.5 under the same names;
      `nostr-relay.ts` keeps its sockets and loses its paging. Nostr keys still come from the
      vault (index `0, 0`, recovery-phrase wallets only), as today.

   The extension's TypeScript is the oracle until it is replaced, so each swap is proved by the
   golden suite passing through the extension's own conformance tests before the TypeScript goes.
4. **Later**: Ledger, dapp provider and permissions, STRK20 submission, swap, gasless, NFTs,
   allowances.

## Key exposure

How far raw private keys travel outside Rust, today and after Phase 3.2.

**Today (Bramble `e6485a7a`).** Krusty's wasm is stateless: keys cross it as hex strings in both
directions. The v12 vault stores each account's derived key (`signingKeys[].privateKey`) under
AES-256-GCM. `signHash` (`vault/src/service/service.ts`) decrypts the whole vault into JavaScript
objects on every signature, then passes the key string back to `kms.signStarkHash`. So every
signature puts every account's key into the JS heap as immutable strings that cannot be zeroed
and live until GC. What limits it: decrypted secrets are per-operation, not held across an unlock
(the grant cache keeps only public account records); the passphrase-derived AES key is a
non-extractable WebCrypto key dropped on lock; everything runs in the background service worker;
`storage.session` is restricted to trusted contexts where the browser allows it.

**`raw-keys` reproduces this model**, no worse and no better. Adopting the core with it on keeps
today's exposure; it is a bridge, not the target.

**Target (3.2, a precondition for dropping `raw-keys`, not a follow-up).**
- The core unlocks the vault and holds it: `unlock(vault_bytes, passphrase) → VaultSession`,
  with signing and derivation as session methods. Keys live in wasm linear memory as `Zeroizing`
  values; only UniFFI object handles reach JavaScript.
- Unavoidable JS exposure is limited to what the UI must touch: the passphrase on unlock, and a
  mnemonic typed on import or shown on backup. Each goes straight into the core and is dropped.
- Then the extension builds `--no-default-features` like mobile, and `raw-keys` is deleted.
- Not a hard boundary: wasm memory is readable by any script in the extension's context, so code
  execution there can still sign or read it. What this removes is accidental exposure (logs,
  error strings, serialisation, GC copies) and unreliable erasure.
- Locking wipes what the core holds. RustCrypto does not wipe stack temporaries
  (`RustCrypto/meta#38`), so `envelope.rs` overwrites 16 KiB of stack after each KDF and AES-GCM
  call, not at lock. Not covered: registers; on wasm32, values the engine keeps in wasm locals,
  on its native stack outside linear memory; and a memory read while unlocked or during an unlock.

Two things the target does not yet answer (both in Open questions, both to settle before 3.2 is
designed, neither measured or tested yet):
- **The passphrase key gets weaker in one respect.** Today PBKDF2 runs in WebCrypto and yields a
  non-extractable key. In the core it runs as Rust compiled to wasm, and the derived key is bytes
  in linear memory: zeroized on lock, but readable while unlocked. Its speed at 600,000
  iterations in wasm is also unmeasured.
- **A session in wasm memory ends when the service worker does.** An MV3 worker is stopped when
  idle, so "the passphrase crosses once, on unlock" needs a rule for a worker restart: unlock
  again, or keep something in `storage.session`, which puts a secret back within reach of
  JavaScript.

**Interim, extension only, independent of this repo**: decrypt only the signing account's key
rather than the whole vault, narrowing each signature's exposure from every account to one.

## Golden tests

The extension's TypeScript is the reference implementation. Its generators in Bramble
(`scripts/generate-mobile-parity-vectors.ts`, which also imports
`generate-mobile-transaction-vectors.ts` and `generate-mobile-privacy-vectors.ts`, all behind
`bun run mobile:parity`) run the real code paths (Krusty, `encryptJson`, account discovery,
transfer planning, portfolio rendering) and write JSON fixtures. Re-running them leaves no diff;
seals with fresh randomness are kept while the extension still opens them.

- **Here**: the fixtures live in `crates/bramble-core/tests/fixtures/`, with `README.md` as the
  manifest: the Bramble commit (one for all files), and per file the generator and the sha256. A
  test checks the hashes, so a hand-edited fixture fails. Bramble stays the source: syncing is
  re-running the generator there and copying the files, never editing them here.
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
  Multisig coordination (2.5) is the known case: `nostr-multisig-parity.json` covers keys and one
  gift wrap, not recipient selection, the sender rule, sync paging or the sync window.
- **G1, the viewing public key vector** (Bramble, `feature/mobile-wallet`).
  `computeViewingKeys()` in `scripts/mobile-privacy-fixture.ts` adds, per case, the public key
  the extension's `previewStrk20ViewingPublicKey`
  (`packages/platform/vault/src/accounts/strk20-viewing-key.ts`) returns. It lands in
  `extension-privacy-wire.json` under `viewingKeys`; mobile's `privacy_parity.rs` asserts it.
  F1 here: copy the fixtures at the new Bramble commit (only that file's hash changes), update
  its row and the commit line, and add a test in `parity.rs` that
  `stark_public_key(derive_strk20_viewing_key(..))` equals it. As built (Bramble #919):
  `computeViewingKeys()` is in `scripts/mobile-privacy-wire.ts`, and each phrase account gains
  `viewingPublicKey`; mobile matched it with no change. F1 (#42) copied it; `parity.rs` checks both that
  composition and `accounts::advisory::own_viewing_public_key` against it.
- **G2, the multisig coordination generator** (Bramble, `feature/mobile-wallet`). It needs
  `main` merged into the branch first: `multisig-nostr-publish.ts` and
  `multisig-sync-window.ts` exist only on `main`, and the generators only on the branch. A new
  fixture, `nostr-coordination-parity.json`, written by a new file beside
  `scripts/mobile-assets/write-nostr-multisig-parity.ts` and reached from `mobile:parity`,
  calling the extension's own functions over fixed inputs. One section per rule 2.5 ports:
  - relay URLs `isNostrRelayUrl` accepts and refuses;
  - gift-wrap batches through `parsedGiftWraps`: wrong kind, missing `p` tag, duplicate ids,
    a batch over the bound;
  - sync paging: unseen-first bounding, the `until` cursor, candidates interleaved across
    relays, `MAX_SYNC_PAGES`;
  - acknowledgement counts per relay;
  - the sync window: first lookback, incremental lookback, the retry cut-off, at fixed clocks;
  - relay policy: the defaults, and one custom relay replacing them;
  - recipient selection and the sender rule, over a fixed address book and signer set.

  `tests/conformance/mobile-nostr-parity.test.ts` gains the drift check. Mobile's
  `nostr_relay.rs` and `coordination.rs` are tested against it where they already implement a
  rule, which shows where mobile and the extension differ before anything is ported. F2 here
  copies it; it sits unused until 2.5. As built (Bramble #920): three files,
  `nostr-coordination-parity-{rules,relays,paging}.json` (Bramble's 500-line limit covers JSON),
  with no extension code extracted. Mobile differs in six cases, which its test lists and does
  not assert: it refuses `ws:` on loopback where the extension accepts it, and it reads one page
  per relay with no `until` cursor and filters seen wraps after the batch bound. It has no
  gift-wrap batch validation, retry cut-off, recipient selection or `MAX_SYNC_PAGES` at all.
  2.5 takes the extension's behaviour for each.
- **G3, the recovery scan generator** (Bramble, `feature/mobile-wallet`). The families fixtures
  prove addresses and `accounts-parity-recovery.json` proves row wording, but nothing proves the
  scan itself: which shapes it walks, which coordinates it lists with no history, how far it
  goes, or the signing verdicts. Mobile's scan tests run against a canned node it wrote. G3
  exports `recoveryScanShapes` and `isCreatableCandidate` from
  `apps/extension/src/background/onboarding/onboarding-ops.ts` (module-private today; move them
  beside `recoveryScanTarget` in `wallet-model/src/accounts/recovery.ts`) and writes
  `accounts-parity-scan.json` from `scripts/mobile-parity-accounts.ts`, with sections (as built:
  a sibling `scripts/mobile-parity-scan.ts` writes two files, the signing verdicts in
  `accounts-parity-scan-signing.json`, because Bramble's 500-line limit covers JSON; the keep rule
  is exported too, as `keepsRecoveryCandidate`):
  - shapes in order, with and without a BIP-39 passphrase, and each shape's creatable flag;
  - `recoveryScanTarget` and the keep rule over fixed candidate sets (a quiet wallet, a ladder of
    real accounts, the `maxScannedIndexes` ceiling, a manual index);
  - `argentSigningRestriction` and `braavosSigningRestriction` over a fake provider answering
    fixed reads: each restriction, no restriction, a read that fails, a Braavos 2FA removal
    before and after `expire_at`;
  - `strk20RegisteredElsewhere`: no key, this wallet's key, another key.
- **F3** copies it and `mobile/core/generated/accounts.json` (written by `bun run mobile:assets`,
  `scripts/generate-mobile-assets.ts` through `mobile-assets/accounts.ts`, from the extension's
  account tables, so it is extension output; the only fixture not written by `mobile:parity`,
  which the README's intro then says) as
  a fixture, so tests here read the class table, Xverse's class and the scan limits rather than
  hand-copying hashes as `tests/plan_support` does today. One Bramble commit covers every file,
  so F3 re-copies all of them; a re-copied fixture that fails here means the core is behind the
  extension, and the core is fixed, never the fixture.
- **G4, a v12 vault** (Bramble), for the mobile vault → v12 migration in 2.7: the extension seals
  a v12 vault for the published phrase and a fixed account set. Not needed before 2.7.
- **The wasm harness covers only what is exported.** `smoke.test.mjs` replays the KMS and Nostr
  vectors. The vault and send modules are not exported through UniFFI yet, so on wasm32 they are
  linted and never run. That is acceptable until 3.2 and 3.3 export them, and those steps extend
  the smoke test with the vault and transaction fixtures.
- **Where fixtures are written after 2.7.** The generators write into
  `mobile/core/tests/fixtures/`, which 2.7 deletes, and `rust-gift-wrap.json` is written by a
  mobile test. Both need a home by then (see Open questions).
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

## Process

Added 2026-11-09 after reviewing the first day's work. Items marked *proposed* are not agreed.

- **Adoption keeps pace with porting.** The core stays at most one step ahead of mobile. 2.4 is
  not ported until M2, M3 and M4 have merged. A module in both repos is written twice, which is
  what this repo exists to stop.
- **Key-custody code gets a recorded second review.** #13, #19, #20 and #22 (about 7,100 added
  lines) merged with no review on GitHub. Before M2 merges, someone other than the author reviews
  `envelope.rs`, `container.rs`, `send/sign.rs` and `send/plan*.rs` and records it in an issue
  here. *Proposed*: the ruleset requires one approval on `main` from then on.
  **Not done as of 2026-11-10**: M2, M3 and M4 have merged, no issue records the review, and the
  `main` ruleset requires no approval (it blocks deletion and force-push only). The review now
  covers #25 and #31 as well, and should be recorded before A1 merges, since A1 and A2 add
  key-derivation paths.
- **The Status table is updated in the PR that changes it**, in either repo's half.
- **Tags are signed.** `v0.1.0` is annotated but unsigned, and a tag is never moved, so it stays
  that way; later tags use `git tag -s`.
- **Provisional, 2026-11-11**: the stacked port PRs (#39 onwards) are unmerged and untagged, so
  each mobile adoption PR pins `wallet-core` by `rev` at its port branch's tip and says it moves
  to a tag before merge. A port branch a mobile branch pins only gains commits; it is not
  rebased or amended. This is the proposed rule below, used because nothing can be tagged yet.
- *Proposed*: **mobile may pin a `rev` between tags** during Phase 2. Every tag needs a version
  bump in three files, and `Cargo.lock` records the commit either way. Tags would mark 2.7 and
  each npm publish. `AGENTS.md` says mobile pins a tag; change both together if agreed.
- **Dependabot does not propose what the hard rules forbid.** Add ignores for
  `starknet-types-core` (follows Krusty), `uniffi*` (follows ubrn and `uniffi-runtime-wasm`) and
  `wasm-bindgen` (must equal the CLI), and close #10, #11 and #12. Done (#24) except
  `wasm-bindgen`, which `.github/dependabot.yml` does not ignore; version updates are off, so
  only a security update could propose it.
- **#16 (crate READMEs, crates.io) is split.** The READMEs and a LICENSE file (both crates
  declare MIT and the repo has none) go in now. Publishing waits on the crates.io question
  below. Its rename to `starknet-wallet-core` is the part with a deadline: M2 writes the package
  name into mobile, so the rename lands and is tagged before M2 merges, or is dropped.
- **Housekeeping** (done on this branch): `tests/fixtures/README.md` said only KMS, accounts and
  Nostr had ported modules; it now names the tests that read each group.

## Open questions

- Which vault container the extension uses after 3.2. `container.rs` is mobile's (a DEK, a
  passphrase slot, a biometric slot). The extension stores a bare v3 envelope under
  `bramble:encrypted-store:<key>`, with a freshness check on its lineage in `chrome.storage` that
  the core has no notion of. Either the extension migrates to the container or the core opens
  bare envelopes, and one of them owns the freshness check.
- What a `VaultSession` does when the MV3 service worker restarts (see Key exposure).
- PBKDF2 in wasm: how long 600,000 iterations take, and whether losing WebCrypto's
  non-extractable key is accepted (see Key exposure). A throwaway export timed in
  `smoke.test.mjs` answers the first.
- `container.rs` holds `bramble.mobile.*` format and scope strings in the generic crate. Make
  them arguments, as the rule says, or record them as an exception like the resource bounds:
  they are bytes on disk, not taste, and changing them orphans every mobile vault. Leaning
  exception.
- Where the generators write, and what writes `rust-gift-wrap.json`, once 2.7 deletes
  `mobile/core/tests/fixtures/`. The obvious answer is this repo's fixtures directory, written
  from a Bramble checkout beside it.
- Whether `security.rs` (wrong-passphrase backoff) is mechanism for `wallet-core` or mobile-only
  glue; it depends on whether the extension has, or wants, the same rule.
- npm scope: `@starknet-innovation` (matches the GitHub org) or `@starknetfoundation` (matches Krusty).
  Deferred; settle before the first npm publish (Phase 3.1), since the extension's pin fixes the
  name. Tags before then don't publish to npm.
- Which `starknet-types-core` the sequencer runs (0.2.4 is reported but unconfirmed). It matters only
  if this repo starts sharing types with sequencer or node crates; until then Krusty sets the pin.
- UniFFI 0.29.5 → 0.31.2: what changes in the generated Swift and Kotlin (Phase 2.7).
- `HostStorage` is synchronous and `chrome.storage` is not; the extension side needs a write-through
  mirror (the pattern `encrypted-store.ts` already uses).
- How long the extension holds the vault passphrase in JavaScript. `signHash` takes it on every
  call and the grant cache (`vault/src/service/grant-cache.ts`) keeps it until lock; where the
  caller holds it was not traced. Under the 3.2 target it crosses once, on unlock.
- The JS binding generator for the extension (ubrn `wasm2`, pre-production, or wasm-bindgen
  directly) is decided at Phase 3.1, not now; the Rust surface does not depend on it.
- Whether the devnet OZ class hash moves into the core or stays a TypeScript constant.
- Who owns the mobile vault → v12 migration. Where and when is proposed (2.7, its first PR, after
  G4); the owner is not named.
- When a second wallet wants `wallet-core`: publish it to crates.io, and whether it ships its own
  UniFFI feature or each wallet keeps exporting from its own crate (today's answer).
- Which modules count as mobile-only glue, and whether they live in the app or in `bramble-core`
  behind mobile's build (Phase 2).
