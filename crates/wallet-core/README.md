# wallet-core

Generic Starknet wallet logic in plain Rust. Any Starknet wallet can build on it; Bramble does,
through [`bramble-core`](../bramble-core).

## What it is

- **Key custody over Krusty KMS** (`kms`): BIP-39 mnemonic generation and validation, Stark
  derivation at `m/44'/9004'/…`, Ready (Argent legacy) derivation, RFC-6979 Stark signing, STRK20
  viewing keys, OpenZeppelin account class hash and counterfactual address.
- **Nostr coordination identity** (`nostr`): the wallet's BIP-340 key, NIP-59 gift wrap and open
  for application data, with bounds on event and data sizes.
- **The encrypted vault** (`envelope`, `container`): envelope v3 and the DEK container,
  byte-compatible with the extension's, over host storage and a clock (`host`).
- **Sans-IO JSON-RPC and transfers** (`rpc`, `send`): builds node requests and reads their
  replies, and derives and plans a transfer. The host carries the bytes.
- **The felt grammar** (`felt`): strict parsing, canonical lowercase hex and the private-key range.
  Everything else here goes through it.
- **Mechanism, with decisions as arguments.** The account version, the salt, the network: the
  caller passes them in.
- **Secret-careful.** Private keys are `SecretFelt`, mnemonics and passphrases are `Zeroizing`
  where the API allows, and errors carry a fixed reason or operation name, never input text.

As `docs/PLAN.md` lands, portfolio, history and privacy logic join it here.

## What it is NOT

- **Not a cryptography library.** Krusty KMS owns signing, derivation and mnemonic checks. This
  crate wraps it and fails closed when Krusty can't do something; it never carries a fallback.
- **Not a product.** No Bramble policy, no constants like "the account version we deploy", no
  catalogs, no vault schema or migrations. A product choice is an argument here and a constant in
  the wallet's own crate.
- **Not an FFI crate.** No UniFFI, no wasm-bindgen, no `extern "C"`. A wallet exports what it
  needs from its own crate (Bramble's is `bramble-core`).
- **Not a configuration framework.** No traits or config objects for wallets that don't exist yet.
- **Not I/O.** No network, no files, no clock of its own; the host implements `Storage` and
  `Clock`.
- **No logging.** Library code prints nothing, and no secret reaches `Debug`, `Display` or an
  error message.

## Using it

Not on crates.io. Depend on it by git tag; both crates share one version and one `v*` tag.

```toml
wallet-core = { package = "starknet-wallet-core", git = "https://github.com/starknet-innovation/wallet-core", tag = "v0.2.1" }
```

```rust
use wallet_core::{derive_starknet_keypair, sign_stark_hash, Zeroizing};

let mnemonic = Zeroizing::new(phrase);
let pair = derive_starknet_keypair(&mnemonic, 0, 0, None)?;
let signature = sign_stark_hash(&pair.private_key, message_hash)?;
```

Checks run through `bash tools/check.sh` at the repo root; `AGENTS.md` has the rules for changing
this crate.
