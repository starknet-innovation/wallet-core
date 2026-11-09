# wallet-core

Starknet wallet logic in Rust, in two crates:

- `crates/wallet-core`: generic key custody (Krusty KMS, compiled in and pinned exactly), Nostr
  coordination and, as the plan lands, vault, transactions and portfolio logic. Plain Rust, no FFI,
  no product policy.
- `crates/bramble-core`: the Bramble wallet's choices on top, exported through UniFFI to the browser
  extension (wasm32, npm `@starknet-innovation/bramble-core`) and the iOS and Android apps.

```bash
git config core.hooksPath .githooks   # once per clone: fmt + guardrails on commit, clippy + tests on push
bash tools/check.sh quick             # fmt + guardrails
bash tools/check.sh rust              # + clippy (default, mobile, wasm32) + tests
bash tools/check.sh all               # everything CI runs, including cargo-deny and the npm build

cargo install wasm-bindgen-cli --version 0.2.130 --locked   # for `wasm`; must match Cargo.lock
```

`AGENTS.md` has the hard rules and conventions for contributors and coding agents.

`docs/PLAN.md` has what goes in which crate, the phases for moving the extension and mobile onto
`bramble-core`, and the decisions behind them. `crates/bramble-core/tests/fixtures/README.md` is the
golden-fixture manifest: where each file comes from and its sha256.
