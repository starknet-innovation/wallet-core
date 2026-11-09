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

- `crates/wallet-core` — generic custody (Krusty KMS), Nostr coordination, and as the plan lands,
  vault, transactions and portfolio. Plain Rust: no FFI, no UniFFI, no Bramble policy.
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
  Prefer none.
- **No secret leaves through a side door.** Private keys, mnemonics, passphrases and viewing keys
  never reach logs, error messages, `Debug`/`Display` output or UniFFI error strings. Hold them in
  `Zeroizing` where the API allows. Library code has no print macros (checked).
- **The mobile build exports no raw keys.** Anything that takes or returns a raw private key goes
  behind the `raw-keys` feature; `--no-default-features` must stay clean (clippy checks it).
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
- Only `wallet-core` depends on Krusty (`kms.rs`, `nostr.rs`); `bramble-core` goes through
  `wallet-core` and never names `krusty_kms`.

## Commits and PRs

- Imperative summary, optional scope (`ci:`, `kms:`), body says why. One concept per commit.
- Fill in the PR template. A behaviour change comes with a test; a parity change with fixtures.

## Releases

- Both crates share the workspace version. The tag `v<x.y.z>` must equal it and
  `js/package.json`; `release.yml` (npm) and `publish-crates.yml` (crates.io) refuse otherwise.
- npm and crates.io publish through OIDC trusted publishing only. Never add a registry token or
  publish locally. crates.io publishing also waits on a reviewer in the `crates-io` environment.
- A version on crates.io can be yanked but never replaced or deleted: bump, don't retag.
- Only a maintainer tags, and only a merged `main` commit. Never move or recreate a tag.
