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

As of 2026-11-10. Every Phase 2 step has two halves: the mechanism lands here, then mobile deletes
its copy and depends on this one. Both halves are done for 2.1 and the vault; `rpc.rs` and the
`send/` mechanism still exist in both repos until M4. A step is done when both halves are.

| Step | Here | Mobile (`mc-wallet`, `feature/mobile-wallet`, at `b1a5a62b`) |
| --- | --- | --- |
| 0 Scaffold, 1 Golden suite | done | n/a |
| 2.1 KMS, Nostr, felt | done (#13, #20) | done (Bramble #907, #909), at `v0.1.0` |
| 2.2 Vault | done (#19) | done (Bramble #911), at `v0.1.0`; the vault → v12 migration has no owner |
| 2.2 Accounts | not started, and not yet described (see 2.2) | n/a |
| 2.3 Transactions | done (#22, #25) | not started (M4); needs `v0.2.0` |
| 2.4 Reads, 2.5 Multisig, 2.6 Catalogs, 2.7 Swap | not started | not started |
| 3 Extension | not started; `js/package.json` is private | n/a |

`v0.1.0` is tagged at `35abc8b` (GitHub release only) and is what M2 and M3 pin. `v0.2.0` is the
tag M4 takes: the package is `starknet-wallet-core` from it, and it carries #25, which removed
`PlanHost::discard`.

### Next PRs

None of the adoption work is a PR in this repo. It is all in Bramble (`starknet-innovation/mc-wallet`)
against `feature/mobile-wallet`, where `mobile/core` and the generators live (`main` has neither).
This repo only receives the fixture copies that follow a generator change.

| PR | Repo, base | Needs | What it is | Described in |
| --- | --- | --- | --- | --- |
| M1 Krusty 0.11 → 2.0.0 (merged, Bramble #907) | Bramble, `feature/mobile-wallet` | nothing | pins, version string, one conformance test, `rust-gift-wrap.json` regenerated | 2.1 step 1 |
| M2 Take `wallet-core`: felt, Nostr, KMS (merged, Bramble #909) | same | M1, `v0.1.0` | adds the dependency; three modules become re-exports or glue; duplicate parity tests deleted | 2.1 step 3 |
| M3 Vault (merged, Bramble #911) | same | M2 | `vault.rs`, `container.rs` and `ClockReading` from the core; `HostClock` and `files.rs` adapt | 2.2 |
| M4 Send | same | M2, `v0.2.0` | `rpc.rs` and the `send/` mechanism from the core; `TransferPlan` wraps `Plan` | 2.3 |
| G1 Viewing public key vector | same | nothing | one field in the privacy generator | Golden tests |
| G2 Multisig coordination generator | same | `main` merged in | a new fixture for the 2.5 rules | Golden tests |
| F1, F2 Fixture copies | here | G1, G2 | files copied unchanged, sha256 rows and the Bramble commit line updated, the test that reads them | Golden tests |

M4 is the one adoption PR left. G1 and G2 can start now (`main` was merged into the branch by
Bramble #908); G2 is the long pole for 2.5. #908 also regenerated four fixtures
(`extension-transactions-copy.json`, `extension-privacy-copy.json`, `extension-privacy-rules.json`,
`portfolio-parity.jsonl`), so the copies here are behind Bramble until the next fixture copy.

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
  src/kms.rs               key custody over Krusty
  src/nostr.rs             coordination identity, NIP-59 wrap/open
  src/envelope.rs          the extension's EncryptedEnvelope v3 (PBKDF2-SHA-256, AES-256-GCM)
  src/container.rs         vault container: DEK, passphrase and biometric slots, sealed payload
  src/host.rs              host-provided Storage and Clock traits, ClockReading
  src/rpc.rs               sans-IO JSON-RPC: envelope, chain id, balanceOf, amounts, u256 text
  src/send/                transfers: calls, fees, judge, rpc_bodies, sign (v3 hashes), u256, and
                           the sans-IO Plan with its PlanHost
crates/bramble-core/       Bramble: lib bramble_core, the cdylib/staticlib/wasm32 module
  src/lib.rs               UniFFI scaffolding, CoreError, core_info(), Bramble constants, exports
  src/bin/uniffi-bindgen.rs  Swift/Kotlin generator (`bindgen` feature)
  uniffi.toml              Swift/Kotlin module names, as mobile's
  tests/parity.rs          extension fixtures and known-answer vectors
  tests/vault_parity.rs    envelope and DEK slot the extension sealed
  tests/transactions_*_parity.rs  send rules; every body, hash and signing; the plan over every capture
  tests/fixtures.rs        holds tests/fixtures/ to its README manifest (sha256 per file)
  tests/fixtures/          copied from Bramble; README is the manifest
js/                        npm package @starknet-innovation/bramble-core: ubrn config,
                           entrypoint, smoke test over the built wasm
.github/workflows/         ci.yml (rust + wasm jobs), release.yml (tag → npm, with provenance),
                           advisories.yml (weekly `cargo deny check advisories` on main)
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
   `accounts-parity-families-mainnet.json`, `accounts-parity-recovery.json` and
   `accounts-parity-rules.json` (2.2 accounts). Where the
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

      **Accounts are not ported and have no description yet**, although this step is named for
      them. Mobile's account mechanism is `kms_accounts.rs` (family addresses through Krusty:
      `AccountClass`, `BraavosAccount`, `calculate_contract_address`), `recovery.rs`,
      `recovery_scan.rs` and `recovery_advisory.rs` (the sans-IO discovery scan), about 1,600
      lines, with `accounts_parity.rs` and `recovery.rs` as its tests and three
      `accounts-parity-*.json` fixtures here that no test reads. It has to move before 2.7,
      because `bramble-core` may not name Krusty. Proposed: a second half of this step, with
      family order and scan limits as arguments. See [Unassigned modules](#unassigned-modules).

      Still open: the accounts half, and the mobile vault → v12 migration.
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

      Mobile's side (M4): take `v0.2.0` (`wallet-core = { package = "starknet-wallet-core", git =
      …, tag = "v0.2.0" }`, with `zeroize` `=1.9.0` → `=1.9.1` and `serde_json` `=1.0.151` →
      `=1.0.154`, the core's pins), delete `rpc.rs` and those `send/` files and `use
      wallet_core::{rpc, send::*}`. Mobile's plan still drops the pending record of a write the
      node errored on (`store::discard` in `plan_deploy.rs` and `plan_submit.rs`); the core's
      does not (#25), so those calls go, the record stays until `observe` settles it, and the
      failure carries the hash. Mobile's tests that expect the record gone change with it.
      `TransferPlan` becomes a UniFFI object around `Mutex<Plan>` holding `Weak<WalletCore>` and the session generation; its `PlanHost` maps onto `plan_sign`,
      `session_generation`, `activity::store` (`PendingRecord` → `Entry`, direction "out",
      status "pending") and `history::explorer`; `sign` must refuse once the generation moved,
      as `plan_sign` does. `wallet_ext.rs` keeps preparing plans (vault records, contacts,
      catalogs), building `Authorization` from `account_catalog` and calling `Transfer::new`. Records keep their UniFFI
      derives through `#[uniffi::remote]` until 2.7. Mobile's wallet-level tests
      (`transactions_lifecycle.rs`, `transactions_block_pin.rs`, and the contact, tracked-token
      and locked-prepare cases of `transactions_plan.rs`) stay there: they exercise its locks,
      store and address book through that wrapper.
   4. **Portfolio, history, privacy reads**: `portfolio/*`, `history/*`, `activity/*`,
      `privacy/*` into `wallet-core`, with the `js/*` JavaScript-semantics shims beside whichever
      code calls them; the iOS `include_str!` in `history/appearance.rs` moves to the Bramble
      crate's `generated/`.
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
   6. **Catalogs**: `generated/*.json` are Bramble's; `wallet-core` takes catalogs as arguments.
   7. **Swap**: what remains in `mobile/core` that is Bramble's but not mobile-only (constants,
      v12 schema and migrations, catalogs, the UniFFI surface, `HostStorage` and `HostClock` as
      `#[uniffi::export(with_foreign)]` traits adapting to `wallet-core`'s) moves into this
      repo's `bramble-core`. `mobile/core` is then replaced by a git dependency on it at a tag
      (same crate, lib and module names), Swift and Kotlin regenerate on UniFFI 0.29.5 → 0.31.2,
      and builds use `--no-default-features`.

   Mobile-only platform glue with no extension counterpart (biometric slots, file storage, app
   UI state) stays in the app, or in `bramble-core` behind mobile's build, unless the extension
   gains it.

   <a id="unassigned-modules"></a>**Unassigned modules.** The steps above name about half of
   `mobile/core/src`. The 22 files below, about 5,800 lines, are named by none, so they all fall
   into 2.7's "what remains", which is written as the smallest step and would be the largest.
   The placements are proposals; each is confirmed when its step starts.

   | Mobile files (lines) | Proposed step | Lands in | Note |
   | --- | --- | --- | --- |
   | `kms_accounts.rs` (198) | 2.2 accounts | `wallet-core`, beside `kms.rs` | names Krusty |
   | `recovery.rs`, `recovery_scan.rs`, `recovery_advisory.rs` (1,398) | 2.2 accounts | `wallet-core`; limits and family order as arguments | oracle: `accounts-parity-recovery.json`, `accounts-parity-rules.json` |
   | `argent_multisig.rs`, `multisig_rpc.rs`, `multisig_store.rs`, `class_declared.rs` (1,008) | 2.5 | constructor layout and authority read in `wallet-core`; registry file and pinned class in `bramble-core` | `argent_multisig.rs` names Krusty |
   | `security.rs` (269) | undecided | backoff over `Clock` and `Storage` is mechanism; the delays are Bramble's | does the extension have a counterpart? |
   | `contacts.rs`, `contact_card.rs`, `groups.rs`, `account_names.rs`, `avatar.rs`, `ui.rs` (1,446) | 2.7 | `bramble-core` | extension product rules: address book, folders, names, the bird, passphrase strength |
   | `wallet.rs`, `wallet_accounts.rs`, `records.rs`, `testing.rs` (1,228) | 2.7 | `bramble-core` | the UniFFI object and its records |
   | `biometric.rs`, `backup.rs`, `public_store.rs` (299) | stays | mobile-only glue (see Open questions) | |

   One constraint orders this. `bramble-core` never names `krusty_kms`, and after M4 mobile
   still does in `kms_accounts.rs`, `argent_multisig.rs`, `account_catalog.rs`, `networks.rs`
   and `send/wallet_ext.rs`. Each of those calls needs a home in `wallet-core` before 2.7.
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
  `stark_public_key(derive_strk20_viewing_key(..))` equals it.
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
  copies it; it sits unused until 2.5.
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
- **The Status table is updated in the PR that changes it**, in either repo's half.
- **Tags are signed.** `v0.1.0` is annotated but unsigned, and a tag is never moved, so it stays
  that way; later tags use `git tag -s`.
- *Proposed*: **mobile may pin a `rev` between tags** during Phase 2. Every tag needs a version
  bump in three files, and `Cargo.lock` records the commit either way. Tags would mark 2.7 and
  each npm publish. `AGENTS.md` says mobile pins a tag; change both together if agreed.
- **Dependabot does not propose what the hard rules forbid.** Add ignores for
  `starknet-types-core` (follows Krusty), `uniffi*` (follows ubrn and `uniffi-runtime-wasm`) and
  `wasm-bindgen` (must equal the CLI), and close #10, #11 and #12.
- **#16 (crate READMEs, crates.io) is split.** The READMEs and a LICENSE file (both crates
  declare MIT and the repo has none) go in now. Publishing waits on the crates.io question
  below. Its rename to `starknet-wallet-core` is the part with a deadline: M2 writes the package
  name into mobile, so the rename lands and is tagged before M2 merges, or is dropped.
- **Housekeeping**: `tests/fixtures/README.md` still says only KMS, accounts and Nostr have
  ported modules; the transaction fixtures and the sealed envelope are read by tests now.

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
- Who owns the mobile vault → v12 migration.
- When a second wallet wants `wallet-core`: publish it to crates.io, and whether it ships its own
  UniFFI feature or each wallet keeps exporting from its own crate (today's answer).
- Which modules count as mobile-only glue, and whether they live in the app or in `bramble-core`
  behind mobile's build (Phase 2).
