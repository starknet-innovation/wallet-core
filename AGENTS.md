# Agent guide — wallet-core

Starknet wallet logic in Rust, shared by Bramble's browser extension (wasm32, npm
`@starknet-innovation/bramble-core`) and its iOS and Android apps (UniFFI). It holds key custody and
signs, so treat every change as security-sensitive. `docs/PLAN.md` owns what goes where and why;
this file is the operational guide.

## Start here

- `bash tools/check.sh quick` before editing, to see any existing drift.
- Read `docs/PLAN.md` and the affected crate's `Cargo.toml` and `src/lib.rs` before changing it.
- Enable the hooks once per clone: `git config core.hooksPath .githooks` (pre-commit: fmt +
  guardrails; pre-push: clippy + tests).

## Commands

`tools/check.sh` is the only command surface; CI calls the same modes. Don't copy its cargo
sequences into workflows or docs.

```bash
bash tools/check.sh quick        # fmt + guardrails
bash tools/check.sh rust         # fmt + clippy (default, mobile, wasm32) + tests
bash tools/check.sh all          # everything CI runs; finish with this
cargo test -p bramble-core <name>   # narrow loop while iterating
```

- `wasm` mode needs `cargo install wasm-bindgen-cli --version 0.2.130 --locked`; the version must
  equal `wasm-bindgen` in `Cargo.lock` (a guardrail checks every place it is written).
- The toolchain is pinned in `rust-toolchain.toml` and repeated as `RUST_TOOLCHAIN` in each
  workflow; change them together (also checked).
- `deny` needs network for the advisory DB. Offline, run the rest and say so.

## Layout

- `crates/wallet-core` (package `starknet-wallet-core`) — generic custody (Krusty KMS), Nostr
  coordination, and as the plan lands, vault, transactions and portfolio. Plain Rust: no FFI, no
  UniFFI, no Bramble policy.
- `crates/bramble-core` — Bramble's choices on top, exported through UniFFI. The only FFI crate.
- `crates/bramble-core/tests/fixtures` — golden fixtures written by the extension (see its README).
- `js/` — the npm package: ubrn-generated wasm bindings plus a thin TypeScript entry.

## Hard rules

- **Krusty KMS owns wallet crypto.** Never add fallback signing, key derivation or mnemonic
  validation; if Krusty can't do it, fail closed. `krusty-kms` stays pinned with `=` and is bumped
  deliberately, never in passing.
- **`starknet-types-core` follows Krusty** (`=0.2.4` today). Bump it only together with
  `krusty-kms`; `cargo tree -i starknet-types-core` must show one version.
- **Every dependency is pinned exactly** (`=x.y.z`), justified in the PR, and used immediately.
  Prefer none. Don't bump one unless a feature needs it or an advisory forces it; Dependabot
  opens no version-update PRs, and advisories reach us through security updates and
  `cargo deny`. Hashes are pinned by `Cargo.lock` (built `--locked`), `js/package-lock.json`
  (`npm ci`, install scripts off in `js/.npmrc`) and action commit SHAs.
- **No secret leaves through a side door.** Private keys, mnemonics, passphrases and viewing keys
  never reach logs, error messages, `Debug`/`Display` output or UniFFI error strings. Hold them in
  `Zeroizing` where the API allows. Library code has no print macros (checked).
- **The mobile build exports no raw keys.** Anything that takes or returns a raw private key goes
  behind the `raw-keys` feature; `--no-default-features --features mobile` must stay clean
  (clippy checks it).
- **Fixtures are evidence, not inputs.** Never edit a file under `tests/fixtures/` to make a test
  pass. Regenerate it in Bramble, copy it unchanged, and update its sha256 row (and the Bramble
  commit line) in the fixtures README.
- **`wallet-core` takes no product policy.** If a rule is Bramble's choice, it lives in
  `bramble-core`.
- `#![forbid(unsafe_code)]` stays on both crates. Fix a lint rather than allow it; the one
  accepted `allow` is `too_many_arguments` on flat UniFFI exports.
- Source files stay under 500 lines (checked); split by behaviour as you approach it.

## Reuse before writing

Bramble's worst consistency bugs came from one agent re-deriving what another had already written.
Grep first; extend in place rather than adding a sibling.

- Felt parsing, canonical hex and the private-key range live in `crates/wallet-core/src/felt.rs`.
- Vault crypto (envelope v3, DEK container, AES-GCM and randomness helpers) lives in
  `crates/wallet-core/src/envelope.rs` and `container.rs`; host I/O goes through `host.rs`.
- JSON-RPC bodies and reply judging live in `rpc.rs` and `send/{rpc_bodies,judge}.rs`; v3
  transaction hashes in `send/sign.rs`; fee bounds in `send/fees.rs`; u256 in `send/u256.rs`.
- JavaScript semantics live in `crates/wallet-core/src/js/`: `trim`, `shortAddress`, UTF-16
  lengths and `Number` formatting in `js::num` (`js_trim`, `short_label`; `send::calls`
  re-exports both), `BigInt` in `js::big`, `JSON` in `js::json`, the budgeted regex matcher in
  `js::regex`, felt helpers over them in `js::felt`. Fixed-UTC dates are `history::time`.
- Ladders, account families, family addresses and constructor calldata live in
  `crates/wallet-core/src/accounts/`; an account's key is `kms::account_key(.., ladder, index)`.
- The recovery scan is `accounts::scan::Scan`, fed a `ScanConfig` (Bramble's shapes, limits,
  tokens, pool and `Authorization`s); its signer and privacy-pool reads and verdicts are
  `accounts::advisory`, whose own viewing public key is `own_viewing_public_key`. A host's batch
  of bodies and replies is `rpc::{Request, Response}`; selectors sit in `send::plan_probe`.
- Argent multisig accounts are `accounts::multisig` (GUIDs, normalised config, calldata, address,
  over a caller's `ArgentMultisigClass`), `accounts::multisig_records` (the extension's
  `ArgentMultisigRecord` and its validators), `accounts::multisig_read` (the pinned-block
  authority read) and `accounts::multisig_declared` (the class-declared read). `lenient_hex_felt`
  (hex in any case with trailing JavaScript whitespace) is `js::felt`.
- The send state machine is `send::plan::Plan`; a wallet's session, keys and activity log reach
  it only through `PlanHost`, and its numbers through `Policy` (Bramble's: `SEND_POLICY`).
- Network and token records (`NetworkInfo`, `TokenInfo`, `TokenRegistry`, borrowed together as
  `Catalogs`) live in `crates/wallet-core/src/catalog.rs`; the core bundles none, the wallet
  passes its own in. A network's catalog spelling (`SN_MAIN`) is `NetworkId::catalog_id`.
- Bramble's catalogs live in `crates/bramble-core/src/catalogs/`, embedded from
  `crates/bramble-core/generated/`: `networks` (`all`, `info`, `registry`, `catalogs()`, and
  `LEGITIMATE_ALIASES`), `accounts` (the class table, family names, `send_authorization`,
  `RecoveryLimits`, `ARGENT_SIGNER_ARRAY`), `account_address` (`resolve_class`, `argent_shape`)
  and `multisig` (`argent_multisig_class`, the limits). Tests load them from there rather than
  rebuilding records from fixture JSON. Bramble's portfolio and history choices are
  `PORTFOLIO_POLICY` and `HISTORY_POLICY` in `bramble-core`'s `lib.rs` (the records are
  `portfolio::PortfolioPolicy` and `history::HistoryPolicy`).
- Portfolio lives in `crates/wallet-core/src/portfolio/`: the price book (`prices::PriceBook`,
  its cache in `price_cache.rs`), saved balances (`snapshots::BalanceSnapshots`), tracked tokens
  (`tokens::TokenPreferences`), token identity, registry and spam (`registry.rs`), on-chain token
  reads (`token_meta.rs`), fiat and amount display (`fiat.rs`), the HTTP and portfolio RPC wire
  contract (`wire.rs`). Account ids and explorer links are `history::explorer`
  (`activity_account_id`, `canonical_account_id`, `explorer_transaction_url`, ...).
- Activity lives in `crates/wallet-core/src/history/` and `activity/`: the row type
  (`history::activity::ActivityEntry`, `ActivityKind`), the Starkscan lane (`starkscan.rs`), the
  RPC history scan (`events::HistoryScan`), the read cadence (`activity_book::ActivityBook`), row
  presentation (`appearance.rs`, `classify.rs`); the one log is `activity::ActivityLog` over
  `host::Storage` (status text and `monotonic` in `activity::store`, the observation windows in
  `activity::observation`).
- Privacy reads live in `crates/wallet-core/src/privacy/`: the read plan
  (`plan::PrivacyReadPlan`, whose viewing key reaches it only through `PrivacyHost`, each reply in
  `plan_steps.rs`), the discovery and pool-fee bodies (`bodies.rs`, the only writer of the key),
  reply judging (`judge.rs`, `notes.rs`), the order-keeping JSON the cursor echo needs
  (`privacy::json`, stricter than `js::json`), routing and limits records (`config.rs`), and the
  gates (`gates.rs`, `home.rs`). `DeploymentState` is `send::types`.
- Nostr relay work lives beside the keys in `crates/wallet-core/src/nostr/`: the relay-URL
  grammar (`relay_url::is_nostr_relay_url`), relay selection, frame parsing, gift-wrap batches and
  publishing (`relay.rs`: `valid_relay_urls`, `parsed_gift_wraps`, `PublishJudge`,
  `publish_many_nostr_gift_wraps`), the paged sync (`sync::SyncJudge`) and the sync window
  (`sync_window.rs`). Bramble's numbers for them (`RELAY_LIMITS`, `SYNC_LOOKBACKS`) and its
  recipient and sender rules are `bramble-core`'s `coordination.rs`.
- Only `wallet-core` depends on Krusty (`kms.rs`, `nostr.rs`, `accounts/`, `send/`); `bramble-core` goes through
  `wallet-core` and never names `krusty_kms`.

## Commits and PRs

- Imperative summary, optional scope (`ci:`, `kms:`), body says why. One concept per commit.
- Fill in the PR template. A behaviour change comes with a test; a parity change with fixtures.

## Releases

- Both crates share the workspace version. The tag `v<x.y.z>` must equal it and
  `js/package.json`; `release.yml` (npm) and `publish-crates.yml` (crates.io) refuse otherwise.
- While `js/package.json` is `private` (until Phase 3.1), `release.yml` makes a GitHub release
  only; the crates still go to crates.io from the same tag, and mobile pins `wallet-core` by that
  tag.
- npm and crates.io publish through OIDC trusted publishing only. Never add a registry token or
  publish locally. crates.io publishing also waits on a reviewer in the `crates-io` environment,
  which admins cannot bypass.
- The one exception: crates.io attaches trusted publishing only to a crate that exists, so a new
  crate name's first version is published once by a maintainer, with a token scoped to that
  crate and revoked straight after, from a clean checkout of the commit that will be tagged
  (`publish-crate.sh` fails the release unless the version on crates.io is that commit's
  package). Then add the trusted publisher (this repo, `publish-crates.yml`, environment
  `crates-io`) before the next tag.
- A version on crates.io can be yanked but never replaced or deleted: bump, don't retag.
- Only a maintainer tags, and only a merged `main` commit. Never move or recreate a tag.
