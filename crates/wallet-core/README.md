# wallet-core (`starknet-wallet-core`)

Generic Starknet wallet logic in plain Rust. Any Starknet wallet can build on it; Bramble does,
through [`bramble-core`](../bramble-core).

## What it is

- **Key custody over Krusty KMS** (`kms`): BIP-39 mnemonic generation and validation, Stark
  derivation at `m/44'/9004'/…`, Ready (Argent legacy) derivation, RFC-6979 Stark signing, STRK20
  viewing keys, OpenZeppelin account class hash and counterfactual address.
- **Accounts** (`accounts`): which key tree an index walks (`Ladder`), the account families, and
  the counterfactual address and constructor calldata of a key under a caller-given class and
  salt. Argent layouts are checked against Krusty's, and fail closed when they disagree. The
  sans-IO recovery scan (`accounts::scan`) walks a caller-given list of shapes over an index
  ladder, probes each address and keeps what has history; its advisory reads
  (`accounts::advisory`) say whether this wallet can sign for a deployed account and whether a
  privacy pool holds another wallet's viewing key for it. Argent Multisig 0.2 accounts
  (`accounts::multisig*`): signer GUIDs, the GUID-sorted configuration, constructor calldata and
  counterfactual address under a caller-given class and signer limit, the extension's stored
  record shape and its validation rules, and two sans-IO reads: the threshold and signers at one
  pinned block, and whether the class is declared.
- **Nostr coordination identity** (`nostr`): the wallet's BIP-340 key, NIP-59 gift wrap and open
  for application data, with bounds on event and data sizes.
- **The encrypted vault** (`envelope`, `container`): envelope v3 and the DEK container,
  byte-compatible with the extension's, over host storage and a clock (`host`).
- **Sans-IO JSON-RPC and transfers** (`rpc`, `send`): builds node requests and reads their
  replies, and derives and plans a transfer. The host carries the bytes.
- **Portfolio** (`portfolio`, `catalog`): fiat prices and 7-day charts with their
  last-known-good cache, saved balances, tracked tokens with the registry's spam rules, ERC-20
  metadata and balance reads, and the amount and fiat display rules, all sans-IO. Network and
  token catalogs are records the caller passes in.
- **Activity history** (`history`, `activity`): the Starkscan lane and the sans-IO RPC history
  scan, the history-read cadence, the activity row model and day groups, explorer links, and the
  wallet's one activity log (validated entries, status merge, the observation and retirement
  policy) stored through the host's `Storage`.
- **STRK20 privacy reads** (`privacy`): the private-balance read as a sans-IO plan (chain id,
  preflight, note discovery, pool fee) with byte-exact discovery bodies and judged replies, and
  the privacy gates for Home, Receive, token detail and the journey. The viewing key comes from
  the wallet through `PrivacyHost` for each request body and is never stored; routing and limits
  are records the caller passes in. Read-only: nothing is submitted.
- **The felt grammar** (`felt`): strict parsing, canonical lowercase hex and the private-key range.
  Everything else here goes through it.
- **JavaScript semantics** (`js`, `history::time`): `trim`, UTF-16 slices, `Number` and
  `toFixed` formatting, `BigInt` parsing, `JSON`, a step- and depth-budgeted regex matcher that
  fails closed, and `Intl`'s fixed-UTC dates, reproduced bit for bit so display output matches
  the extension's.
- **Mechanism, with decisions as arguments.** The account version, the salt, the network: the
  caller passes them in.
- **Secret-careful.** Private keys are `SecretFelt`, mnemonics and passphrases are `Zeroizing`
  where the API allows, and errors carry a fixed reason or operation name, never input text.

## What it is NOT

- **Not a cryptography library.** Krusty KMS owns signing, derivation and mnemonic checks. This
  crate wraps it and fails closed when Krusty can't do something; it never carries a fallback.
- **Not a product.** No Bramble policy, no constants like "the account version we deploy", no
  bundled catalogs (`catalog` holds their shapes only), no vault schema or migrations. A product choice is an argument here and a constant in
  the wallet's own crate.
- **Not an FFI crate.** No UniFFI, no wasm-bindgen, no `extern "C"`. A wallet exports what it
  needs from its own crate (Bramble's is `bramble-core`).
- **Not a configuration framework.** No traits or config objects for wallets that don't exist yet.
- **Not I/O.** No network, no files, no clock of its own; the host implements `Storage` and
  `Clock`.
- **No logging.** Library code prints nothing, and no secret reaches `Debug`, `Display` or an
  error message.

## Using it

Published to crates.io as `starknet-wallet-core` (lib `starknet_wallet_core`) with `bramble-core`,
from the same `v*` tag and at the same version.

```rust
use starknet_wallet_core::{derive_starknet_keypair, sign_stark_hash, Zeroizing};

let mnemonic = Zeroizing::new(phrase);
let pair = derive_starknet_keypair(&mnemonic, 0, 0, None)?;
let signature = sign_stark_hash(&pair.private_key, message_hash)?;
```

Checks run through `bash tools/check.sh` at the repo root; `AGENTS.md` has the rules for changing
this crate.
