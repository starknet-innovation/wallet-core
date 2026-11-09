# wallet-core

The Rust core of the Bramble Starknet wallet, shared by the browser extension (wasm32) and the iOS
and Android apps (UniFFI). Key custody runs through Krusty KMS, compiled in and pinned exactly.

```bash
cargo test --locked                                   # unit and extension-parity tests
cargo clippy --all-targets --locked -- -D warnings
cargo build --locked --no-default-features            # mobile profile: no raw-key exports
cargo deny check

cargo install wasm-bindgen-cli --version 0.2.130 --locked   # must match Cargo.lock
cd js && npm ci && npm run build && npm test          # wasm2 bindings, then the fixture through the wasm
```

`docs/PLAN.md` has the phases for moving the extension and mobile onto this crate, and the
decisions behind them. `crates/core/tests/fixtures/README.md` says where the parity fixtures come from.
