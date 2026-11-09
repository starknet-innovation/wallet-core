# Parity fixtures

Written by the Bramble extension's real code path, never by hand:

| File | Generator (in `starknet-innovation/mc-wallet`) | Bramble commit |
| --- | --- | --- |
| `extension-parity.json` | `bun run mobile:parity` → `mobile/core/tests/fixtures/extension-parity.json` | `e6485a7a` (feature/mobile-wallet, 2026-11-09) |
| `accounts-parity-families-sepolia.json` | `bun run mobile:parity` → `mobile/core/tests/fixtures/accounts-parity-families-sepolia.json` | `e6485a7a` |
| `nostr-parity.json` | the `nostrKeys` and `giftWrap` members of `mobile/core/tests/fixtures/nostr-multisig-parity.json` (same generator) | `e6485a7a` |

`tests/conformance/mobile-*.test.ts` in Bramble fails when those files drift from the extension,
so copying them here is the whole sync step. Re-copy when Bramble bumps them, and record the commit.
