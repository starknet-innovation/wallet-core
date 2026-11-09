# wallet-core

Starknet wallet logic in Rust, in two crates:

- `crates/wallet-core`: generic key custody (Krusty KMS, compiled in and pinned exactly), Nostr
  coordination and, as the plan lands, vault, transactions and portfolio logic. Plain Rust, no FFI,
  no product policy.
- `crates/bramble-core`: the Bramble wallet's choices on top, exported through UniFFI to the browser
  extension (wasm32, npm `@starknet-innovation/bramble-core`) and the iOS and Android apps.

```bash
cargo test --locked                                   # unit and extension-parity tests
cargo clippy --all-targets --locked -- -D warnings
cargo build --locked --no-default-features            # mobile profile: no raw-key exports
cargo deny check

cargo install wasm-bindgen-cli --version 0.2.130 --locked   # must match Cargo.lock
cd js && npm ci && npm run build && npm test          # wasm2 bindings, then the fixture through the wasm
```

`docs/PLAN.md` has what goes in which crate, the phases for moving the extension and mobile onto
`bramble-core`, and the decisions behind them. `crates/bramble-core/tests/fixtures/README.md` says
where the parity fixtures come from.
