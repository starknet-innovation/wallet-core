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
  (Revised 2026-11-13: the target is the extension's current schema, v13, and mobile switches to
  it rather than migrating, since no mobile vault holds funds yet. See [Vault switch](#vault-switch).)
- The extension consumes a published npm package at an exact pin with a digest check, the way it
  pins Krusty today. Extension PRs go to `main`; mobile adopts the crate on `feature/mobile-wallet`.
- Host I/O is one key/value storage trait plus a clock, both host-provided (files on mobile,
  `chrome.storage` in the extension). Sans-IO RPC stays as the mobile core has it.
- The core becomes the source of truth for the network, account-class, token-registry and privacy
  catalogs. Design tokens stay in the extension's `ui-kit`.
- Everything is in scope long term (Ledger, dapp provider and permissions, STRK20 submission, swap,
  gasless, NFTs, allowances) but not in v1.

## Status

Every Phase 2 step has two halves: the mechanism lands here, then mobile deletes its copy and
depends on this one. A step is done when both halves are.
As of 2026-11-13 both halves of every step through 2.7 are done. Every port here is merged
(#39–#50, with #51 and #52) and `0.3.0` carries them. In Bramble, G1–G3 (#918–#920) and
M5–M13 (#921–#929) are merged into `feature/mobile-wallet`, whose `mobile/core` is now a shell
that builds this crate's `bramble-core` at `tag = "v0.3.0"` with `features = ["mobile"]`. M7–M13
merged into each other's stacked branches and reached `feature/mobile-wallet` through M6's merge.
#54 (stack scrub after Krusty and Nostr secret calls) is merged after the tag and ships in the
next release. M13 merged before the vault step that 2.7 placed ahead of it, so mobile still
writes its own vault version 5. Left in Phase 2: the [vault switch](#vault-switch).

| Step | Here | Mobile (`mc-wallet`, `feature/mobile-wallet`) |
| --- | --- | --- |
| 0 Scaffold, 1 Golden suite | done | n/a |
| 2.1 KMS, Nostr, felt | done (#13, #20) | done (Bramble #907, #909) |
| 2.2 Vault | done (#19) | done (Bramble #911); the payload switch to v13 is the [vault switch](#vault-switch) |
| 2.2 Accounts | done (A1 #39, F3 #40, A2 #41) | done (G3 Bramble #918, M5 #921) |
| 2.3 Transactions | done (#22, #25, #31) | done (Bramble #916) |
| 2.4a Shims | done (#43) | done (M6, Bramble #922) |
| 2.4b Portfolio | done (#44) | done (M7, Bramble #923) |
| 2.4c History and activity | done (#45) | done (M8, Bramble #924) |
| 2.4d Privacy | done (#46) | done (G1 Bramble #919, M9 #925) |
| 2.5a Multisig account | done (#47) | done (M10, Bramble #926) |
| 2.5b Multisig coordination | done (#48) | done (G2 Bramble #920, M11 #927) |
| 2.6 Catalogs | done (#49) | done (M12, Bramble #928) |
| 2.7 Swap | done (#50), behind the `mobile` feature | done (M13, Bramble #929), at `v0.3.0`: `mobile/core` is a shell over this crate |
| Vault switch | not started | not started (G4, M14) |
| 3 Extension | not started; `js/package.json` is private | n/a |

`v0.1.0` is tagged at `35abc8b` (GitHub release only) and was what M2 and M3 pinned. `v0.2.0`
renamed the package to `starknet-wallet-core` and carries #25, which removed
`PlanHost::discard`. `v0.2.1` was what mobile pinned through M4: it makes `send::plan_probe`'s
selectors, `selector` and `braavos_signers` public, which mobile's `recovery_advisory.rs` used.
`v0.2.2` is the first crates.io publish (#38, #27) and adds, since `v0.2.1`, the vault's refusal
of a DEK the file's slots do not wrap (#36) and vault transition tests from the Quint specs (#34,
#35). Mobile went from `v0.2.1` to `v0.3.0` at M5; M5–M13 each merged pinned to that tag.

### Next PRs

Generators (G*) and adoption work (M*) are PRs in Bramble (`starknet-innovation/mc-wallet`)
against `feature/mobile-wallet`, where `mobile/core` and the generators live (`main` has neither).
The rest are PRs here. Phase 2's PRs (M1–M13, G1–G3, A1, A2, F1–F3 and the ports) are all
merged; each is described under its step in [Phases](#phases).

| PR | Repo, base | Needs | What it is |
| --- | --- | --- | --- |
| G4 v13 vault generator | Bramble, `feature/mobile-wallet` | nothing | the extension's own store and schema code seal a v13 vault for the published phrase and a fixed account set; payload and sealed file written as fixtures |
| V1 Vault switch | here | G4 | `bramble-core` writes the extension's v13 payload inside mobile's container; the v1 → v5 chain and its regression vaults go; G4's files copied as fixtures |
| Release `v0.4.0` | here | V1, #55 | the format break, plus #54 |
| M14 Take `v0.4.0` | Bramble, `feature/mobile-wallet` | `v0.4.0` | the pin moves; `importWallet` loses its unused `passphrase` argument at its call sites; both apps build; testers reset their wallets |

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
| Later phases: vault container (envelope v3), storage and clock traits, sans-IO RPC, transaction hashing, transfer planning, portfolio/history/privacy logic, Nostr relay paging and gift-wrap batch checks | Later phases: the extension's vault schema (v13), UniFFI foreign traits adapting to `wallet-core`'s, catalogs, multisig coordination policy (identifier, relays, sync window, who counts as a co-signer), iOS/extension-only assets |

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
Stark private key, which the extension's vault needs until Phase 3.2; `extension` (default,
implied by `raw-keys`) exports the extension's other KMS and Nostr functions, several of which take
or return a recovery phrase; mobile builds `--no-default-features --features mobile`, so neither
reaches its bindings. `mobile` adds the mobile wallet's surface (2.7), and
`testing` (implies `mobile`) its integration-test hooks. `bindgen` adds the Swift/Kotlin generator
binary. `wallet-core` has no features.

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
      `mobile/core` until 2.7; mobile's vault moves to the extension's schema after it (Vault switch).
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
      `contact_card.rs` into `js::felt`. **2.5b, the coordination** (done, #48), the rest of this
      section, with the extension's names and arguments (snake-cased): `nostr::relay_url`
      (`is_nostr_relay_url`, which now accepts `ws:` on `127.0.0.1` and `localhost` as the
      extension does), `nostr::relay` (`valid_relay_urls`, `parsed_gift_wraps` in the extension's
      order of checks, `publish_many_nostr_gift_wraps` and mobile's acknowledgement counting),
      `nostr::sync` (`sync_nostr_gift_wraps`, mobile's `SyncJudge` extended to pages:
      unseen-first bounding, the `until` cursor, the page cap, per-page subscription ids) and
      `nostr::sync_window` (`nostr_sync_since`, `retryable_synced_at`, `is_wrap_past_retry`), with
      the page cap and relay count as `RelayLimits` and the lookbacks as `SyncLookbacks`; the
      retry cut-off is the first lookback, as in the TypeScript, so it has no constant of its own.
      `bramble-core`'s `coordination` holds the identifier, default relays, `MAX_SYNC_PAGES`, the
      lookbacks, `coordination_relay_urls`, `with_nostr_relay_url`, `is_coordination_signer` and
      `coordination_recipients` (over plain address-book records, with the extension's `Map`
      dedupe), as plain Rust until 2.7 exports them. Every case of the three G2 fixtures passes;
      none is skipped. Left for 2.7 with the coordination file: the stored-relay normaliser,
      `is_trusted`'s package parsing and `is_checkpoint` (no fixture cases).
   6. **Catalogs**: `generated/*.json` are Bramble's; `wallet-core` takes catalogs as arguments.
      Done (#49): `bramble-core` gains `generated/{networks,token-registry,accounts,multisig}.json`
      (Bramble's assets, byte-identical to their fixture copies, which a test checks) and
      `catalogs::{networks, accounts, account_address, multisig}`, mobile's loaders as plain Rust
      (no UniFFI until 2.7), including `multisig_constants.rs` and the class rules of
      `account_address.rs` (`resolve_class`, `argent_shape`), which the table below did not list.
      The tests that hand-built catalog records now read these. `wallet-core`'s remaining
      Bramble strings became arguments: `portfolio::PortfolioPolicy` (the
      `bramble.mobile.portfolio.v1` schema, the price API base, the one priced network) for
      `PriceBook`, `BalanceSnapshots` and `TokenPreferences`; `history::HistoryPolicy` (the
      Starkscan API base, the untracked-token name, the pre-confirmed hint that names Bramble) for
      `ActivityBook`, the Starkscan functions and `activity_groups`; and the USDC.e alias joined
      the registry as `TokenRegistry::aliases`. Bramble's values are `PORTFOLIO_POLICY`,
      `HISTORY_POLICY` and `LEGITIMATE_ALIASES` in `bramble-core`. All breaking. Left in
      `wallet-core`: `container.rs`'s format strings (Open questions) and the extension's own copy
      the fixtures check (row copy, activity labels, notices). Mobile keeps its own loaders and
      copies of the policies until 2.7 swaps them for these; M12 follows the new signatures and
      drops mobile's direct Krusty dependency (only `kms.rs` named it, for `ChainId` and
      `SecretFelt`; `networks.rs` no longer did).
   7. **Swap**: what remains in `mobile/core` that is Bramble's but not mobile-only (constants,
      v12 schema and migrations, catalogs, the UniFFI surface, `HostStorage` and `HostClock` as
      `#[uniffi::export(with_foreign)]` traits adapting to `wallet-core`'s) moves into this
      repo's `bramble-core`. `mobile/core` is then replaced by a git dependency on it at a tag
      (same crate, lib and module names), Swift and Kotlin regenerate on UniFFI 0.29.5 → 0.31.2,
      and builds use `--no-default-features --features mobile`.

      **The port** (open, #50): every file of `mobile/core/src` at Bramble
      `8d1f4b8c` except `bin/uniffi-bindgen.rs` moved, copied unchanged but for paths, behind
      `bramble-core`'s `mobile` feature; its modules keep their crate-root names (`src/mobile.rs`,
      included by `lib.rs`). Merged rather than copied: `CoreError` (the union, the extension's
      variants first), `CoreInfo` and `core_info`, `StarkSignature` (wallet-core's), the catalogs
      (2.6's `catalogs::*` replace `networks.rs`, `account_catalog.rs`, `multisig_constants.rs` and
      the class rules of `account_address.rs`, regaining mobile's UniFFI attributes under
      `mobile`), `OZ_ACCOUNT_VERSION`, and `coordination` (mobile's file becomes
      `coordination::file`, whose relay set and sync window are 2.5b's
      `coordination_relay_urls` and `SYNC_LOOKBACKS`). `security.rs`, `biometric.rs`, `backup.rs`
      and `public_store.rs` moved too, under `mobile`: `wallet.rs` uses them and two export through
      the same UniFFI module. Mobile's tests run with `--features mobile,testing`; its regression
      fixtures are in `crates/bramble-core/tests/regression/`. The vault chain still ends at v5;
      the plan put the v5 → v12 step before M13, but M13 merged first (see Vault switch). Found
      in review: one crate exporting both surfaces put the extension's phrase-handling exports
      (`generate_mnemonic` returns a phrase; five more take one) into mobile's bindings, so they
      moved behind a default `extension` feature mobile leaves off; and the FFI-surface test's scan
      missed exports written under `cfg_attr`, now fixed. `catalogs::GENERATED` lists every
      generated file with the path Bramble writes it to, for M13's drift test.
      **M13** (Bramble): `mobile/core` keeps its manifest, lockfile, `bin/uniffi-bindgen.rs`,
      `generated/` (the generators' output) and `tests/fixtures/` (the generators' output and the
      conformance tests' input), and depends on this crate by `rev` with
      `default-features = false, features = ["mobile"]`; the build scripts build the dependency
      (`cargo build -p bramble-core --lib`), which yields the same library names, and its own
      bindgen reads this crate's `uniffi.toml`, so Swift and Kotlin keep their module names. A test
      there holds `generated/` to `catalogs::GENERATED`. As built (Bramble #929): as above, with
      `uniffi.toml` gone (the bindgen reads this crate's) and cargo-ndk given `--manifest-path`;
      no Swift or Kotlin app code changed; both apps build and pass their unit tests on the
      regenerated bindings, which equal this crate's.

      <a id="vault-switch"></a>**Vault switch** (decided 2026-11-13; replaces the v5 → v12
      migration first placed here). M13 merged before it, so mobile still writes its own vault
      version 5 (`migrations.rs`: a chain v1 → v5 inside the DEK container) while the extension
      writes schema v13 (#826 added the multisig STRK20 viewing keys) inside a bare v3 envelope.
      No mobile vault holds funds yet, so mobile switches format rather than migrating:
      - `bramble-core` writes the extension's v13 payload, unchanged in shape and validated by a
        port of its rules (`packages/platform/vault/src/schema/`), inside mobile's existing DEK
        container. The container stays (its biometric slot is mobile's); which container the
        extension uses is the separate Open question, since this changes only the payload.
      - The v1 → v5 chain goes: most of `migrations.rs`, `migrations_tests.rs`,
        `tests/migration.rs`, and the regression vaults `vault-*.json` and
        `migration-v1-to-v5.golden.json`. A container whose payload is not v13 fails closed with
        an error telling the user to reset the wallet; there is no converter.
      - Mobile's records map onto the extension's. `id` (`<network>:<address>`), `address` and
        `class_hash` keep their values; `network` → `networkId`; `public_key` → `publicKeyX`, plus
        `publicKeyY` for `starknet-bip44` (`kms::StarkKeypair::public_key_y`); `family` →
        `accountContractType`; `deployment_salt` → `deploymentSalt`, which v13 requires for
        Argent and Braavos and forbids for OpenZeppelin. `ladder` and `account_index` →
        `derivation`: `Account` is `starknet-bip44` at `{accountIndex: i, addressIndex: 0}`,
        `Address` (Braavos's) at `{accountIndex: 0, addressIndex: i}`, `ArgentX` is
        `argentx-bip44` at `{addressIndex: i}` (the extension's `recoveryLadderIndex` reads each
        back as `i`). `originGrants`, `importedSeeds`, `accountSecrets`, `shadowAccountScopes` and
        the multisig key lists are written empty.
      - Every account's derived private key is sealed in `signingKeys`, as the extension does and
        its validator requires. The keys stay inside Rust and never cross the FFI, so the mobile
        build still exports no raw key; it is still a custody change and the PR says so.
      - Mobile's BIP39 passphrase (`mnemonicPassphrase`) is dropped: v13 has no field for it, and
        the extension's recovery refuses one. Both apps pass `nil` to `import_wallet`'s
        `passphrase` today, so the argument goes, which changes the bindings (M14 updates the call
        sites).
      - Labels and groups leave the vault for `public_store.rs`, as the extension keeps them in
        preferences. `backupPending` and `recoveryPending` are mobile's own (the extension has
        neither) and move there too. None is secret.
      - The oracle is G4: the extension's own store and schema code seal a v13 vault for the
        published phrase and a fixed account set (OZ, Argent, Braavos, a Ready `argentx-bip44`
        account). The test opens it, checks its payload passes the ported validators and
        re-serialises to equal JSON, and checks that a vault mobile builds for the same phrase
        and accounts holds an equal payload.
      - When the extension's schema moves past v13, mobile follows in a release of this crate,
        until Phase 3.2 makes the core the one writer.

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
   | 2.7, provisional | `security.rs` (wrong-passphrase backoff) | 269 | `bramble-core` under `mobile`; see Open questions |
   | 2.7 | `biometric.rs`, `backup.rs`, `public_store.rs` | 299 | `bramble-core` under `mobile` (mobile-only glue that exports through the same module) |
   | stays | `bin/uniffi-bindgen.rs` | 4 | mobile's generator binary |

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
      Migrations from schemas before v13 stay in TypeScript. See [Key exposure](#key-exposure).
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

How far raw private keys, the mnemonic and the vault passphrase travel outside Rust, today and
after Phase 3.2.

**Today (Bramble `main`, `a01b240c`, v0.10.10).** Krusty's wasm is stateless: keys cross it as
hex strings in both directions. The v13 vault holds the mnemonic (recovery-phrase wallets), each
account's derived key (`signingKeys[].privateKey`), optional account secrets and multisig STRK20
viewing keys under AES-256-GCM.
- **Every signature decrypts the whole vault.** `signHash` (`vault/src/service/service.ts`) reads
  and decrypts the full vault into JavaScript objects, then passes one key string to
  `kms.signStarkHash`. Each signature puts the mnemonic, every account's key and the other vault
  secrets into the JS heap as immutable strings that cannot be zeroed; GC frees them later but never erases them.
- **The passphrase is held for the whole unlock.** The background worker's unlock session
  (`apps/extension/src/background/runtime/session.ts`) keeps the plaintext passphrase in memory on
  a rolling inactivity window (30 minutes by default, 8 hours at most) and mirrors it to
  `chrome.storage.session` under `bramble:session` (`runtime/unlock-session-boot.ts`), so an MV3
  worker restart does not lock the wallet. Lock or expiry erases the mirror or overwrites it with
  a revocation marker that a restarted worker refuses. The grant cache
  (`vault/src/service/grant-cache.ts`) keeps the passphrase as its entry key beside public account
  records only.
- **PBKDF2 runs once per unlock.** `encrypted-store.ts` memoises the derived AES key (at most 8,
  keyed by a passphrase fingerprint, salt and iterations) as a non-extractable WebCrypto key and
  drops it on lock. Non-extractable stops export, not use, and the plaintext passphrase sits beside
  it, so it protects little against code running in the worker.
- What limits it: everything runs in the background service worker; `storage.session` is
  in-memory, cleared when the browser exits, set to trusted contexts on Chromium
  (`apps/extension/src/browser-storage.ts`) and unreachable from content scripts by default
  elsewhere. Other extensions cannot reach the worker's memory or storage.

**`raw-keys` reproduces this model**, no worse and no better. Adopting the core with it on keeps
today's exposure; it is a bridge, not the target.

**Target (3.2, a precondition for dropping `raw-keys`, not a follow-up).**
- The core unlocks the vault and holds it: `unlock(vault_bytes, passphrase) → VaultSession`,
  with signing and derivation as session methods. Keys live in wasm linear memory as `Zeroizing`
  values; only UniFFI object handles reach JavaScript.
- Unavoidable JS exposure is limited to what the UI must touch: the passphrase on unlock, and a
  mnemonic typed on import or shown on backup. Each goes straight into the core and is dropped.
  That holds only if the extension stops holding and mirroring the passphrase (see the restart
  question below); keeping today's unlock session keeps the passphrase in JavaScript.
- Then the extension builds `--no-default-features` like mobile, and `raw-keys` is deleted.
- Not a hard boundary: wasm memory is readable by any script in the extension's context, so code
  execution there can still sign or read it. What this removes is accidental exposure (logs,
  error strings, serialisation, GC copies) and unreliable erasure.
- Locking wipes what the core holds. Neither RustCrypto nor Krusty wipes stack temporaries
  (`RustCrypto/meta#38`), so every function that takes or makes a secret runs its body through
  `scrub::scrubbed`, which overwrites 128 KiB of stack afterwards: the vault's KDF and AES-GCM
  calls, phrase generation and checks, every key derivation, Stark signing and public keys, the
  STRK20 viewing key, and Nostr's key, wrap and open. A phrase derivation, the deepest, uses about
  67 KiB; a test keeps every such call within the scrubbed size. Not covered: a secret a function
  returns by value (moving it out copies it into the caller's frames, so the extension's raw-key
  exports leave one copy); registers; on wasm32, values the engine keeps in wasm locals, on its
  native stack outside linear memory; and a memory read while unlocked or during an unlock.

Two things the target does not yet answer (both in Open questions, both to settle before 3.2 is
designed, neither measured or tested yet):
- **The passphrase key loses non-extractability.** Today PBKDF2 runs in WebCrypto and yields a
  non-extractable key. In the core it runs as Rust compiled to wasm, and the derived key is bytes
  in linear memory: zeroized on lock, but readable while unlocked. This costs little while the
  plaintext passphrase is held beside it, as it is today. PBKDF2 runs once per unlock (and once
  per restored session), so its speed at 600,000 iterations in wasm matters there; it is
  unmeasured.
- **A session in wasm memory ends when the service worker does.** An MV3 worker is stopped when
  idle. Today a restart restores the unlock from the plaintext passphrase in `storage.session`.
  3.2 picks one: keep that mirror (same as today, and the passphrase stays in JavaScript), or
  require unlocking again after a restart (the passphrase crosses once, at the cost of more
  unlocks).

**Interim, extension only, independent of this repo**: decrypt only the signing account's key
rather than the whole vault, narrowing each signature's exposure from the mnemonic and every
account's key to one key.

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
- **G4, a v13 vault** (Bramble), for the [vault switch](#vault-switch): the extension's own store
  and schema code seal a v13 vault for the published phrase and a fixed account set (OZ, Argent,
  Braavos, Ready), written as the payload and the sealed file.
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
- **The package check and a stale local Cargo cache (2026-11-12).** On the machine that ran
  these ports, `check.sh`'s `package` step failed from 2.5b on, which was first put down to the
  published 0.2.2 lacking the APIs `bramble-core` now uses. Wrong: `cargo package --workspace`
  verifies `bramble-core` against the workspace's own `wallet-core`, and CI's package step passed
  on #48 and #49; the local failure came from a stale extracted copy of the published 0.2.2 in
  `~/.cargo/registry/src`. With a clean `CARGO_HOME`, `check.sh all` passes on #48, #49 and #50
  at 0.2.2. The breaking changes since 0.2.2 (`Storage::rename`, 2.6's constructors) still call
  for 0.3.0 at the next release.
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
- What a `VaultSession` does when the MV3 service worker restarts: keep the extension's
  plaintext passphrase mirror in `storage.session`, or unlock again (see Key exposure).
- PBKDF2 in wasm: how long 600,000 iterations take at unlock, and whether losing WebCrypto's
  non-extractable key is accepted (see Key exposure). A throwaway export timed in
  `smoke.test.mjs` answers the first.
- `container.rs` holds `bramble.mobile.*` format and scope strings in the generic crate. Make
  them arguments, as the rule says, or record them as an exception like the resource bounds:
  they are bytes on disk, not taste, and changing them orphans every mobile vault. Leaning
  exception.
- Where the generators write after 2.7. *Provisional*: they keep writing Bramble's
  `mobile/core/tests/fixtures/`, which M13 keeps (the conformance tests read it there), and this
  repo copies from it as before.
  `rust-gift-wrap.json` and `ui-coordination.json` are settled by the 2.7 port: Rust writes them
  into `crates/bramble-core/tests/regression/` (`BRAMBLE_WRITE_FIXTURES=1`), and Bramble copies
  them to where the extension and the iOS UI suite read them.
- Whether `security.rs` (wrong-passphrase backoff) is mechanism for `wallet-core` or mobile-only
  glue; it depends on whether the extension has, or wants, the same rule. Provisionally in
  `bramble-core` under `mobile` (2.7).
- npm scope: `@starknet-innovation` (matches the GitHub org) or `@starknetfoundation` (matches Krusty).
  Deferred; settle before the first npm publish (Phase 3.1), since the extension's pin fixes the
  name. Tags before then don't publish to npm.
- Which `starknet-types-core` the sequencer runs (0.2.4 is reported but unconfirmed). It matters only
  if this repo starts sharing types with sequencer or node crates; until then Krusty sets the pin.
- UniFFI 0.29.5 → 0.31.2: what changes in the generated Swift and Kotlin (Phase 2.7). From the
  2.7 port: no exported name, argument, field or case of mobile's changed and nothing was added
  (the extension's exports are behind `extension`); `CoreError`'s cases are reordered (the
  extension's first), which changes their wire numbers; the rest is generator code (handles for
  pointers, synthesised `Equatable`/`Hashable`, qualified Kotlin type names). The apps' build
  against it is M13's.
- `HostStorage` is synchronous and `chrome.storage` is not; the extension side needs a write-through
  mirror (the pattern `encrypted-store.ts` already uses).
- The JS binding generator for the extension (ubrn `wasm2`, pre-production, or wasm-bindgen
  directly) is decided at Phase 3.1, not now; the Rust surface does not depend on it.
- Whether the devnet OZ class hash moves into the core or stays a TypeScript constant.
- When a second wallet wants `wallet-core`: publish it to crates.io, and whether it ships its own
  UniFFI feature or each wallet keeps exporting from its own crate (today's answer).
- *Provisional, 2.5b*: the core accepts `ws:` relays on loopback, as the extension does (its
  fixture says so); mobile keeps refusing every `ws:` relay as a platform rule (App Transport
  Security and `usesCleartextTraffic=false` block cleartext), on top of the core's grammar
  (M11, Bramble #927). Revisit if a mobile build ever needs a local relay.
- *Provisional, 2.5b*: mobile reads one page per relay (the core at a page cap of 1) until 2.7,
  because paging needs a third `RelayStep` variant across the FFI and the bindings may not
  change before then. Three paging cases stay listed as mobile's differences until 2.7.
- Which modules count as mobile-only glue, and whether they live in the app or in `bramble-core`
  behind mobile's build (Phase 2).
